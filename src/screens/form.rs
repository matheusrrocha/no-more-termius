//! Add/edit connection form.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{Action, PickerTarget};
use crate::model::{unique_name, Connection};
use crate::theme;

const FIELD_LABELS: [&str; 7] = [
    "Name",
    "Host",
    "Port",
    "User",
    "Key file",
    "Password",
    "SFTP folder",
];
const PORT_FIELD: usize = 2;
const KEY_FIELD: usize = 4;
const PASSWORD_FIELD: usize = 5;
const SFTP_DIR_FIELD: usize = 6;
const FAVORITE_FIELD: usize = 7;
const FOCUS_COUNT: usize = 8;

pub struct FormScreen {
    pub editing: Option<usize>,
    fields: [String; 7],
    favorite: bool,
    /// Ctrl-r: show the password field in the clear.
    reveal: bool,
    focus: usize,
    error: Option<String>,
    /// Preserved so an edit keeps its last_used timestamp.
    last_used: Option<u64>,
}

impl FormScreen {
    pub fn new_add() -> Self {
        Self {
            editing: None,
            fields: Default::default(),
            favorite: false,
            reveal: false,
            focus: 0,
            error: None,
            last_used: None,
        }
    }

    pub fn new_edit(idx: usize, conn: &Connection) -> Self {
        Self {
            editing: Some(idx),
            fields: [
                conn.name.clone(),
                conn.host.clone(),
                conn.port.to_string(),
                conn.user.clone().unwrap_or_default(),
                conn.identity_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                conn.password.clone().unwrap_or_default(),
                conn.sftp_dir.clone().unwrap_or_default(),
            ],
            favorite: conn.favorite,
            reveal: false,
            focus: 0,
            error: None,
            last_used: conn.last_used,
        }
    }

    /// Pre-filled copy of an existing connection, saved as a NEW entry.
    pub fn new_duplicate(conn: &Connection, connections: &[Connection]) -> Self {
        let mut form = Self::new_edit(0, conn);
        form.editing = None;
        form.fields[0] = duplicate_name(&conn.name, connections);
        form.last_used = None;
        form
    }

    /// On the password field `?` is a character, not the help key.
    pub fn typing_password(&self) -> bool {
        self.focus == PASSWORD_FIELD
    }

    pub fn set_key_path(&mut self, path: PathBuf) {
        self.fields[KEY_FIELD] = path.display().to_string();
    }

