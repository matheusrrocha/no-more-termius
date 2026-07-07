//! Rendering for the dual-pane SFTP screen.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::screens::help::centered_rect;
use crate::theme;
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

    let footer = match screen.status_line() {
        Some(status) => theme::status_line(status),
        None if screen.filtering => theme::hints(&[
            ("type", "filter"),
            ("Enter", "open/transfer"),
            ("Esc", "done"),
        ]),
        None => theme::hints(&[
            ("Tab", "pane"),
            ("Enter", "open/transfer"),
            ("/", "filter"),
            ("Space", "preview"),
            ("R", "rename"),
            ("D", "delete"),
            ("q", "back"),
            ("?", "help"),
        ]),
    };
    frame.render_widget(Paragraph::new(footer), footer_area);

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
    let mut title = format!("{label}: {}", pane.cwd.display());
    if pane.loading {
        title.push_str(" ⟳");
    }
    if filtering || !pane.filter.is_empty() {
        title.push_str(&format!(" [/{}]", pane.filter));
    }
    let block = theme::panel(title, active);

    if !ready {
        frame.render_widget(
            Paragraph::new("connecting…")
                .style(Style::default().fg(theme::DIM))
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
            let padded = format!(" {name:<name_width$.name_width$} {size:>9}");
            let style = if e.is_dir {
                Style::default().fg(theme::DIR).add_modifier(Modifier::BOLD)
            } else if e.is_symlink {
                Style::default().fg(theme::LINK)
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
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECTION_SYMBOL);
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
            let block = theme::modal(format!("{verb} {name}"), theme::ACCENT);
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
                    .gauge_style(Style::default().fg(theme::ACCENT))
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
        Modal::Rename { entry, input } => {
            let lines = vec![
                Line::raw(format!("Rename {}:", entry.name)),
                Line::from(vec![
                    Span::styled(input.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled("▏", Style::default().fg(Color::Cyan)),
                ]),
                Line::default(),
                Line::styled(
                    "Enter rename · Esc cancel",
                    Style::default().fg(Color::DarkGray),
                )
                .centered(),
            ];
            render_box(frame, " Rename ", lines, Color::Cyan);
        }
        Modal::ConfirmDelete(entry) => {
            let what = if entry.is_dir {
                "directory (must be empty)"
            } else if entry.is_symlink {
                "symlink"
            } else {
                "file"
            };
            let lines = vec![
                Line::from(vec![
                    Span::raw("Delete "),
                    Span::styled(
                        entry.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" ({what})?")),
                ]),
                Line::raw("This cannot be undone."),
                Line::default(),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(Color::Red).bold()),
                    Span::raw(" delete   "),
                    Span::styled("n", Style::default().fg(Color::Green).bold()),
                    Span::raw(" cancel (default)"),
                ])
                .centered(),
            ];
            render_box(frame, " Delete? ", lines, Color::Red);
        }
        Modal::ImagePreview { name, lines, graphics } => {
            let area = match graphics {
                Some(img) => image_modal_area(img, frame.area()),
                None => {
                    let img_w = lines.iter().map(|l| l.width()).max().unwrap_or(1) as u16;
                    centered_rect(img_w + 2, lines.len() as u16 + 2, frame.area())
                }
            };
            frame.render_widget(Clear, area);
            // With a graphics protocol the interior stays blank: the app
            // emits the actual pixels over it after the frame is drawn.
            frame.render_widget(
                Paragraph::new(lines.clone())
                    .block(theme::modal(format!("{name} — q close"), theme::ACCENT)),
                area,
            );
        }
        Modal::Preview { name, lines, scroll } => {
            let frame_area = frame.area();
            // 80% of the screen, centered.
            let area = centered_rect(
                (frame_area.width as u32 * 4 / 5) as u16,
                (frame_area.height as u32 * 4 / 5) as u16,
                frame_area,
            );
            frame.render_widget(Clear, area);
            let total = lines.len();
            let visible: Vec<Line> = lines
                .iter()
                .skip(*scroll)
                .take(area.height.saturating_sub(2) as usize)
                .map(|l| Line::raw(l.clone()))
                .collect();
            let title = format!(
                "{name} — {}/{total} · j/k scroll · y copy · q close",
                (*scroll + 1).min(total)
            );
            frame.render_widget(
                Paragraph::new(visible).block(theme::modal(title, theme::ACCENT)),
                area,
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

/// Modal rect for a protocol-drawn image (interior = the image cell box).
pub fn image_modal_area(img: &super::graphics::EncodedImage, frame_area: Rect) -> Rect {
    centered_rect(img.cols + 2, img.rows + 2, frame_area)
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
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(theme::modal(title.trim(), color)),
        area,
    );
}
