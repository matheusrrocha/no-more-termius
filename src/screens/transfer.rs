//! Export / import of an encrypted bundle — the "set up my other machine"
//! screen. Both directions are the same three fields, so they share one form;
//! importing then stops on a summary modal, because it is the direction that
//! writes private keys onto the disk.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{Action, PickerTarget};
use crate::bundle::{self, ImportPlan};
use crate::crypto::MIN_PASSPHRASE;
use crate::model::Connection;
use crate::screens::help::centered_rect;
use crate::theme;

const EXPORT_LABELS: [&str; 3] = ["Bundle file", "Passphrase", "Repeat passphrase"];
const IMPORT_LABELS: [&str; 3] = ["Bundle file", "Passphrase", "Restore keys into"];
const FILE_FIELD: usize = 0;
const PASS_FIELD: usize = 1;
const LAST_FIELD: usize = 2;
const FIELD_COUNT: usize = 3;

#[derive(Clone, Copy, PartialEq)]
pub enum TransferMode {
    Export,
    Import,
}

/// A question the screen is holding the user on. Both directions can destroy
/// something on disk, and neither does so without an answer.
enum Pending {
    /// The export destination already exists.
    ExportOverwrite(PathBuf),
    /// A decrypted bundle, and everything importing it would do.
    Import(ImportPlan),
}

pub struct TransferScreen {
    pub mode: TransferMode,
    fields: [String; FIELD_COUNT],
    focus: usize,
    error: Option<String>,
    /// What an export would contain; empty for imports.
    summary: String,
    pending: Option<Pending>,
}

impl TransferScreen {
    /// Export form, pre-filled with a default destination.
    pub fn export(connections: &[Connection]) -> Self {
        let keys = connections
            .iter()
            .filter_map(|c| c.identity_file.as_ref())
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        Self {
            mode: TransferMode::Export,
            fields: [
                bundle::default_export_path().display().to_string(),
                String::new(),
                String::new(),
            ],
            focus: PASS_FIELD, // the path is usually fine as-is
            error: None,
            summary: format!(
                "{} connection(s) and {keys} key file(s) will be sealed with this passphrase",
                connections.len()
            ),
            pending: None,
        }
    }

    /// Import form, pre-filled with the OS-conventional key folder.
    pub fn import() -> Self {
        Self {
            mode: TransferMode::Import,
            fields: [
                String::new(),
                String::new(),
                bundle::default_keys_dir().display().to_string(),
            ],
            focus: FILE_FIELD,
            error: None,
            summary: String::new(),
            pending: None,
        }
    }

    fn labels(&self) -> [&'static str; FIELD_COUNT] {
        match self.mode {
            TransferMode::Export => EXPORT_LABELS,
            TransferMode::Import => IMPORT_LABELS,
        }
    }

    /// Passphrases are masked; on export so is the repeat.
    fn masked(&self, index: usize) -> bool {
        index == PASS_FIELD || (index == LAST_FIELD && self.mode == TransferMode::Export)
    }

    /// `?` must type a literal `?` into a passphrase, never open help.
    pub fn help_allowed(&self) -> bool {
        self.pending.is_some() || !self.masked(self.focus)
    }