    pub fn handle_key(&mut self, key: KeyEvent, connections: &[Connection]) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::CancelForm,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % FOCUS_COUNT,
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + FOCUS_COUNT - 1) % FOCUS_COUNT;
            }
            KeyCode::Enter => match self.validate(connections) {
                Ok(conn) => return Action::SubmitForm(self.editing, conn),
                Err(msg) => self.error = Some(msg),
            },
            KeyCode::Char('o') if ctrl => {
                if self.focus == KEY_FIELD {
                    return Action::OpenPicker(PickerTarget::KeyFile, self.picker_start());
                }
            }
            KeyCode::Char('r') if ctrl => {
                if self.focus == PASSWORD_FIELD {
                    self.reveal = !self.reveal;
                }
            }
            KeyCode::Char(' ') if self.focus == FAVORITE_FIELD => {
                self.favorite = !self.favorite;
            }
            KeyCode::Char(c) if !ctrl && self.focus < FAVORITE_FIELD => {
                if self.focus == PORT_FIELD && !c.is_ascii_digit() {
                    return Action::None; // port accepts digits only
                }
                self.fields[self.focus].push(c);
            }
            KeyCode::Backspace if self.focus < FAVORITE_FIELD => {
                self.fields[self.focus].pop();
            }
            _ => {}
        }
        Action::None
    }

    fn picker_start(&self) -> PathBuf {
        let current = PathBuf::from(self.fields[KEY_FIELD].trim());
        current
            .parent()
            .filter(|p| p.is_dir())
            .map(|p| p.to_path_buf())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn validate(&self, connections: &[Connection]) -> Result<Connection, String> {
        let name = self.fields[0].trim();
        let host = self.fields[1].trim();
        if name.is_empty() {
            return Err("Name is required".into());
        }
        if host.is_empty() {
            return Err("Host is required".into());
        }
        let duplicate = connections
            .iter()
            .enumerate()
            .any(|(i, c)| Some(i) != self.editing && c.name == name);
        if duplicate {
            return Err(format!("A connection named \"{name}\" already exists"));
        }
        let port = if self.fields[PORT_FIELD].trim().is_empty() {
            22
        } else {
            self.fields[PORT_FIELD]
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("Port must be between 1 and 65535")?
        };
        let user = Some(self.fields[3].trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let key = self.fields[KEY_FIELD].trim();
        let identity_file = if key.is_empty() {
            None
        } else {
            let path = PathBuf::from(key);
            if !path.is_file() {
                return Err(format!("Key file not found: {key}"));
            }
            Some(path)
        };
        // Not trimmed: spaces can be part of a password.
        let password = Some(&self.fields[PASSWORD_FIELD])
            .filter(|s| !s.is_empty())
            .cloned();
        let sftp_dir = Some(self.fields[SFTP_DIR_FIELD].trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        Ok(Connection {
            name: name.into(),
            host: host.into(),
            port,
            user,
            identity_file,
            password,
            sftp_dir,
            favorite: self.favorite,
            last_used: self.last_used,
        })
    }

    pub fn render(&self, frame: &mut Frame) {
        let title = if self.editing.is_some() {
            "Edit connection"
        } else {
            "New connection"
        };
        let outer = theme::panel(title, true);
        let area = frame.area();
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let rows = Layout::vertical([
            Constraint::Length(3), // name
            Constraint::Length(3), // host
            Constraint::Length(3), // port
            Constraint::Length(3), // user
            Constraint::Length(3), // key
            Constraint::Length(3), // password
            Constraint::Length(3), // sftp folder
            Constraint::Length(1), // favorite
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error/hints
            Constraint::Min(0),
        ])
        .split(inner);

        for (i, label) in FIELD_LABELS.iter().enumerate() {
            let focused = self.focus == i;
            let mut text = if i == PASSWORD_FIELD && !self.reveal {
                "•".repeat(self.fields[i].chars().count())
            } else {
                self.fields[i].clone()
            };
            if text.is_empty() {
                text = match i {
                    KEY_FIELD => "(optional — Ctrl-o to browse)".into(),
                    PASSWORD_FIELD => "(optional — typed into ssh's password prompt for you)".into(),
                    SFTP_DIR_FIELD => "(optional — remote dir to open, e.g. /var/www)".into(),
                    _ => text,
                };
            }
            let value_style = if self.fields[i].is_empty()
                && matches!(i, KEY_FIELD | PASSWORD_FIELD | SFTP_DIR_FIELD)
            {
                Style::default().fg(theme::DIM)
            } else {
                Style::default()
            };
            frame.render_widget(
                Paragraph::new(Line::styled(text, value_style))
                    .block(theme::panel(*label, focused)),
                rows[i],
            );
            if focused {
                frame.set_cursor_position(Position::new(
                    rows[i].x + 1 + self.fields[i].chars().count() as u16,
                    rows[i].y + 1,
                ));
            }
        }

        let fav_style = if self.focus == FAVORITE_FIELD {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let fav_mark = if self.favorite { "★" } else { " " };
        frame.render_widget(
            Paragraph::new(format!("  [{fav_mark}] Favorite  (Space toggles)")).style(fav_style),
            rows[FAVORITE_FIELD],
        );

        let footer = match &self.error {
            Some(err) => Line::styled(format!(" {err}"), Style::default().fg(theme::DANGER)),
            None => theme::hints(&[
                ("Enter", "save"),
                ("Esc", "cancel"),
                ("Tab", "next field"),
                ("?", "help"),
            ]),
        };
        frame.render_widget(Paragraph::new(footer), rows[FAVORITE_FIELD + 2]);
    }
}

/// "name (copy)", "name (copy 2)", ... — first variant not already taken.
pub fn duplicate_name(base: &str, connections: &[Connection]) -> String {
    unique_name(base, "copy", |candidate| {
        connections.iter().any(|c| c.name == candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn duplicate_names_stay_unique() {
        let conns = vec![conn("web")];
        assert_eq!(duplicate_name("web", &conns), "web (copy)");

        let conns = vec![conn("web"), conn("web (copy)")];
        assert_eq!(duplicate_name("web", &conns), "web (copy 2)");

        let conns = vec![conn("web"), conn("web (copy)"), conn("web (copy 2)")];
        assert_eq!(duplicate_name("web", &conns), "web (copy 3)");
    }

    #[test]
    fn duplicate_form_is_a_new_entry_with_copied_fields() {
        let original = Connection {
            name: "web".into(),
            host: "10.0.0.1".into(),
            port: 2222,
            user: Some("root".into()),
            identity_file: None,
            password: None,
            sftp_dir: Some("/var/www".into()),
            favorite: true,
            last_used: Some(123),
        };
        let form = FormScreen::new_duplicate(&original, std::slice::from_ref(&original));
        assert_eq!(form.editing, None);
        assert_eq!(form.fields[0], "web (copy)");
        assert_eq!(form.fields[1], "10.0.0.1");
        assert_eq!(form.fields[2], "2222");
        assert_eq!(form.fields[3], "root");
        assert_eq!(form.fields[SFTP_DIR_FIELD], "/var/www");
        assert!(form.favorite);
        assert_eq!(form.last_used, None);
    }

    #[test]
    fn password_round_trips_untrimmed_and_types_question_marks() {
        let mut form = FormScreen::new_add();
        form.fields[0] = "web".into();
        form.fields[1] = "10.0.0.1".into();
        form.focus = PASSWORD_FIELD;
        assert!(form.typing_password());
        for c in " pa?ss ".chars() {
            form.handle_key(KeyEvent::from(KeyCode::Char(c)), &[]);
        }
        let conn = form.validate(&[]).unwrap();
        assert_eq!(conn.password.as_deref(), Some(" pa?ss "));

        let edit = FormScreen::new_edit(0, &conn);
        assert_eq!(edit.fields[PASSWORD_FIELD], " pa?ss ");

        form.fields[PASSWORD_FIELD].clear();
        assert_eq!(form.validate(&[]).unwrap().password, None);
    }
}
