//! Add/edit connection form.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::Action;
use crate::theme;
use crate::model::Connection;

const FIELD_LABELS: [&str; 5] = ["Name", "Host", "Port", "User", "Key file"];
const FAVORITE_FIELD: usize = 5;

pub struct FormScreen {
    pub editing: Option<usize>,
    fields: [String; 5],
    favorite: bool,
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
            ],
            favorite: conn.favorite,
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

    pub fn set_key_path(&mut self, path: PathBuf) {
        self.fields[4] = path.display().to_string();
    }

    pub fn handle_key(&mut self, key: KeyEvent, connections: &[Connection]) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::CancelForm,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % 6,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 5) % 6,
            KeyCode::Enter => match self.validate(connections) {
                Ok(conn) => return Action::SubmitForm(self.editing, conn),
                Err(msg) => self.error = Some(msg),
            },
            KeyCode::Char('o') if ctrl => {
                if self.focus == 4 {
                    return Action::OpenPicker(self.picker_start());
                }
            }
            KeyCode::Char(' ') if self.focus == FAVORITE_FIELD => {
                self.favorite = !self.favorite;
            }
            KeyCode::Char(c) if !ctrl && self.focus < 5 => {
                if self.focus == 2 && !c.is_ascii_digit() {
                    return Action::None; // port accepts digits only
                }
                self.fields[self.focus].push(c);
            }
            KeyCode::Backspace if self.focus < 5 => {
                self.fields[self.focus].pop();
            }
            _ => {}
        }
        Action::None
    }

    fn picker_start(&self) -> PathBuf {
        let current = PathBuf::from(self.fields[4].trim());
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
        let port = if self.fields[2].trim().is_empty() {
            22
        } else {
            self.fields[2]
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("Port must be between 1 and 65535")?
        };
        let user = Some(self.fields[3].trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let key = self.fields[4].trim();
        let identity_file = if key.is_empty() {
            None
        } else {
            let path = PathBuf::from(key);
            if !path.is_file() {
                return Err(format!("Key file not found: {key}"));
            }
            Some(path)
        };
        Ok(Connection {
            name: name.into(),
            host: host.into(),
            port,
            user,
            identity_file,
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
            Constraint::Length(1), // favorite
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error/hints
            Constraint::Min(0),
        ])
        .split(inner);

        for (i, label) in FIELD_LABELS.iter().enumerate() {
            let focused = self.focus == i;
            let mut text = self.fields[i].clone();
            if i == 4 && text.is_empty() {
                text = "(optional — Ctrl-o to browse)".into();
            }
            let value_style = if i == 4 && self.fields[4].is_empty() {
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
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
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
        frame.render_widget(Paragraph::new(footer), rows[7]);
    }
}

/// "name (copy)", "name (copy 2)", ... — first variant not already taken.
pub fn duplicate_name(base: &str, connections: &[Connection]) -> String {
    let taken = |candidate: &str| connections.iter().any(|c| c.name == candidate);
    let first = format!("{base} (copy)");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{base} (copy {n})"))
        .find(|candidate| !taken(candidate))
        .unwrap()
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
            favorite: true,
            last_used: Some(123),
        };
        let form = FormScreen::new_duplicate(&original, std::slice::from_ref(&original));
        assert_eq!(form.editing, None);
        assert_eq!(form.fields[0], "web (copy)");
        assert_eq!(form.fields[1], "10.0.0.1");
        assert_eq!(form.fields[2], "2222");
        assert_eq!(form.fields[3], "root");
        assert!(form.favorite);
        assert_eq!(form.last_used, None);
    }
}
