use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::model::{now_epoch, Connection};
use crate::screens::form::FormScreen;
use crate::screens::help::{self, HelpOverlay};
use crate::screens::list::{ListModal, ListScreen};
use crate::screens::picker::PickerScreen;
use crate::screens::transfer::{TransferMode, TransferScreen};
use crate::sftp::SftpScreen;
use crate::store::Store;
use crate::{bundle, ssh, ssh_config};

#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    List,
    Form,
    Picker,
    Sftp,
    Transfer,
}

/// What the file browser was opened for. Decides whether it picks files or
/// folders, and which field the answer goes back to.
#[derive(Clone, Copy, PartialEq)]
pub enum PickerTarget {
    KeyFile,
    BundleFile,
    KeysDir,
}

impl PickerTarget {
    pub fn title(self) -> &'static str {
        match self {
            PickerTarget::KeyFile => "Pick key file",
            PickerTarget::BundleFile => "Pick bundle",
            PickerTarget::KeysDir => "Pick folder",
        }
    }
}

pub enum Action {
    None,
    Quit,
    /// `None` = add, `Some(idx)` = edit connection at store index.
    OpenForm(Option<usize>),
    /// Open the form pre-filled as a copy of the connection at store index.
    Duplicate(usize),
    SubmitForm(Option<usize>, Connection),
    CancelForm,
    OpenPicker(PickerTarget, PathBuf),
    PickFile(Option<PathBuf>),
    Connect(usize),
    OpenSftp(usize),
    Delete(usize),
    ToggleFavorite(usize),
    Reload,
    ImportAccept,
    ImportDecline,
    /// Open the encrypted export/import screen.
    OpenTransfer(TransferMode),
    RunExport {
        path: PathBuf,
        passphrase: String,
    },
    /// Decrypt a bundle and work out what importing it would do.
    PlanImport {
        path: PathBuf,
        passphrase: String,
        keys_dir: PathBuf,
    },
    /// Carry out the confirmed import; `overwrite` covers clashing key files.
    ApplyImport {
        overwrite: bool,
    },
    CancelTransfer,
}

pub struct App {
    store: Store,
    screen: Screen,
    list: ListScreen,
    form: Option<FormScreen>,
    picker: Option<PickerScreen>,
    sftp: Option<SftpScreen>,
    transfer: Option<TransferScreen>,
    help: Option<HelpOverlay>,
    should_quit: bool,
    /// True while a protocol-drawn image is on screen (needs cleanup).
    gfx_on_screen: bool,
    /// Force re-emission of the image (resize, overlay toggles, first draw).
    gfx_dirty: bool,
}

