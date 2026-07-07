//! Dual-pane SFTP browser screen: local filesystem on the left, remote SFTP
//! on the right. Overwrites ALWAYS require confirmation.

pub mod pane;
pub mod ui;
pub mod worker;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::Connection;
use pane::{read_local_dir, FsEntry, PaneState, Side};
use worker::{ConnectParams, Direction, SftpEvent, SftpRequest, WorkerHandle};

const STATUS_TTL: Duration = Duration::from_secs(4);

pub enum Phase {
    Connecting,
    Ready,
}

pub enum Modal {
    HostKey { host: String, fingerprint: String },
    Passphrase { key_path: PathBuf, input: String },
    ConfirmOverwrite(PendingTransfer),
    Transfer { direction: Direction, name: String, transferred: u64, total: u64 },
    Fatal(String),
}

#[derive(Clone)]
pub struct PendingTransfer {
    pub direction: Direction,
    pub src: PathBuf,
    pub name: String,
    pub src_size: u64,
    pub dst: PathBuf,
    pub existing_size: u64,
}

enum PendingStat {
    Transfer(PendingTransfer),
    Navigate,
}

pub struct SftpScreen {
    pub conn_name: String,
    pub phase: Phase,
    pub local: PaneState,
    pub remote: PaneState,
    pub active: Side,
    pub show_hidden: bool,
    /// True while `/` filter mode is active on the active pane.
    pub filtering: bool,
    pub modal: Option<Modal>,
    pub exit: bool,
    status: Option<(String, Instant)>,
    pending_stat: Option<PendingStat>,
    worker: WorkerHandle,
}

impl SftpScreen {
    pub fn new(conn: &Connection) -> Self {
        let worker = WorkerHandle::spawn(ConnectParams {
            host: conn.host.clone(),
            port: conn.port,
            user: conn.ssh_user(),
            key_path: conn.identity_file.clone(),
        });

        let mut local = PaneState::new();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let entries = read_local_dir(&home).unwrap_or_default();
        local.set_listing(home, entries, true);

        let mut remote = PaneState::new();
        remote.loading = true;

        Self {
            conn_name: conn.name.clone(),
            phase: Phase::Connecting,
            local,
            remote,
            active: Side::Local,
            show_hidden: true,
            filtering: false,
            modal: None,
            exit: false,
            status: None,
            pending_stat: None,
            worker,
        }
    }

    pub fn status_line(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_TTL)
            .map(|(msg, _)| msg.as_str())
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// `?` must not open help while typing a passphrase or a filter.
    pub fn help_allowed(&self) -> bool {
        !self.filtering && !matches!(self.modal, Some(Modal::Passphrase { .. }))
    }

    pub fn drain_events(&mut self) {
        while let Ok(event) = self.worker.rx.try_recv() {
            self.on_event(event);
        }
    }

