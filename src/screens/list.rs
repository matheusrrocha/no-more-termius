//! Connection list: vim-style modal UX. Single letters are actions; `/`
//! focuses the fuzzy search. While searching, typing edits the query and
//! Enter connects to the selection.

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::Action;
use crate::model::{now_epoch, Connection};
use crate::screens::help::centered_rect;

pub enum ListModal {
    ConfirmDelete(usize),
    ImportPrompt(Vec<Connection>),
}

pub struct ListScreen {
    pub query: String,
    /// True while `/` search mode is active (typing edits the query).
    pub searching: bool,
    /// Indices into the store's connection vec, in display order.
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub modal: Option<ListModal>,
    pub status: Option<String>,
    list_state: ListState,
    matcher: SkimMatcherV2,
}

impl ListScreen {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            searching: false,
            filtered: Vec::new(),
            selected: 0,
            modal: None,
            status: None,
            list_state: ListState::default(),
            matcher: SkimMatcherV2::default(),
        }
    }

    pub fn selected_store_idx(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    pub fn refilter(&mut self, connections: &[Connection]) {
        if self.query.is_empty() {
            let mut idx: Vec<usize> = (0..connections.len()).collect();
            idx.sort_by(|&a, &b| {
                let (ca, cb) = (&connections[a], &connections[b]);
                cb.favorite
                    .cmp(&ca.favorite)
                    .then(cb.last_used.unwrap_or(0).cmp(&ca.last_used.unwrap_or(0)))
                    .then(ca.name.to_lowercase().cmp(&cb.name.to_lowercase()))
            });
            self.filtered = idx;
        } else {
            let mut scored: Vec<(i64, usize)> = connections
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    self.matcher
                        .fuzzy_match(&c.search_text(), &self.query)
                        .map(|score| (score, i))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then(connections[b.1].favorite.cmp(&connections[a.1].favorite))
                    .then(connections[a.1].name.cmp(&connections[b.1].name))
            });
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: KeyEvent, connections: &[Connection]) -> Action {
        self.status = None;

        // Modal captures everything while open.
        if let Some(modal) = &self.modal {
            match (modal, key.code) {
                (ListModal::ConfirmDelete(idx), KeyCode::Char('y')) => {
                    let idx = *idx;
                    self.modal = None;
                    return Action::Delete(idx);
                }
                (ListModal::ImportPrompt(_), KeyCode::Char('y')) => {
                    return Action::ImportAccept;
                }
                (_, KeyCode::Char('n') | KeyCode::Esc) => {
                    let declined = matches!(self.modal, Some(ListModal::ImportPrompt(_)));
                    self.modal = None;
                    if declined {
                        return Action::ImportDecline;
                    }
                }
                _ => {}
            }
            return Action::None;
        }

        if self.searching {
            return self.handle_search_key(key, connections);
        }

        // Normal mode: single-letter actions.
        match key.code {
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.filtered.len().saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(idx) = self.selected_store_idx() {
                    return Action::Connect(idx);
                }
            }
            KeyCode::Char('a') => return Action::OpenForm(None),
            KeyCode::Char('e') => {
                if let Some(idx) = self.selected_store_idx() {
                    return Action::OpenForm(Some(idx));
                }
            }
            KeyCode::Char('y') => {
                if let Some(idx) = self.selected_store_idx() {
                    return Action::Duplicate(idx);
                }
            }
            KeyCode::Char('f') => {
                if let Some(idx) = self.selected_store_idx() {
                    return Action::ToggleFavorite(idx);
                }
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.selected_store_idx() {
                    self.modal = Some(ListModal::ConfirmDelete(idx));
                }
            }
            KeyCode::Char('s') => {
                if let Some(idx) = self.selected_store_idx() {
                    return Action::OpenSftp(idx);
                }
            }
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => {
                if self.query.is_empty() {
                    return Action::Quit;
                }
                self.query.clear();
                self.selected = 0;
                self.refilter(connections);
            }
            _ => {}
        }
        Action::None
    }

    /// `/` search mode: typing edits the query; Enter connects to the
    /// selection; Esc goes back to normal mode keeping the filter.
    fn handle_search_key(&mut self, key: KeyEvent, connections: &[Connection]) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.searching = false,
            KeyCode::Enter => {
                if let Some(idx) = self.selected_store_idx() {
                    self.searching = false;
                    return Action::Connect(idx);
                }
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Char('p') if ctrl => self.move_selection(-1),
            KeyCode::Char('n') if ctrl => self.move_selection(1),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.selected = 0;
                self.refilter(connections);
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.selected = 0;
                self.refilter(connections);
            }
            KeyCode::Backspace => {
                if self.query.is_empty() {
                    self.searching = false;
                } else {
                    self.query.pop();
                    self.selected = 0;
                    self.refilter(connections);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn move_selection(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
    }

    pub fn render(&mut self, frame: &mut Frame, connections: &[Connection]) {
        let [search_area, list_area, footer_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // Search box: focused via `/`, highlighted while active.
        let (search_title, search_style) = if self.searching {
            (" Search ", Style::default().fg(Color::Cyan))
        } else {
            (" Search (/) ", Style::default().fg(Color::DarkGray))
        };
        let search = Paragraph::new(self.query.as_str()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(search_style)
                .title(search_title),
        );
        frame.render_widget(search, search_area);
        if self.searching && self.modal.is_none() {
            frame.set_cursor_position(Position::new(
                search_area.x + 1 + self.query.chars().count() as u16,
                search_area.y + 1,
            ));
        }

        // Connection list
        if self.filtered.is_empty() {
            let msg = if connections.is_empty() {
                "No connections yet — press 'a' to add one"
            } else {
                "No matches"
            };
            frame.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(Color::DarkGray))
                    .centered(),
                list_area,
            );
        } else {
            let now = now_epoch();
            let items: Vec<ListItem> = self
                .filtered
                .iter()
                .map(|&i| {
                    let c = &connections[i];
                    let star = if c.favorite {
                        Span::styled("★ ", Style::default().fg(Color::Yellow))
                    } else {
                        Span::raw("  ")
                    };
                    let mut spans = vec![
                        star,
                        Span::styled(
                            c.name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", c.label()),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ];
                    if let Some(ts) = c.last_used {
                        spans.push(Span::styled(
                            format!("  {}", relative_time(now.saturating_sub(ts))),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();

            self.list_state.select(Some(self.selected));
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL))
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(list, list_area, &mut self.list_state);
        }

        // Footer
        let footer = self.status.clone().unwrap_or_else(|| {
            if self.searching {
                "type to filter · Enter connect · Esc done · ↑/↓ move".into()
            } else {
                "Enter connect · / search · s sftp · a add · e edit · y dup · f fav · d del · q quit · ? help".into()
            }
        });
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
            footer_area,
        );

        // Modals
        match &self.modal {
            Some(ListModal::ConfirmDelete(idx)) => {
                let name = connections.get(*idx).map(|c| c.name.as_str()).unwrap_or("?");
                render_confirm(
                    frame,
                    " Delete connection ",
                    &format!("Delete \"{name}\"? This cannot be undone."),
                );
            }
            Some(ListModal::ImportPrompt(conns)) => {
                render_confirm(
                    frame,
                    " First run ",
                    &format!(
                        "Import {} host(s) from ~/.ssh/config?",
                        conns.len()
                    ),
                );
            }
            None => {}
        }
    }
}

fn render_confirm(frame: &mut Frame, title: &str, message: &str) {
    let width = (message.chars().count() as u16 + 6).max(30);
    let area = centered_rect(width, 5, frame.area());
    frame.render_widget(Clear, area);
    let body = vec![
        Line::raw(message),
        Line::default(),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Green).bold()),
            Span::raw(" yes   "),
            Span::styled("n", Style::default().fg(Color::Red).bold()),
            Span::raw(" no"),
        ])
        .centered(),
    ];
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(title),
        ),
        area,
    );
}

fn relative_time(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=604_799 => format!("{}d ago", secs / 86_400),
        _ => format!("{}w ago", secs / 604_800),
    }
}
