//! Rendering for the dual-pane SFTP screen.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::screens::help::centered_rect;
use super::pane::{human_size, PaneState, Side};
use super::worker::Direction;
use super::{Modal, Phase, SftpScreen};

pub fn render(frame: &mut Frame, screen: &mut SftpScreen) {
    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(main_area);

    let filtering = screen.filtering;
    render_pane(
        frame,
        left,
        &mut screen.local,
        screen.active == Side::Local,
        filtering && screen.active == Side::Local,
        "Local",
        true,
    );
    let remote_ready = matches!(screen.phase, Phase::Ready);
    render_pane(
        frame,
        right,
        &mut screen.remote,
        screen.active == Side::Remote,
        filtering && screen.active == Side::Remote,
        &format!("Remote ({})", screen.conn_name),
        remote_ready,
    );

    let footer = screen.status_line().map(String::from).unwrap_or_else(|| {
        if screen.filtering {
            "type to filter · Enter open/transfer · Esc done".into()
        } else {
            "Tab pane · Enter open/transfer · h/l dirs · / filter · . hidden · r refresh · q back · ? help"
                .into()
        }
    });
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );

    render_modal(frame, screen);
}

#[allow(clippy::too_many_arguments)]
fn render_pane(
    frame: &mut Frame,
    area: Rect,
    pane: &mut PaneState,
    active: bool,
    filtering: bool,
    label: &str,
    ready: bool,
) {
    let border_style = if active {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut title = format!(" {label}: {} ", pane.cwd.display());
    if pane.loading {
        title.push_str("⟳ ");
    }
    if filtering || !pane.filter.is_empty() {
        title.push_str(&format!("[/{}] ", pane.filter));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if !ready {
        frame.render_widget(
            Paragraph::new("connecting…")
                .style(Style::default().fg(Color::DarkGray))
                .centered()
                .block(block),
            area,
        );
        return;
    }

    let name_width = area.width.saturating_sub(14) as usize;
    let items: Vec<ListItem> = pane
        .filtered
        .iter()
        .map(|&i| {
            let e = &pane.entries[i];
            let mut name = e.name.clone();
            if e.is_dir {
                name.push('/');
            } else if e.is_symlink {
                name.push('@');
            }
            let size = if e.is_dir {
                String::new()
            } else {
                human_size(e.size)
            };
            let padded = format!("{name:<name_width$.name_width$} {size:>9}");
            let style = if e.is_dir {
                Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
            } else if e.is_symlink {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default()
            };
            ListItem::new(Line::styled(padded, style))
        })
        .collect();

    pane.list_state.select(if pane.filtered.is_empty() {
        None
    } else {
        Some(pane.selected)
    });
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶");
    frame.render_stateful_widget(list, area, &mut pane.list_state);
}

fn render_modal(frame: &mut Frame, screen: &SftpScreen) {
    let Some(modal) = &screen.modal else { return };
    match modal {
        Modal::HostKey { host, fingerprint } => {
            let lines = vec![
                Line::raw(format!("The authenticity of host \"{host}\" can't be established.")),
                Line::raw(format!("Key fingerprint: {fingerprint}")),
                Line::default(),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(Color::Green).bold()),
                    Span::raw(" trust & save   "),
                    Span::styled("n", Style::default().fg(Color::Red).bold()),
                    Span::raw(" abort"),
                ])
                .centered(),
            ];
            render_box(frame, " Unknown host key ", lines, Color::Yellow);
        }
        Modal::Passphrase { key_path, input } => {
            let masked = "•".repeat(input.chars().count());
            let lines = vec![
                Line::raw(format!("Passphrase for {}:", key_path.display())),
                Line::styled(masked, Style::default().fg(Color::Cyan)),
                Line::default(),
                Line::styled(
                    "Enter submit · Esc skip key auth",
                    Style::default().fg(Color::DarkGray),
                )
                .centered(),
            ];
            render_box(frame, " Passphrase required ", lines, Color::Cyan);
        }
        Modal::ConfirmOverwrite(pending) => {
            let target_dir = pending
                .dst
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let lines = vec![
                Line::from(vec![
                    Span::styled(
                        pending.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" already exists in "),
                    Span::raw(target_dir),
                ]),
                Line::raw(format!(
                    "existing: {}   new: {}",
                    human_size(pending.existing_size),
                    human_size(pending.src_size)
                )),
                Line::default(),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(Color::Red).bold()),
                    Span::raw(" overwrite   "),
                    Span::styled("n", Style::default().fg(Color::Green).bold()),
                    Span::raw(" cancel (default)"),
                ])
                .centered(),
            ];
            render_box(frame, " Overwrite file? ", lines, Color::Red);
        }
        Modal::Transfer { direction, name, transferred, total } => {
            let verb = match direction {
                Direction::Upload => "Uploading",
                Direction::Download => "Downloading",
            };
            let area = centered_rect(60, 6, frame.area());
            frame.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" {verb} {name} "));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [gauge_area, label_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
                    .areas(Rect {
                        x: inner.x + 1,
                        y: inner.y + 1,
                        width: inner.width.saturating_sub(2),
                        height: inner.height.saturating_sub(1),
                    });
            let ratio = if *total == 0 {
                0.0
            } else {
                (*transferred as f64 / *total as f64).clamp(0.0, 1.0)
            };
            frame.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .ratio(ratio),
                gauge_area,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "{} / {}  —  Esc cancels",
                    human_size(*transferred),
                    human_size(*total)
                ))
                .style(Style::default().fg(Color::DarkGray))
                .centered(),
                label_area,
            );
        }
        Modal::Fatal(msg) => {
            let lines = vec![
                Line::raw(msg.clone()),
                Line::default(),
                Line::styled(
                    "press any key to return to the connection list",
                    Style::default().fg(Color::DarkGray),
                )
                .centered(),
            ];
            render_box(frame, " SFTP error ", lines, Color::Red);
        }
    }
}

fn render_box(frame: &mut Frame, title: &str, lines: Vec<Line>, color: Color) {
    let width = lines
        .iter()
        .map(|l| l.width() as u16)
        .max()
        .unwrap_or(40)
        .max(title.len() as u16)
        + 4;
    let height = lines.len() as u16 + 2;
    let area = centered_rect(width.min(frame.area().width), height, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color))
                .title(title.to_string()),
        ),
        area,
    );
}