    fn on_event(&mut self, event: SftpEvent) {
        match event {
            SftpEvent::HostKeyUnknown { host, fingerprint } => {
                self.modal = Some(Modal::HostKey { host, fingerprint });
            }
            SftpEvent::PassphraseNeeded { key_path } => {
                self.modal = Some(Modal::Passphrase {
                    key_path,
                    input: String::new(),
                });
            }
            SftpEvent::Connected { remote_home } => {
                self.phase = Phase::Ready;
                self.remote.loading = true;
                self.worker.send(SftpRequest::ReadDir(remote_home));
            }
            SftpEvent::DirListing { path, result } => {
                self.remote.loading = false;
                match result {
                    Ok(entries) => {
                        self.remote
                            .set_listing(path, entries, self.show_hidden);
                    }
                    Err(e) => self.set_status(format!("remote: {e}")),
                }
            }
            SftpEvent::StatResult { exists, is_dir, size, .. } => {
                match self.pending_stat.take() {
                    Some(PendingStat::Transfer(mut pending)) => {
                        if !exists {
                            self.start_transfer(pending);
                        } else if is_dir {
                            self.set_status(format!(
                                "cannot overwrite: {} is a directory",
                                pending.dst.display()
                            ));
                        } else {
                            pending.existing_size = size;
                            self.modal = Some(Modal::ConfirmOverwrite(pending));
                        }
                    }
                    Some(PendingStat::Navigate) => {
                        // Symlink resolution: enter if dir, otherwise treat as file transfer.
                        if is_dir {
                            if let Some(entry) = self.active_pane().selected_entry().cloned() {
                                self.navigate(entry.path);
                            }
                        } else if let Some(entry) = self.active_pane().selected_entry().cloned() {
                            self.request_transfer(&entry);
                        }
                    }
                    None => {}
                }
            }
            SftpEvent::Progress { transferred, total } => {
                if let Some(Modal::Transfer {
                    transferred: t,
                    total: tot,
                    ..
                }) = &mut self.modal
                {
                    *t = transferred;
                    *tot = total;
                }
            }
            SftpEvent::TransferDone { direction, name, bytes } => {
                self.modal = None;
                self.set_status(format!(
                    "{} {} ({})",
                    direction.verb(),
                    name,
                    pane::human_size(bytes)
                ));
                // Refresh the destination pane.
                match direction {
                    Direction::Upload => self.refresh_remote(),
                    Direction::Download => self.refresh_local(),
                }
            }
            SftpEvent::TransferFailed { error, cancelled } => {
                self.modal = None;
                if cancelled {
                    self.set_status("transfer cancelled");
                } else {
                    self.set_status(format!("transfer failed: {error}"));
                }
            }
            SftpEvent::Fatal(msg) => {
                self.modal = Some(Modal::Fatal(msg));
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        if self.filtering {
            self.on_filter_key(key);
            return;
        }

        // Normal mode: single-letter actions.
        match key.code {
            KeyCode::Tab => {
                self.active = match self.active {
                    Side::Local => Side::Remote,
                    Side::Remote => Side::Local,
                };
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('j') | KeyCode::Down => self.active_pane_mut().move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.active_pane_mut().move_selection(-1),
            KeyCode::PageUp => self.active_pane_mut().move_selection(-15),
            KeyCode::PageDown => self.active_pane_mut().move_selection(15),
            KeyCode::Char('g') | KeyCode::Home => self.active_pane_mut().select_first(),
            KeyCode::Char('G') | KeyCode::End => self.active_pane_mut().select_last(),
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                let show = self.show_hidden;
                self.local.apply_filter(show);
                self.remote.apply_filter(show);
            }
            KeyCode::Char('r') => {
                self.refresh_local();
                self.refresh_remote();
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.go_parent(),
            KeyCode::Char('l') | KeyCode::Right => {
                // vim-style: `l` only enters directories, never transfers.
                if let Some(entry) = self.active_pane().selected_entry().cloned()
                    && entry.is_dir
                {
                    self.navigate(entry.path);
                }
            }
            KeyCode::Enter => self.on_enter(),
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Esc => {
                if !self.active_pane().filter.is_empty() {
                    let show = self.show_hidden;
                    let pane = self.active_pane_mut();
                    pane.filter.clear();
                    pane.apply_filter(show);
                } else {
                    self.exit = true;
                }
            }
            _ => {}
        }
    }

    /// `/` filter mode: typing edits the active pane's filter; Enter acts on
    /// the selection (open dir / transfer file); Esc keeps the filter.
    fn on_filter_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let show = self.show_hidden;
        match key.code {
            KeyCode::Esc => self.filtering = false,
            KeyCode::Enter => {
                self.filtering = false;
                self.on_enter();
            }
            KeyCode::Up => self.active_pane_mut().move_selection(-1),
            KeyCode::Down => self.active_pane_mut().move_selection(1),
            KeyCode::Char('p') if ctrl => self.active_pane_mut().move_selection(-1),
            KeyCode::Char('n') if ctrl => self.active_pane_mut().move_selection(1),
            KeyCode::Char(c) if !ctrl => {
                let pane = self.active_pane_mut();
                pane.filter.push(c);
                pane.apply_filter(show);
            }
            KeyCode::Backspace => {
                if self.active_pane().filter.is_empty() {
                    self.filtering = false;
                } else {
                    let pane = self.active_pane_mut();
                    pane.filter.pop();
                    pane.apply_filter(show);
                }
            }
            _ => {}
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent) {
        let modal = self.modal.take();
        match modal {
            Some(Modal::HostKey { host, fingerprint }) => match key.code {
                KeyCode::Char('y') => self.worker.send(SftpRequest::AcceptHostKey(true)),
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.worker.send(SftpRequest::AcceptHostKey(false));
                }
                _ => self.modal = Some(Modal::HostKey { host, fingerprint }),
            },
            Some(Modal::Passphrase { key_path, mut input }) => match key.code {
                KeyCode::Enter => {
                    self.worker.send(SftpRequest::Passphrase(Some(input)));
                }
                KeyCode::Esc => {
                    self.worker.send(SftpRequest::Passphrase(None));
                    self.set_status("key auth cancelled — trying agent/default keys");
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.modal = Some(Modal::Passphrase { key_path, input });
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.modal = Some(Modal::Passphrase { key_path, input });
                }
                _ => self.modal = Some(Modal::Passphrase { key_path, input }),
            },
            Some(Modal::ConfirmOverwrite(pending)) => match key.code {
                KeyCode::Char('y') => self.start_transfer(pending),
                // Default is NO: Enter, n and Esc all decline.
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Enter => {
                    self.set_status("overwrite declined");
                }
                _ => self.modal = Some(Modal::ConfirmOverwrite(pending)),
            },
            Some(Modal::Transfer { direction, name, transferred, total }) => {
                if key.code == KeyCode::Esc {
                    self.worker.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                // Modal stays open until the worker reports done/failed.
                self.modal = Some(Modal::Transfer { direction, name, transferred, total });
            }
            Some(Modal::Fatal(_)) => self.exit = true,
            None => {}
        }
    }

    fn on_enter(&mut self) {
        let Some(entry) = self.active_pane().selected_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            self.navigate(entry.path);
        } else if entry.is_symlink && self.active == Side::Remote {
            // Resolve remote symlink first: dir → enter, file → transfer.
            self.pending_stat = Some(PendingStat::Navigate);
            self.worker.send(SftpRequest::StatRemote(entry.path));
        } else {
            self.request_transfer(&entry);
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        match self.active {
            Side::Local => {
                match read_local_dir(&path) {
                    Ok(entries) => {
                        self.local
                            .set_listing(path, entries, self.show_hidden);
                    }
                    Err(e) => self.set_status(format!("local: {e}")),
                }
            }
            Side::Remote => {
                self.remote.loading = true;
                self.worker.send(SftpRequest::ReadDir(path));
            }
        }
    }

    fn go_parent(&mut self) {
        if let Some(parent) = self.active_pane().cwd.parent().map(|p| p.to_path_buf()) {
            self.navigate(parent);
        }
    }

    /// Kick off a transfer of `entry` into the other pane's cwd,
    /// checking the destination for an existing file first.
    fn request_transfer(&mut self, entry: &FsEntry) {
        match self.active {
            Side::Local => {
                let dst = self.remote.cwd.join(&entry.name);
                let pending = PendingTransfer {
                    direction: Direction::Upload,
                    src: entry.path.clone(),
                    name: entry.name.clone(),
                    src_size: entry.size,
                    dst: dst.clone(),
                    existing_size: 0,
                };
                // Freshly stat the destination — never trust the cached listing.
                self.pending_stat = Some(PendingStat::Transfer(pending));
                self.worker.send(SftpRequest::StatRemote(dst));
                self.set_status("checking destination…");
            }
            Side::Remote => {
                let dst = self.local.cwd.join(&entry.name);
                let mut pending = PendingTransfer {
                    direction: Direction::Download,
                    src: entry.path.clone(),
                    name: entry.name.clone(),
                    src_size: entry.size,
                    dst: dst.clone(),
                    existing_size: 0,
                };
                // Local destination check is synchronous.
                match std::fs::symlink_metadata(&dst) {
                    Ok(meta) if meta.is_dir() => {
                        self.set_status(format!(
                            "cannot overwrite: {} is a directory",
                            dst.display()
                        ));
                    }
                    Ok(meta) => {
                        pending.existing_size = meta.len();
                        self.modal = Some(Modal::ConfirmOverwrite(pending));
                    }
                    Err(_) => self.start_transfer(pending),
                }
            }
        }
    }

    fn start_transfer(&mut self, pending: PendingTransfer) {
        self.worker
            .cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.modal = Some(Modal::Transfer {
            direction: pending.direction,
            name: pending.name.clone(),
            transferred: 0,
            total: pending.src_size,
        });
        let request = match pending.direction {
            Direction::Upload => SftpRequest::Upload {
                local: pending.src,
                remote: pending.dst,
            },
            Direction::Download => SftpRequest::Download {
                remote: pending.src,
                local: pending.dst,
            },
        };
        self.worker.send(request);
    }

    fn refresh_local(&mut self) {
        let cwd = self.local.cwd.clone();
        let filter = self.local.filter.clone();
        match read_local_dir(&cwd) {
            Ok(entries) => {
                self.local
                    .set_listing(cwd, entries, self.show_hidden);
                self.local.filter = filter;
                self.local.apply_filter(self.show_hidden);
            }
            Err(e) => self.set_status(format!("local: {e}")),
        }
    }

    fn refresh_remote(&mut self) {
        self.remote.loading = true;
        self.worker
            .send(SftpRequest::ReadDir(self.remote.cwd.clone()));
    }

    fn active_pane(&self) -> &PaneState {
        match self.active {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        }
    }

    fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.active {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }
}
