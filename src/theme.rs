//! Central look & feel: rounded borders, one accent color, dim chrome,
//! full-row selection and a key-hint bar — lazygit-inspired.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;
pub const FAVORITE: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;
pub const OK: Color = Color::Green;
pub const DIR: Color = Color::Blue;
pub const LINK: Color = Color::Magenta;

/// Rounded panel; focused panels get the accent border + bold title.
pub fn panel(title: impl Into<String>, focused: bool) -> Block<'static> {
    let (border, title_style) = if focused {
        (
            Style::default().fg(ACCENT),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default().fg(DIM), Style::default().fg(DIM))
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Line::styled(format!(" {} ", title.into()), title_style))
}

/// Modal panel: rounded, colored border and bold title.
pub fn modal(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Line::styled(
            format!(" {} ", title.into()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

/// Full-row selection style for lists.
pub fn selection() -> Style {
    Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

pub const SELECTION_SYMBOL: &str = "▌";

/// Key-hint bar: `key` in accent, description dimmed, dot-separated.
pub fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, desc)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {desc}"),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

/// Transient status line (results of actions, errors).
pub fn status_line(msg: &str) -> Line<'static> {
    let color = if msg.contains("failed") || msg.contains("error") || msg.contains("cannot") {
        DANGER
    } else {
        OK
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(msg.to_string(), Style::default().fg(color)),
    ])
}