    /// Report a failure (bad passphrase, unwritable path) back onto the form.
    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.pending = None;
    }

    /// Hand over a decrypted import for confirmation.
    pub fn set_pending(&mut self, plan: ImportPlan) {
        self.error = None;
        self.pending = Some(Pending::Import(plan));
    }

    pub fn take_pending(&mut self) -> Option<ImportPlan> {
        match self.pending.take() {
            Some(Pending::Import(plan)) => Some(plan),
            other => {
                self.pending = other;
                None
            }
        }
    }

    /// Result of browsing with Ctrl-o: the field that opened the picker still
    /// has focus, so it is the one that gets the path.
    pub fn set_picked(&mut self, path: PathBuf) {
        self.fields[self.focus] = path.display().to_string();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if self.pending.is_some() {
            return self.handle_confirm_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::CancelTransfer,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % FIELD_COUNT,
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + FIELD_COUNT - 1) % FIELD_COUNT;
            }
            KeyCode::Enter => match self.validate() {
                // Exporting writes a file the user named; if something is
                // already there, ask before replacing it.
                Ok(Action::RunExport { path, .. }) if path.exists() => {
                    self.error = None;
                    self.pending = Some(Pending::ExportOverwrite(path));
                }
                Ok(action) => return action,
                Err(message) => self.error = Some(message),
            },
            KeyCode::Char('o') if ctrl => {
                if let Some(target) = self.picker_target() {
                    return Action::OpenPicker(target, self.picker_start());
                }
            }
            KeyCode::Char('u') if ctrl => self.fields[self.focus].clear(),
            KeyCode::Char(c) if !ctrl => self.fields[self.focus].push(c),
            KeyCode::Backspace => {
                self.fields[self.focus].pop();
            }
            _ => {}
        }
        Action::None
    }

    /// The pending question: `y` goes ahead, `n`/Esc backs out. On the import
    /// summary `o` additionally replaces the clashing key files — overwriting
    /// is never what plain `y` does.
    fn handle_confirm_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('n') | KeyCode::Esc => {
                self.pending = None;
                Action::None
            }
            KeyCode::Char('y') => match &self.pending {
                Some(Pending::Import(_)) => Action::ApplyImport { overwrite: false },
                Some(Pending::ExportOverwrite(path)) => {
                    let action = Action::RunExport {
                        path: path.clone(),
                        passphrase: self.fields[PASS_FIELD].clone(),
                    };
                    self.pending = None;
                    action
                }
                None => Action::None,
            },
            KeyCode::Char('o') if self.conflicts() > 0 => {
                Action::ApplyImport { overwrite: true }
            }
            _ => Action::None,
        }
    }

    fn conflicts(&self) -> usize {
        match &self.pending {
            Some(Pending::Import(plan)) => plan.conflicts(),
            _ => 0,
        }
    }

    fn picker_target(&self) -> Option<PickerTarget> {
        match (self.mode, self.focus) {
            (_, FILE_FIELD) => Some(PickerTarget::BundleFile),
            (TransferMode::Import, LAST_FIELD) => Some(PickerTarget::KeysDir),
            _ => None,
        }
    }

    /// Open the browser where the typed path points, falling back to its
    /// parent and then to home.
    fn picker_start(&self) -> PathBuf {
        let current = bundle::expand_tilde(&self.fields[self.focus]);
        Some(current.clone())
            .filter(|p| p.is_dir())
            .or_else(|| current.parent().filter(|p| p.is_dir()).map(PathBuf::from))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn validate(&self) -> Result<Action, String> {
        let path = self.fields[FILE_FIELD].trim();
        if path.is_empty() {
            return Err("A bundle file is required".into());
        }
        let path = bundle::expand_tilde(path);
        let passphrase = self.fields[PASS_FIELD].clone();

        match self.mode {
            TransferMode::Export => {
                if passphrase.chars().count() < MIN_PASSPHRASE {
                    return Err(format!(
                        "Passphrase must be at least {MIN_PASSPHRASE} characters"
                    ));
                }
                if passphrase != self.fields[LAST_FIELD] {
                    return Err("The two passphrases do not match".into());
                }
                if path.is_dir() {
                    return Err(format!("{} is a directory", path.display()));
                }
                Ok(Action::RunExport { path, passphrase })
            }
            TransferMode::Import => {
                if !path.is_file() {
                    return Err(format!("No such file: {}", path.display()));
                }
                if passphrase.is_empty() {
                    return Err("Passphrase is required".into());
                }
                let keys_dir = self.fields[LAST_FIELD].trim();
                if keys_dir.is_empty() {
                    return Err("A folder for the restored keys is required".into());
                }
                let keys_dir = bundle::expand_tilde(keys_dir);
                if keys_dir.exists() && !keys_dir.is_dir() {
                    return Err(format!("{} is not a folder", keys_dir.display()));
                }
                Ok(Action::PlanImport {
                    path,
                    passphrase,
                    keys_dir,
                })
            }
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let title = match self.mode {
            TransferMode::Export => "Export connections",
            TransferMode::Import => "Import connections",
        };
        let outer = theme::panel(title, true);
        let area = frame.area();
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let rows = Layout::vertical([
            Constraint::Length(3), // file
            Constraint::Length(3), // passphrase
            Constraint::Length(3), // repeat / keys folder
            Constraint::Length(1), // spacer
            Constraint::Length(1), // summary
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error / hints
            Constraint::Min(0),
        ])
        .split(inner);

        for (i, label) in self.labels().iter().enumerate() {
            let focused = self.focus == i;
            let shown = self.display_value(i);
            let placeholder = self.fields[i].is_empty() && !shown.is_empty();
            let style = if placeholder {
                Style::default().fg(theme::DIM)
            } else {
                Style::default()
            };
            frame.render_widget(
                Paragraph::new(Line::styled(shown.clone(), style))
                    .block(theme::panel(*label, focused)),
                rows[i],
            );
            if focused {
                let width = if placeholder { 0 } else { shown.chars().count() };
                frame.set_cursor_position(Position::new(
                    rows[i].x + 1 + width as u16,
                    rows[i].y + 1,
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {}", self.note()),
                Style::default().fg(theme::DIM),
            )),
            rows[4],
        );

        let footer = match &self.error {
            Some(err) => Line::styled(format!(" {err}"), Style::default().fg(theme::DANGER)),
            None => theme::hints(&[
                ("Enter", self.submit_hint()),
                ("Esc", "cancel"),
                ("Tab", "next field"),
                ("Ctrl-o", "browse"),
                ("?", "help"),
            ]),
        };
        frame.render_widget(Paragraph::new(footer), rows[6]);

        match &self.pending {
            Some(Pending::Import(plan)) => render_import_confirm(frame, plan),
            Some(Pending::ExportOverwrite(path)) => render_export_confirm(frame, path),
            None => {}
        }
    }

    /// Masked fields render as bullets; empty ones show a hint instead.
    fn display_value(&self, index: usize) -> String {
        let value = &self.fields[index];
        if self.masked(index) {
            return "•".repeat(value.chars().count());
        }
        if !value.is_empty() {
            return value.clone();
        }
        match (self.mode, index) {
            (TransferMode::Import, FILE_FIELD) => "(Ctrl-o to browse)".into(),
            _ => String::new(),
        }
    }

    fn note(&self) -> String {
        match self.mode {
            TransferMode::Export if !self.summary.is_empty() => self.summary.clone(),
            TransferMode::Export => String::new(),
            TransferMode::Import => {
                "Use the passphrase the bundle was exported with.".into()
            }
        }
    }

    fn submit_hint(&self) -> &'static str {
        match self.mode {
            TransferMode::Export => "export",
            TransferMode::Import => "open bundle",
        }
    }
}

fn render_export_confirm(frame: &mut Frame, path: &std::path::Path) {
    let lines = vec![
        Line::raw("A file is already there:"),
        Line::styled(
            path.display().to_string(),
            Style::default().fg(theme::ACCENT),
        ),
        Line::default(),
        Line::raw("Replace it?"),
        Line::default(),
        Line::from(vec![
            Span::styled("y", Style::default().fg(theme::OK).bold()),
            Span::raw(" replace   "),
            Span::styled("n", Style::default().fg(theme::DANGER).bold()),
            Span::raw(" keep it"),
        ])
        .centered(),
    ];
    render_modal(frame, "Overwrite", lines);
}

fn render_import_confirm(frame: &mut Frame, plan: &ImportPlan) {
    let conflicts = plan.conflicts();
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Add "),
            Span::styled(plan.additions.len().to_string(), Style::default().bold()),
            Span::raw(" connection(s) and write "),
            Span::styled(plan.writes(false).to_string(), Style::default().bold()),
            Span::raw(" key file(s) to"),
        ]),
        Line::styled(
            plan.keys_dir.display().to_string(),
            Style::default().fg(theme::ACCENT),
        ),
    ];
    if !plan.duplicates.is_empty() {
        lines.push(Line::styled(
            format!("{} already here, skipped", plan.duplicates.len()),
            Style::default().fg(theme::DIM),
        ));
    }
    if !plan.renamed.is_empty() {
        let (from, to) = &plan.renamed[0];
        let extra = match plan.renamed.len() {
            1 => String::new(),
            n => format!(" (+{} more)", n - 1),
        };
        lines.push(Line::styled(
            format!("name taken: \"{from}\" → \"{to}\"{extra}"),
            Style::default().fg(theme::DIM),
        ));
    }
    if conflicts > 0 {
        lines.push(Line::default());
        lines.push(Line::styled(
            format!("{conflicts} key file(s) already exist there with different contents"),
            Style::default().fg(theme::FAVORITE),
        ));
    }

    lines.push(Line::default());
    let mut answers = vec![
        Span::styled("y", Style::default().fg(theme::OK).bold()),
        Span::raw(if conflicts > 0 { " import, keep those   " } else { " yes   " }),
    ];
    if conflicts > 0 {
        answers.push(Span::styled("o", Style::default().fg(theme::FAVORITE).bold()));
        answers.push(Span::raw(" import, overwrite them   "));
    }
    answers.push(Span::styled("n", Style::default().fg(theme::DANGER).bold()));
    answers.push(Span::raw(" cancel"));
    lines.push(Line::from(answers).centered());

    render_modal(frame, "Import", lines);
}