impl App {
    pub fn new() -> Result<App> {
        let path = Store::default_path()?;
        let mut list = ListScreen::new();

        let store = match Store::load(path.clone())? {
            Some(store) => store,
            None => {
                // First run: offer to import from ~/.ssh/config.
                let mut store = Store::new_empty(path);
                let imported = read_ssh_config_hosts();
                if imported.is_empty() {
                    store.save()?;
                } else {
                    list.modal = Some(ListModal::ImportPrompt(imported));
                }
                store
            }
        };

        list.refilter(&store.connections);
        Ok(App {
            store,
            screen: Screen::List,
            list,
            form: None,
            picker: None,
            sftp: None,
            transfer: None,
            help: None,
            should_quit: false,
            gfx_on_screen: false,
            gfx_dirty: false,
        })
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            if let Some(sftp) = &mut self.sftp {
                sftp.drain_events();
                if sftp.exit {
                    self.close_sftp();
                }
            }

            // Pick up saves made by other running instances. Only on the
            // list screen with no modal open: forms and modals hold store
            // indices that a reload would shift.
            if self.screen == Screen::List
                && self.list.modal.is_none()
                && self.store.changed_on_disk()
            {
                self.reload_store()?;
            }

            terminal.draw(|frame| self.render(frame))?;
            self.sync_graphics(terminal)?;

            // Short tick while SFTP is active so worker events drain promptly.
            let timeout = if self.screen == Screen::Sftp { 50 } else { 250 };
            if event::poll(Duration::from_millis(timeout))? {
                match event::read()? {
                    Event::Key(key) if key.is_press() => self.on_key(key, terminal)?,
                    Event::Resize(..) => self.gfx_dirty = true,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        match self.screen {
            Screen::List => self.list.render(frame, &self.store.connections),
            Screen::Form => {
                if let Some(form) = &self.form {
                    form.render(frame);
                }
            }
            Screen::Picker => {
                if let Some(picker) = &mut self.picker {
                    picker.render(frame);
                }
            }
            Screen::Sftp => {
                if let Some(sftp) = &mut self.sftp {
                    crate::sftp::ui::render(frame, sftp);
                }
            }
            Screen::Transfer => {
                if let Some(transfer) = &self.transfer {
                    transfer.render(frame);
                }
            }
        }
        if let Some(help) = &self.help {
            help.render(frame);
        }
    }

    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        // Help overlay: any key closes it.
        if self.help.is_some() {
            self.help = None;
            return Ok(());
        }

        // Global bindings.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Ok(());
        }
        if key.code == KeyCode::Char('?') && self.help_allowed() {
            self.help = Some(match self.screen {
                Screen::List => help::for_list(),
                Screen::Form => help::for_form(),
                Screen::Picker => help::for_picker(),
                Screen::Sftp => help::for_sftp(),
                Screen::Transfer => help::for_transfer(),
            });
            return Ok(());
        }

        let action = match self.screen {
            Screen::List => self.list.handle_key(key, &self.store.connections),
            Screen::Form => match &mut self.form {
                Some(form) => form.handle_key(key, &self.store.connections),
                None => Action::None,
            },
            Screen::Picker => match &mut self.picker {
                Some(picker) => picker.handle_key(key),
                None => Action::None,
            },
            Screen::Sftp => {
                let mut shell = None;
                if let Some(sftp) = &mut self.sftp {
                    sftp.on_key(key);
                    if sftp.exit {
                        self.close_sftp();
                    } else if let Some(dir) = sftp.shell_request.take() {
                        shell = Some((sftp.conn.clone(), dir));
                    }
                }
                if let Some((conn, dir)) = shell {
                    self.open_remote_shell(&conn, &dir, terminal)?;
                }
                Action::None
            }
            Screen::Transfer => match &mut self.transfer {
                Some(transfer) => transfer.handle_key(key),
                None => Action::None,
            },
        };
        self.apply(action, terminal)
    }

    /// `?` opens contextual help everywhere except in text-entry contexts
    /// (search/filter modes, passphrase and password fields), where it types
    /// a literal `?`.
    fn help_allowed(&self) -> bool {
        match self.screen {
            Screen::List => !self.list.searching,
            Screen::Form => self.form.as_ref().is_none_or(|f| !f.typing_password()),
            Screen::Picker => self.picker.as_ref().is_none_or(|p| !p.filtering),
            Screen::Sftp => self.sftp.as_ref().is_none_or(|s| s.help_allowed()),
            Screen::Transfer => self.transfer.as_ref().is_none_or(|t| t.help_allowed()),
        }
    }

