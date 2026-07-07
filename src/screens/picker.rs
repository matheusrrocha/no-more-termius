//! Filesystem browser used to pick a private key file.
//! Hidden files are SHOWN by default: keys live in ~/.ssh.

use std::fs;
use std::path::{Path, PathBuf};

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::Action;
use crate::theme;

struct PickEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

pub struct PickerScreen {
    cwd: PathBuf,
    entries: Vec<PickEntry>,
    filtered: Vec<usize>,
    selected: usize,
    filter: String,
    /// True while `/` filter mode is active (typing edits the filter).
    pub filtering: bool,
    show_hidden: bool,
    error: Option<String>,
    list_state: ListState,
    matcher: SkimMatcherV2,
}

impl PickerScreen {
    pub fn new(start: PathBuf) -> Self {
        let mut picker = Self {
            cwd: start,
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            filter: String::new(),
            filtering: false,
            show_hidden: true,
            error: None,
            list_state: ListState::default(),
            matcher: SkimMatcherV2::default(),
        };
        picker.load_dir(picker.cwd.clone());
        picker
    }

    fn load_dir(&mut self, dir: PathBuf) {
        match read_entries(&dir) {
            Ok(entries) => {
                self.cwd = dir;
                self.entries = entries;
                self.filter.clear();
                self.error = None;
                self.apply_filter();
            }
            Err(e) => self.error = Some(format!("{}: {e}", dir.display())),
        }
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered = (0..self.entries.len())
                .filter(|&i| self.show_hidden || !self.entries[i].name.starts_with('.'))
                .collect();
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| self.show_hidden || !e.name.starts_with('.'))
                .filter_map(|(i, e)| {
                    self.matcher
                        .fuzzy_match(&e.name, &self.filter)
                        .map(|s| (s, i))
                })
                .collect();
            scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = 0;
    }

    fn go_parent(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            let from = self.cwd.clone();
            self.load_dir(parent.to_path_buf());
            // Keep the dir we came from selected, for quick backtracking.
            if let Some(pos) = self
                .filtered
                .iter()
                .position(|&i| self.entries[i].path == from)
            {
                self.selected = pos;
            }
        }
    }

    fn selected_entry(&self) -> Option<&PickEntry> {
        self.filtered.get(self.selected).map(|&i| &self.entries[i])
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if self.filtering {
            return self.handle_filter_key(key);
        }

        // Normal mode: single-letter actions.
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::PickFile(None),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.apply_filter();
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.filtered.len().saturating_sub(1);
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.go_parent(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                return self.open_selected();
            }
            _ => {}
        }
        Action::None
    }

    /// `/` filter mode: typing edits the filter; Enter opens/picks the
    /// selection; Esc goes back to normal mode keeping the filter.
    fn handle_filter_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.filtering = false,
            KeyCode::Enter => {
                self.filtering = false;
                return self.open_selected();
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Char('p') if ctrl => self.move_selection(-1),
            KeyCode::Char('n') if ctrl => self.move_selection(1),
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.apply_filter();
            }
            KeyCode::Backspace => {
                if self.filter.is_empty() {
                    self.filtering = false;
                } else {
                    self.filter.pop();
                    self.apply_filter();
                }
            }
            _ => {}
        }
        Action::None
    }

    fn open_selected(&mut self) -> Action {
        if let Some(entry) = self.selected_entry() {
            if entry.is_dir {
                self.load_dir(entry.path.clone());
            } else {
                return Action::PickFile(Some(entry.path.clone()));
            }
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

    pub fn render(&mut self, frame: &mut Frame) {
        let [list_area, footer_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

        let mut title = format!(" Pick key file — {} ", self.cwd.display());
        if self.filtering || !self.filter.is_empty() {
            title.push_str(&format!("[/{}] ", self.filter));
        }

        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .map(|&i| {
                let e = &self.entries[i];
                if e.is_dir {
                    ListItem::new(Line::from(Span::styled(
                        format!(" {}/", e.name),
                        Style::default()
                            .fg(theme::DIR)
                            .add_modifier(Modifier::BOLD),
                    )))
                } else {
                    ListItem::new(format!(" {}", e.name))
                }
            })
            .collect();

        self.list_state.select(if self.filtered.is_empty() {
            None
        } else {
            Some(self.selected)
        });
        let list = List::new(items)
            .block(theme::panel(title.trim(), true))
            .highlight_style(theme::selection())
            .highlight_symbol(theme::SELECTION_SYMBOL);
        frame.render_stateful_widget(list, list_area, &mut self.list_state);

        let footer = match &self.error {
            Some(err) => Line::styled(format!(" {err}"), Style::default().fg(theme::DANGER)),
            None if self.filtering => theme::hints(&[
                ("type", "filter"),
                ("Enter", "open/pick"),
                ("Esc", "done"),
            ]),
            None => theme::hints(&[
                ("Enter/l", "open/pick"),
                ("h", "parent"),
                ("/", "filter"),
                (".", "hidden"),
                ("j/k", "move"),
                ("Esc", "cancel"),
                ("?", "help"),
            ]),
        };
        frame.render_widget(Paragraph::new(footer), footer_area);
    }
}

fn read_entries(dir: &Path) -> std::io::Result<Vec<PickEntry>> {
    let mut entries: Vec<PickEntry> = fs::read_dir(dir)?
        .filter_map(|res| res.ok())
        .map(|e| {
            let path = e.path();
            PickEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: path.is_dir(), // follows symlinks: symlinked dirs enterable
                path,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}