fn render_modal(frame: &mut Frame, title: &str, lines: Vec<Line>) {
    let width = lines.iter().map(|l| l.width()).max().unwrap_or(40) as u16 + 6;
    let height = lines.len() as u16 + 2;
    let area = centered_rect(width, height, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(theme::modal(title, Color::Yellow)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(screen: &mut TransferScreen, code: KeyCode) -> Action {
        screen.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_str(screen: &mut TransferScreen, text: &str) {
        for c in text.chars() {
            press(screen, KeyCode::Char(c));
        }
    }

    fn conn(name: &str) -> Connection {
        Connection {
            name: name.into(),
            host: "h".into(),
            port: 22,
            user: None,
            identity_file: None,
            password: None,
            sftp_dir: None,
            favorite: false,
            last_used: None,
        }
    }

    fn error_of(screen: &mut TransferScreen) -> String {
        press(screen, KeyCode::Enter);
        screen.error.clone().unwrap_or_default()
    }

    #[test]
    fn export_rejects_a_short_passphrase() {
        let mut screen = TransferScreen::export(&[conn("a")]);
        type_str(&mut screen, "short");
        assert!(error_of(&mut screen).contains("at least"));
    }

    #[test]
    fn export_rejects_a_mistyped_repeat() {
        let mut screen = TransferScreen::export(&[conn("a")]);
        type_str(&mut screen, "passphrase123");
        press(&mut screen, KeyCode::Tab);
        type_str(&mut screen, "passphrase124");
        assert!(error_of(&mut screen).contains("do not match"));
    }

    #[test]
    fn export_submits_the_expanded_path_and_the_passphrase() {
        let mut screen = TransferScreen::export(&[conn("a")]);
        press(&mut screen, KeyCode::BackTab); // back to the file field
        screen.fields[FILE_FIELD] = "~/out.nmt".into();
        press(&mut screen, KeyCode::Tab);
        type_str(&mut screen, "passphrase123");
        press(&mut screen, KeyCode::Tab);
        type_str(&mut screen, "passphrase123");

        match press(&mut screen, KeyCode::Enter) {
            Action::RunExport { path, passphrase } => {
                assert_eq!(path, dirs::home_dir().unwrap().join("out.nmt"));
                assert_eq!(passphrase, "passphrase123");
            }
            _ => panic!("expected an export"),
        }
    }

    #[test]
    fn import_rejects_a_missing_file() {
        let mut screen = TransferScreen::import();
        type_str(&mut screen, "/definitely/not/here.nmt");
        assert!(error_of(&mut screen).contains("No such file"));
    }

    #[test]
    fn import_defaults_to_the_conventional_key_folder() {
        let screen = TransferScreen::import();
        assert_eq!(
            screen.fields[LAST_FIELD],
            dirs::home_dir().unwrap().join(".ssh").display().to_string()
        );
    }

    /// A `?` in a passphrase must reach the field, not open the help overlay.
    #[test]
    fn help_stays_out_of_the_way_of_passphrases() {
        let mut screen = TransferScreen::import();
        assert!(screen.help_allowed(), "file field is not a secret");
        press(&mut screen, KeyCode::Tab);
        assert!(!screen.help_allowed(), "passphrase field is");
        press(&mut screen, KeyCode::Tab);
        assert!(screen.help_allowed(), "keys folder is not");
    }

    #[test]
    fn passphrases_are_never_drawn() {
        let mut screen = TransferScreen::export(&[conn("a")]);
        type_str(&mut screen, "hunter2hunter2");

        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|frame| screen.render(frame)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!rendered.contains("hunter2"), "passphrase is on screen");
        assert!(rendered.contains("••••"), "no mask was drawn");
    }

    #[test]
    fn browsing_targets_the_focused_field() {
        let mut screen = TransferScreen::import();
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        match screen.handle_key(ctrl_o) {
            Action::OpenPicker(PickerTarget::BundleFile, _) => {}
            _ => panic!("expected the bundle-file browser"),
        }
        press(&mut screen, KeyCode::Tab); // passphrase — nothing to browse
        assert!(matches!(screen.handle_key(ctrl_o), Action::None));
        press(&mut screen, KeyCode::Tab);
        match screen.handle_key(ctrl_o) {
            Action::OpenPicker(PickerTarget::KeysDir, _) => {}
            _ => panic!("expected the folder browser"),
        }
    }

    fn plan_of(additions: Vec<Connection>) -> ImportPlan {
        ImportPlan {
            keys_dir: PathBuf::from("/tmp/nmt-keys"),
            keys: Vec::new(),
            additions,
            duplicates: Vec::new(),
            renamed: Vec::new(),
        }
    }

    #[test]
    fn the_import_summary_waits_for_a_deliberate_answer() {
        let mut screen = TransferScreen::import();
        screen.set_pending(plan_of(vec![conn("a")]));

        // Stray keys must not start writing keys into ~/.ssh.
        assert!(matches!(press(&mut screen, KeyCode::Char('z')), Action::None));
        assert!(screen.pending.is_some());

        assert!(matches!(
            press(&mut screen, KeyCode::Char('y')),
            Action::ApplyImport { overwrite: false }
        ));
    }

    /// `o` is only ever a way to say "yes, and overwrite"; with nothing to
    /// overwrite it must not become a second way to confirm.
    #[test]
    fn overwriting_is_not_offered_when_no_key_file_clashes() {
        let mut screen = TransferScreen::import();
        screen.set_pending(plan_of(vec![conn("a")]));
        assert!(matches!(press(&mut screen, KeyCode::Char('o')), Action::None));
        assert!(screen.pending.is_some());
    }

    #[test]
    fn declining_the_import_leaves_the_form_filled_in() {
        let mut screen = TransferScreen::import();
        type_str(&mut screen, "/tmp/b.nmt");
        screen.set_pending(plan_of(vec![conn("a")]));
        assert!(matches!(press(&mut screen, KeyCode::Char('n')), Action::None));
        assert!(screen.pending.is_none());
        assert_eq!(screen.fields[FILE_FIELD], "/tmp/b.nmt");
    }

    /// Exporting onto an existing file replaces it, so it has to be asked.
    #[test]
    fn exporting_over_an_existing_file_asks_first() {
        let existing = std::env::temp_dir().join(format!("nmt-export-{}.nmt", std::process::id()));
        std::fs::write(&existing, "old").unwrap();

        let mut screen = TransferScreen::export(&[conn("a")]);
        screen.fields[FILE_FIELD] = existing.display().to_string();
        type_str(&mut screen, "passphrase123");
        press(&mut screen, KeyCode::Tab);
        type_str(&mut screen, "passphrase123");

        assert!(matches!(press(&mut screen, KeyCode::Enter), Action::None));
        assert!(screen.pending.is_some(), "no confirmation was raised");

        // n leaves the file alone; y hands the export through unchanged.
        assert!(matches!(press(&mut screen, KeyCode::Char('n')), Action::None));
        assert!(screen.pending.is_none());
        press(&mut screen, KeyCode::Enter);
        match press(&mut screen, KeyCode::Char('y')) {
            Action::RunExport { path, passphrase } => {
                assert_eq!(path, existing);
                assert_eq!(passphrase, "passphrase123");
            }
            _ => panic!("expected the export to go ahead"),
        }
        std::fs::remove_file(&existing).ok();
    }

    #[test]
    fn a_picked_path_lands_in_the_field_that_asked_for_it() {
        let mut screen = TransferScreen::import();
        screen.set_picked(PathBuf::from("/tmp/b.nmt"));
        assert_eq!(screen.fields[FILE_FIELD], "/tmp/b.nmt");
    }
}