    fn apply(&mut self, action: Action, terminal: &mut DefaultTerminal) -> Result<()> {
        match action {
            Action::None => {}
            Action::Quit => self.should_quit = true,
            Action::OpenForm(None) => {
                self.form = Some(FormScreen::new_add());
                self.screen = Screen::Form;
            }
            Action::OpenForm(Some(idx)) => {
                if let Some(conn) = self.store.connections.get(idx) {
                    self.form = Some(FormScreen::new_edit(idx, conn));
                    self.screen = Screen::Form;
                }
            }
            Action::Duplicate(idx) => {
                if let Some(conn) = self.store.connections.get(idx) {
                    self.form = Some(FormScreen::new_duplicate(conn, &self.store.connections));
                    self.screen = Screen::Form;
                }
            }
            Action::SubmitForm(editing, conn) => {
                match editing {
                    Some(idx) if idx < self.store.connections.len() => {
                        self.store.connections[idx] = conn;
                    }
                    _ => self.store.connections.push(conn),
                }
                self.store.save()?;
                self.form = None;
                self.screen = Screen::List;
                self.list.refilter(&self.store.connections);
            }
            Action::CancelForm => {
                self.form = None;
                self.screen = Screen::List;
            }
            Action::OpenPicker(target, start) => {
                self.picker = Some(PickerScreen::new(target, start));
                self.screen = Screen::Picker;
            }
            Action::PickFile(path) => {
                // The target also says which screen asked, i.e. where to go back to.
                match self.picker.take().map(|p| p.target) {
                    Some(PickerTarget::KeyFile) => {
                        if let (Some(form), Some(path)) = (&mut self.form, path) {
                            form.set_key_path(path);
                        }
                        self.screen = Screen::Form;
                    }
                    Some(PickerTarget::BundleFile | PickerTarget::KeysDir) => {
                        if let (Some(transfer), Some(path)) = (&mut self.transfer, path) {
                            transfer.set_picked(path);
                        }
                        self.screen = Screen::Transfer;
                    }
                    None => self.screen = Screen::List,
                }
            }
            Action::Connect(idx) => self.connect(idx, terminal)?,
            Action::OpenSftp(idx) => {
                if let Some(conn) = self.store.connections.get(idx) {
                    self.sftp = Some(SftpScreen::new(conn));
                    self.screen = Screen::Sftp;
                }
            }
            Action::Delete(idx) => {
                if idx < self.store.connections.len() {
                    let removed = self.store.connections.remove(idx);
                    self.store.save()?;
                    self.list.refilter(&self.store.connections);
                    self.list.status = Some(format!("Deleted \"{}\"", removed.name));
                }
            }
            Action::ToggleFavorite(idx) => {
                if let Some(conn) = self.store.connections.get_mut(idx) {
                    conn.favorite = !conn.favorite;
                    self.store.save()?;
                    self.list.refilter(&self.store.connections);
                }
            }
            Action::ImportAccept => {
                if let Some(ListModal::ImportPrompt(conns)) = self.list.modal.take() {
                    let count = conns.len();
                    self.store.connections.extend(conns);
                    self.store.save()?;
                    self.list.refilter(&self.store.connections);
                    self.list.status = Some(format!("Imported {count} connection(s)"));
                }
            }
            Action::ImportDecline => {
                self.store.save()?;
            }
            Action::Reload => {
                self.reload_store()?;
                self.list.status = Some(format!(
                    "Reloaded {} connection(s)",
                    self.store.connections.len()
                ));
            }
            Action::OpenTransfer(mode) => {
                self.transfer = Some(match mode {
                    TransferMode::Export => TransferScreen::export(&self.store.connections),
                    TransferMode::Import => TransferScreen::import(),
                });
                self.screen = Screen::Transfer;
            }
            Action::RunExport { path, passphrase } => {
                match bundle::export_to(&path, &passphrase, &self.store.connections) {
                    Ok(report) => {
                        self.transfer = None;
                        self.screen = Screen::List;
                        let mut msg = format!(
                            "Exported {} connection(s) and {} key(s) to {}",
                            report.connections,
                            report.keys,
                            path.display()
                        );
                        if !report.unreadable.is_empty() {
                            msg.push_str(&format!(
                                " — {} key file(s) could not be read",
                                report.unreadable.len()
                            ));
                        }
                        self.list.status = Some(msg);
                    }
                    Err(e) => self.report_transfer_error(&e),
                }
            }
            Action::PlanImport {
                path,
                passphrase,
                keys_dir,
            } => {
                let planned = bundle::read_bundle(&path, &passphrase)
                    .and_then(|b| bundle::plan(&b, &keys_dir, &self.store.connections));
                match planned {
                    Ok(plan) => {
                        if let Some(transfer) = &mut self.transfer {
                            transfer.set_pending(plan);
                        }
                    }
                    Err(e) => self.report_transfer_error(&e),
                }
            }
            Action::ApplyImport { overwrite } => {
                let plan = self.transfer.as_mut().and_then(|t| t.take_pending());
                if let Some(plan) = plan {
                    match self.finish_import(plan, overwrite) {
                        Ok(status) => {
                            self.transfer = None;
                            self.screen = Screen::List;
                            self.list.status = Some(status);
                        }
                        Err(e) => self.report_transfer_error(&e),
                    }
                }
            }
            Action::CancelTransfer => {
                self.transfer = None;
                self.screen = Screen::List;
            }
        }
        Ok(())
    }

    /// Failures on the transfer screen belong on the transfer screen: the user
    /// is one keystroke from fixing the passphrase or the path.
    fn report_transfer_error(&mut self, error: &anyhow::Error) {
        let message = error
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        match &mut self.transfer {
            Some(transfer) => transfer.set_error(message),
            None => self.list.status = Some(format!("failed: {message}")),
        }
    }

    /// Write the imported keys, then merge the connections into the store.
    fn finish_import(&mut self, plan: bundle::ImportPlan, overwrite: bool) -> Result<String> {
        // Keys first: a connection is useless without the key it points at,
        // and a failure here must not leave the store half-updated.
        let written = bundle::apply_keys(&plan, overwrite)?;

        let mut status = format!(
            "Imported {} connection(s) and {written} key file(s)",
            plan.additions.len()
        );
        if !plan.duplicates.is_empty() {
            status.push_str(&format!(", {} already present", plan.duplicates.len()));
        }
        if !plan.renamed.is_empty() {
            status.push_str(&format!(", {} renamed", plan.renamed.len()));
        }
        let kept = if overwrite { 0 } else { plan.conflicts() };
        if kept > 0 {
            status.push_str(&format!(", {kept} existing key(s) kept"));
        }

        self.store.connections.extend(plan.additions);
        self.store.save()?;
        self.list.refilter(&self.store.connections);
        Ok(status)
    }

