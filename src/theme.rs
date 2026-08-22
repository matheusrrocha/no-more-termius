//! Central look & feel: rounded borders, one accent color, dim chrome,
//! full-row selection and a key-hint bar — lazygit-inspired.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

pub const ACCENT: Color = Color::Cyan;
/// Secondary text. A fixed grey rather than `DarkGray`, which most themes map
/// to something around #555 — legible only if you already know what it says.
pub const DIM: Color = Color::Indexed(245); // #8a8a8a, 4.5:1 on a dark backdrop
/// Borders and separators: pure chrome, so it sits a step below [`DIM`].
pub const CHROME: Color = Color::Indexed(240); // #585858
pub const FAVORITE: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;
pub const OK: Color = Color::Green;
pub const DIR: Color = Color::Blue;
pub const LINK: Color = Color::Magenta;
/// Plain row text: whatever the terminal uses as its foreground.
pub const TEXT: Color = Color::Reset;

// Selection colors are fixed 256-color slots, not ANSI names, on purpose:
// themes remap the 16 ANSI colors freely and in most of them `Blue` and
// `White` end up close enough that white-on-blue text disappears. Slots
// 16-255 are identical in every terminal, so these contrast ratios hold.
const SEL_BG: Color = Color::Indexed(24); // #005f87, 7.1:1 against SEL_TEXT
const SEL_BG_IDLE: Color = Color::Indexed(238); // #444444, unfocused pane
const SEL_MARKER: Color = Color::Indexed(51); // #00ffff
const SEL_TEXT: Color = Color::Indexed(231); // #ffffff
const SEL_DIM: Color = Color::Indexed(252); // #d0d0d0, 4.6:1 — secondary text
const SEL_FAVORITE: Color = Color::Indexed(227); // #ffff5f
const SEL_DIR: Color = Color::Indexed(153); // #afd7ff
const SEL_LINK: Color = Color::Indexed(219); // #ffafff

/// Rounded panel; focused panels get the accent border + bold title.
pub fn panel(title: impl Into<String>, focused: bool) -> Block<'static> {
    let (border, title_style) = if focused {
        (
            Style::default().fg(ACCENT),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default().fg(CHROME), Style::default().fg(DIM))
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

/// Full-row selection bar. Only the background is set: ratatui paints
/// `highlight_style` over the whole row *after* the item, so setting a
/// foreground here would flatten every span into one color. Rows keep their
/// own spans and run the colors through [`selected_fg`] instead.
pub fn selection(focused: bool) -> Style {
    Style::default().bg(if focused { SEL_BG } else { SEL_BG_IDLE })
}

/// Foreground for a row sitting on the selection bar, given the color the same
/// row uses when it is not selected. Dim greys and the blue/magenta file
/// accents would sink into the bar, so they swap for light variants.
pub fn selected_fg(normal: Color) -> Color {
    match normal {
        DIM => SEL_DIM,
        FAVORITE => SEL_FAVORITE,
        DIR => SEL_DIR,
        LINK => SEL_LINK,
        _ => SEL_TEXT,
    }
}

/// Left marker of the selection bar.
pub fn selection_symbol(focused: bool) -> Line<'static> {
    let color = if focused { SEL_MARKER } else { SEL_DIM };
    Line::styled("▌", Style::default().fg(color))
}

/// Key-hint bar: `key` in accent, description dimmed, dot-separated.
pub fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, desc)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(CHROME)));
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
