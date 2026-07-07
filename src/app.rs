use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::model::{now_epoch, Connection};
use crate::screens::form::FormScreen;
use crate::screens::help::{self, HelpOverlay};
use crate::screens::list::{ListModal, ListScreen};
use crate::screens::picker::PickerScreen;
use crate::sftp::SftpScreen;
use crate::store::Store;
use crate::{ssh, ssh_config};

#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    List,
    Form,
    Picker,
    Sftp,
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
    OpenPicker(PathBuf),
    PickFile(Option<PathBuf>),
    Connect(usize),
    OpenSftp(usize),
    Delete(usize),
    ToggleFavorite(usize),
    ImportAccept,
    ImportDecline,
}

pub struct App {
    store: Store,
    screen: Screen,
    list: ListScreen,
    form: Option<FormScreen>,
    picker: Option<PickerScreen>,
    sftp: Option<SftpScreen>,
    help: Option<HelpOverlay>,
    should_quit: bool,
}

impl App {
    pub fn new() -> Result<App> {
        let path = Store::default_path()?;
        let mut list = ListScreen::new();

        let store = match Store::load(path.clone())? {
            Some(store) => store,
            None => {
                // First run: offer to import from ~/.ssh/config.
                let store = Store::new_empty(path);
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
            help: None,
            should_quit: false,
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

            terminal.draw(|frame| self.render(frame))?;

            // Short tick while SFTP is active so worker events drain promptly.
            let timeout = if self.screen == Screen::Sftp { 50 } else { 250 };
            if event::poll(Duration::from_millis(timeout))? {
                match event::read()? {
                    Event::Key(key) if key.is_press() => self.on_key(key, terminal)?,
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
                if let Some(sftp) = &mut self.sftp {
                    sftp.on_key(key);
                    if sftp.exit {
                        self.close_sftp();
                    }
                }
                Action::None
            }
        };
        self.apply(action, terminal)
    }

    /// `?` opens contextual help everywhere except in text-entry contexts
    /// (search/filter modes, passphrase prompt), where it types a literal `?`.
    fn help_allowed(&self) -> bool {
        match self.screen {
            Screen::List => !self.list.searching,
            Screen::Form => true,
            Screen::Picker => self.picker.as_ref().is_none_or(|p| !p.filtering),
            Screen::Sftp => self.sftp.as_ref().is_none_or(|s| s.help_allowed()),
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
            Action::OpenPicker(start) => {
                self.picker = Some(PickerScreen::new(start));
                self.screen = Screen::Picker;
            }
            Action::PickFile(path) => {
                if let (Some(form), Some(path)) = (&mut self.form, path) {
                    form.set_key_path(path);
                }
                self.picker = None;
                self.screen = Screen::Form;
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
        }
        Ok(())
    }

    /// Suspend the TUI, run interactive ssh in the real terminal, resume.
    fn connect(&mut self, idx: usize, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(conn) = self.store.connections.get(idx) else {
            return Ok(());
        };
        let mut cmd = ssh::build_command(conn);

        ratatui::restore();
        let status = cmd.status();
        *terminal = ratatui::init();
        terminal.clear().context("re-initializing terminal")?;

        match status {
            Ok(exit) => {
                self.store.connections[idx].last_used = Some(now_epoch());
                self.store.save()?;
                self.list.refilter(&self.store.connections);
                if !exit.success() {
                    self.list.status = Some(format!("ssh exited with {exit}"));
                }
            }
            Err(e) => {
                self.list.status = Some(format!("failed to launch ssh: {e}"));
            }
        }
        Ok(())
    }

    fn close_sftp(&mut self) {
        self.sftp = None; // Drop joins the worker thread.
        self.screen = Screen::List;
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