    /// Re-read the store and refresh the list, keeping the selection on the
    /// same connection when it still exists.
    fn reload_store(&mut self) -> Result<()> {
        let keep = self
            .list
            .selected_store_idx()
            .and_then(|i| self.store.connections.get(i))
            .map(|c| c.name.clone());
        self.store.reload()?;
        self.list.refilter(&self.store.connections);
        if let Some(name) = keep
            && let Some(pos) = self
                .list
                .filtered
                .iter()
                .position(|&i| self.store.connections[i].name == name)
        {
            self.list.selected = pos;
        }
        Ok(())
    }

    /// Hand the terminal over to ssh: the TUI is torn down and this process is
    /// *replaced*, so the session is indistinguishable from typing `ssh`
    /// yourself and ending it drops you at your shell, not back in the app.
    fn connect(&mut self, idx: usize, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(conn) = self.store.connections.get(idx) else {
            return Ok(());
        };
        let mut cmd = ssh::build_command(conn);

        // Anything that must outlive the app has to happen before the exec:
        // there is no coming back to do it afterwards.
        let previous = self.store.connections[idx].last_used;
        self.store.connections[idx].last_used = Some(now_epoch());
        // The timestamp only drives list ordering — never hold up a connection
        // because it could not be written.
        let saved = self.store.save();

        ratatui::restore();
        let err = cmd.exec();

        // Only reachable when the exec itself failed (no ssh on PATH, say):
        // nothing was replaced, so put the TUI and the timestamp back.
        *terminal = ratatui::init();
        terminal.clear().context("re-initializing terminal")?;
        self.store.connections[idx].last_used = previous;
        if saved.is_ok() {
            let _ = self.store.save();
        }
        self.list.refilter(&self.store.connections);
        self.list.status = Some(format!("failed to launch ssh: {err}"));
        Ok(())
    }

    /// Suspend the TUI and open an interactive ssh shell in `dir` on the
    /// remote host; the SFTP screen resumes when the shell exits.
    fn open_remote_shell(
        &mut self,
        conn: &Connection,
        dir: &Path,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        let mut cmd = ssh::build_shell_command(conn, dir);

        ratatui::restore();
        let status = cmd.status();
        *terminal = ratatui::init();
        terminal.clear().context("re-initializing terminal")?;
        self.gfx_dirty = true;

        if let Some(sftp) = &mut self.sftp {
            sftp.shell_returned(status);
        }
        Ok(())
    }

    fn close_sftp(&mut self) {
        self.sftp = None; // Drop joins the worker thread.
        self.screen = Screen::List;
    }

    /// Keep the protocol-drawn image in sync with the UI: emit it right
    /// after the frame when the image modal is topmost, remove it as soon
    /// as anything covers or closes it.
    fn sync_graphics(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        use crate::sftp::{graphics, ui::image_modal_area, Modal};

        let frame_area = terminal.get_frame().area();
        let image = if self.help.is_none() {
            self.sftp.as_ref().and_then(|s| match &s.modal {
                Some(Modal::ImagePreview {
                    graphics: Some(img),
                    ..
                }) => Some(img),
                _ => None,
            })
        } else {
            None
        };

        let mut out = std::io::stdout();
        match image {
            Some(img) => {
                if !self.gfx_on_screen || self.gfx_dirty {
                    if let Some(protocol) = graphics::detect() {
                        let area = image_modal_area(img, frame_area);
                        graphics::emit(protocol, img, area.x + 1, area.y + 1, &mut out)?;
                    }
                    self.gfx_on_screen = true;
                    self.gfx_dirty = false;
                }
            }
            None => {
                if self.gfx_on_screen {
                    if let Some(protocol) = graphics::detect() {
                        graphics::clear(protocol, &mut out)?;
                    }
                    self.gfx_on_screen = false;
                    self.gfx_dirty = false;
                }
            }
        }
        Ok(())
    }
}

fn read_ssh_config_hosts() -> Vec<Connection> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(home.join(".ssh/config")) else {
        return Vec::new();
    };
    ssh_config::to_connections(ssh_config::parse(&content), &home)
}
