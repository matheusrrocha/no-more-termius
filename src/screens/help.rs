//! Contextual help overlay, opened with `?` from any screen.
//! The section for the current screen always comes first.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

pub struct HelpSection {
    pub title: &'static str,
    pub entries: &'static [(&'static str, &'static str)],
}

pub struct HelpOverlay {
    pub title: &'static str,
    pub sections: Vec<HelpSection>,
}

const GLOBAL: HelpSection = HelpSection {
    title: "Global",
    entries: &[
        ("?", "toggle this help"),
        ("Ctrl-c", "quit the application"),
        ("Esc", "clear filter / go back / quit"),
    ],
};

pub fn for_list() -> HelpOverlay {
    HelpOverlay {
        title: " Help — Connections ",
        sections: vec![
            HelpSection {
                title: "Connections",
                entries: &[
                    ("/", "fuzzy-search (Enter connects, Esc leaves)"),
                    ("Enter", "connect (ssh) to selection"),
                    ("s", "open SFTP browser for selection"),
                    ("j/k  ↑/↓", "move selection"),
                    ("g / G", "first / last"),
                    ("a", "add new connection"),
                    ("e", "edit selection"),
                    ("y", "duplicate selection"),
                    ("f", "toggle favorite ★"),
                    ("d", "delete selection (asks y/n)"),
                    ("q", "quit"),
                ],
            },
            GLOBAL,
        ],
    }
}

pub fn for_form() -> HelpOverlay {
    HelpOverlay {
        title: " Help — Edit connection ",
        sections: vec![
            HelpSection {
                title: "Form",
                entries: &[
                    ("Tab / ↓", "next field"),
                    ("Shift-Tab / ↑", "previous field"),
                    ("Ctrl-o", "browse for key file (on Key field)"),
                    ("Space", "toggle favorite (on Favorite field)"),
                    ("Enter", "save connection"),
                    ("Esc", "cancel without saving"),
                ],
            },
            GLOBAL,
        ],
    }
}

pub fn for_picker() -> HelpOverlay {
    HelpOverlay {
        title: " Help — Key file picker ",
        sections: vec![
            HelpSection {
                title: "File picker",
                entries: &[
                    ("/", "filter entries in this directory"),
                    ("Enter / l", "enter directory or pick file"),
                    ("h / Backspace", "parent directory"),
                    ("j/k  ↑/↓", "move selection"),
                    (".", "show/hide hidden files"),
                    ("Esc / q", "cancel back to form"),
                ],
            },
            GLOBAL,
        ],
    }
}

pub fn for_sftp() -> HelpOverlay {
    HelpOverlay {
        title: " Help — SFTP ",
        sections: vec![
            HelpSection {
                title: "SFTP",
                entries: &[
                    ("Tab", "switch local ⇄ remote pane"),
                    ("Enter (on file)", "transfer to the other pane"),
                    ("Enter / l (on dir)", "enter directory"),
                    ("h / Backspace", "parent directory"),
                    ("/", "filter files in active pane"),
                    ("Space", "preview text/images in a modal"),
                    ("y", "copy path to clipboard"),
                    ("R", "rename selection"),
                    ("D", "delete selection (asks y/n)"),
                    ("j/k ↑/↓ PgUp/PgDn g/G", "move selection"),
                    (".", "show/hide hidden files"),
                    ("r", "refresh both panes"),
                    ("Esc", "cancel transfer / clear filter / leave"),
                    ("q", "leave SFTP"),
                    ("y / n", "answer overwrite & host-key prompts"),
                ],
            },
            GLOBAL,
        ],
    }
}

impl HelpOverlay {
    pub fn render(&self, frame: &mut Frame) {
        let key_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let section_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);

        let key_width = self
            .sections
            .iter()
            .flat_map(|s| s.entries.iter())
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(8);

        let mut lines: Vec<Line> = Vec::new();
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::styled(section.title, section_style));
            for (key, desc) in section.entries {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key:key_width$}  "), key_style),
                    Span::raw(*desc),
                ]));
            }
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            "press any key to close",
            Style::default().fg(Color::DarkGray),
        ));

        let width = (lines
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(30)
            .max(self.title.len())
            + 4) as u16;
        let height = lines.len() as u16 + 2;
        let area = centered_rect(width, height, frame.area());

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(crate::theme::modal(self.title.trim(), Color::Cyan)),
            area,
        );
    }
}

pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}
