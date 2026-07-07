//! Background thread that owns the SSH/SFTP session. The UI talks to it via
//! typed mpsc messages; mid-connect prompts (host key, passphrase) block the
//! worker on the request channel while the UI shows a modal.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ssh2::{CheckResult, HashType, KnownHostFileKind, RenameFlags, Session, Sftp};

use super::pane::{sort_entries, FsEntry};

const CHUNK: usize = 64 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_path: Option<PathBuf>,
}

pub enum SftpRequest {
    AcceptHostKey(bool),
    Passphrase(Option<String>),
    ReadDir(PathBuf),
    StatRemote(PathBuf),
    Upload { local: PathBuf, remote: PathBuf },
    Download { remote: PathBuf, local: PathBuf },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

impl Direction {
    pub fn verb(&self) -> &'static str {
        match self {
            Direction::Upload => "Uploaded",
            Direction::Download => "Downloaded",
        }
    }
}

pub enum SftpEvent {
    HostKeyUnknown { host: String, fingerprint: String },
    PassphraseNeeded { key_path: PathBuf },
    Connected { remote_home: PathBuf },
    DirListing { path: PathBuf, result: Result<Vec<FsEntry>, String> },
    StatResult { exists: bool, is_dir: bool, size: u64 },
    Progress { transferred: u64, total: u64 },
    TransferDone { direction: Direction, name: String, bytes: u64 },
    TransferFailed { error: String, cancelled: bool },
    Fatal(String),
}

pub struct WorkerHandle {
    pub tx: Sender<SftpRequest>,
    pub rx: Receiver<SftpEvent>,
    pub cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn spawn(params: ConnectParams) -> WorkerHandle {
        let (req_tx, req_rx) = mpsc::channel::<SftpRequest>();
        let (ev_tx, ev_rx) = mpsc::channel::<SftpEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = cancel.clone();
        let join = std::thread::spawn(move || run_worker(params, req_rx, ev_tx, cancel2));
        WorkerHandle {
            tx: req_tx,
            rx: ev_rx,
            cancel,
            join: Some(join),
        }
    }

    pub fn send(&self, req: SftpRequest) {
        let _ = self.tx.send(req);
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(SftpRequest::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_worker(
    params: ConnectParams,
    req_rx: Receiver<SftpRequest>,
    tx: Sender<SftpEvent>,
    cancel: Arc<AtomicBool>,
) {
    let (sess, sftp) = match connect(&params, &req_rx, &tx) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = tx.send(SftpEvent::Fatal(format!("{e:#}")));
            return;
        }
    };

    let home = sftp
        .realpath(Path::new("."))
        .unwrap_or_else(|_| PathBuf::from("/"));
    let _ = tx.send(SftpEvent::Connected { remote_home: home });

    loop {
        match req_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(SftpRequest::ReadDir(path)) => {
                let result = read_remote_dir(&sftp, &path);
                let _ = tx.send(SftpEvent::DirListing { path, result });
            }
            Ok(SftpRequest::StatRemote(path)) => {
                let event = match sftp.stat(&path) {
                    Ok(st) => SftpEvent::StatResult {
                        exists: true,
                        is_dir: st.is_dir(),
                        size: st.size.unwrap_or(0),
                    },
                    Err(_) => SftpEvent::StatResult {
                        exists: false,
                        is_dir: false,
                        size: 0,
                    },
                };
                let _ = tx.send(event);
            }
            Ok(SftpRequest::Upload { local, remote }) => {
                transfer(&sess, &sftp, Direction::Upload, &local, &remote, &tx, &cancel);
            }
            Ok(SftpRequest::Download { remote, local }) => {
                transfer(&sess, &sftp, Direction::Download, &remote, &local, &tx, &cancel);
            }
            Ok(SftpRequest::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(_) => {} // stray prompt replies
            Err(RecvTimeoutError::Timeout) => {
                if sess.keepalive_send().is_err() {
                    let _ = tx.send(SftpEvent::Fatal("connection lost".into()));
                    return;
                }
            }
        }
    }
}

fn connect(
    params: &ConnectParams,
    req_rx: &Receiver<SftpRequest>,
    tx: &Sender<SftpEvent>,
) -> Result<(Session, Sftp)> {
    let addr = format!("{}:{}", params.host, params.port)
        .to_socket_addrs()
        .with_context(|| format!("resolving {}", params.host))?
        .next()
        .with_context(|| format!("no address for {}", params.host))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .with_context(|| format!("connecting to {addr}"))?;

    let mut sess = Session::new().context("creating ssh session")?;
    sess.set_tcp_stream(stream);
    sess.set_timeout(30_000);
    sess.handshake().context("ssh handshake")?;

    check_host_key(&sess, params, req_rx, tx)?;
    authenticate(&sess, params, req_rx, tx)?;

    sess.set_keepalive(true, 15);
    let sftp = sess.sftp().context("opening sftp subsystem")?;
    Ok((sess, sftp))
}

fn check_host_key(
    sess: &Session,
    params: &ConnectParams,
    req_rx: &Receiver<SftpRequest>,
    tx: &Sender<SftpEvent>,
) -> Result<()> {
    let (key, key_type) = sess.host_key().context("server sent no host key")?;
    let mut known_hosts = sess.known_hosts().context("initializing known_hosts")?;
    let path = dirs::home_dir()
        .context("no home directory")?
        .join(".ssh/known_hosts");
    let _ = known_hosts.read_file(&path, KnownHostFileKind::OpenSSH); // may not exist yet

    match known_hosts.check_port(&params.host, params.port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::Mismatch => {
            bail!(
                "HOST KEY MISMATCH for {} ({}) — possible man-in-the-middle. \
                 Fix ~/.ssh/known_hosts manually if the host legitimately changed.",
                params.host,
                fingerprint(sess)
            )
        }
        CheckResult::NotFound | CheckResult::Failure => {
            tx.send(SftpEvent::HostKeyUnknown {
                host: params.host.clone(),
                fingerprint: fingerprint(sess),
            })
            .ok();
            loop {
                match req_rx.recv() {
                    Ok(SftpRequest::AcceptHostKey(true)) => {
                        let host_entry = if params.port == 22 {
                            params.host.clone()
                        } else {
                            format!("[{}]:{}", params.host, params.port)
                        };
                        known_hosts
                            .add(&host_entry, key, "added by termius-tui", key_type.into())
                            .context("adding host key")?;
                        known_hosts
                            .write_file(&path, KnownHostFileKind::OpenSSH)
                            .context("writing known_hosts")?;
                        return Ok(());
                    }
                    Ok(SftpRequest::AcceptHostKey(false)) => bail!("host key rejected"),
                    Ok(SftpRequest::Shutdown) | Err(_) => bail!("cancelled"),
                    Ok(_) => {}
                }
            }
        }
    }
}

fn fingerprint(sess: &Session) -> String {
    sess.host_key_hash(HashType::Sha256)
        .map(|h| format!("SHA256:{}", base64_nopad(h)))
        .unwrap_or_else(|| "unknown fingerprint".into())
}

fn authenticate(
    sess: &Session,
    params: &ConnectParams,
    req_rx: &Receiver<SftpRequest>,
    tx: &Sender<SftpEvent>,
) -> Result<()> {
    let user = &params.user;

    // 1. Explicit key from the connection, with passphrase prompt on failure.
    if let Some(key) = &params.key_path {
        if sess.userauth_pubkey_file(user, None, key, None).is_ok() {
            return Ok(());
        }
        let mut attempts = 0;
        while attempts < 3 && !sess.authenticated() {
            tx.send(SftpEvent::PassphraseNeeded {
                key_path: key.clone(),
            })
            .ok();
            match req_rx.recv() {
                Ok(SftpRequest::Passphrase(Some(phrase))) => {
                    attempts += 1;
                    if sess
                        .userauth_pubkey_file(user, None, key, Some(&phrase))
                        .is_ok()
                    {
                        return Ok(());
                    }
                }
                Ok(SftpRequest::Passphrase(None)) => break, // cancelled: fall through
                Ok(SftpRequest::Shutdown) | Err(_) => bail!("cancelled"),
                Ok(_) => {}
            }
        }
    }

    // 2. ssh-agent.
    if !sess.authenticated() {
        let _ = sess.userauth_agent(user);
    }

    // 3. Default keys, tried silently.
    if !sess.authenticated()
        && let Some(home) = dirs::home_dir()
    {
        for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
            let path = home.join(".ssh").join(name);
            if path.is_file() {
                let _ = sess.userauth_pubkey_file(user, None, &path, None);
                if sess.authenticated() {
                    break;
                }
            }
        }
    }

    if sess.authenticated() {
        Ok(())
    } else {
        bail!(
            "authentication failed for {user}@{} (tried key file, ssh-agent, default keys)",
            params.host
        )
    }
}

fn read_remote_dir(sftp: &Sftp, path: &Path) -> Result<Vec<FsEntry>, String> {
    let raw = sftp.readdir(path).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(raw.len());
    for (entry_path, stat) in raw {
        let Some(name) = entry_path.file_name() else {
            continue;
        };
        let perm = stat.perm.unwrap_or(0);
        out.push(FsEntry {
            name: name.to_string_lossy().into_owned(),
            path: entry_path,
            is_dir: stat.is_dir(),
            is_symlink: perm & 0o170000 == 0o120000,
            size: stat.size.unwrap_or(0),
        });
    }
    sort_entries(&mut out);
    Ok(out)
}

/// Copy `src` → `dst` in either direction. Writes to `<dst>.part` first and
/// renames on success, so an existing destination is never corrupted by a
/// cancelled or failed transfer.
fn transfer(
    sess: &Session,
    sftp: &Sftp,
    direction: Direction,
    src: &Path,
    dst: &Path,
    tx: &Sender<SftpEvent>,
    cancel: &AtomicBool,
) {
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let result = match direction {
        Direction::Upload => copy_streams(
            std::fs::File::open(src).map_err(|e| e.to_string()),
            src_size_local(src),
            |part| sftp.create(part).map_err(|e| e.to_string()),
            dst,
            |part| finalize_remote(sftp, part, dst),
            |part| sftp.unlink(part).map_err(|e| e.to_string()),
            tx,
            cancel,
        ),
        Direction::Download => copy_streams(
            sftp.open(src).map_err(|e| e.to_string()),
            sftp.stat(src).ok().and_then(|s| s.size).unwrap_or(0),
            |part| std::fs::File::create(part).map_err(|e| e.to_string()),
            dst,
            |part| std::fs::rename(part, dst).map_err(|e| e.to_string()),
            |part| std::fs::remove_file(part).map_err(|e| e.to_string()),
            tx,
            cancel,
        ),
    };

    match result {
        Ok(bytes) => {
            let _ = tx.send(SftpEvent::TransferDone {
                direction,
                name,
                bytes,
            });
        }
        Err(TransferError::Cancelled) => {
            let _ = tx.send(SftpEvent::TransferFailed {
                error: "cancelled".into(),
                cancelled: true,
            });
        }
        Err(TransferError::Failed(error)) => {
            let _ = tx.send(SftpEvent::TransferFailed {
                error,
                cancelled: false,
            });
            // A failed write may mean the connection died; probe it.
            if sess.keepalive_send().is_err() {
                let _ = tx.send(SftpEvent::Fatal("connection lost".into()));
            }
        }
    }
}

fn src_size_local(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// OpenSSH's SFTPv3 rename fails when the destination exists and ignores the
/// OVERWRITE flag, so fall back to unlink-then-rename.
fn finalize_remote(sftp: &Sftp, part: &Path, dst: &Path) -> Result<(), String> {
    if sftp
        .rename(part, dst, Some(RenameFlags::OVERWRITE))
        .is_ok()
    {
        return Ok(());
    }
    let _ = sftp.unlink(dst);
    sftp.rename(part, dst, None).map_err(|e| e.to_string())
}

enum TransferError {
    Cancelled,
    Failed(String),
}

#[allow(clippy::too_many_arguments)]
fn copy_streams<R, W>(
    src: Result<R, String>,
    total: u64,
    open_part: impl FnOnce(&Path) -> Result<W, String>,
    dst: &Path,
    finalize: impl FnOnce(&Path) -> Result<(), String>,
    remove_part: impl FnOnce(&Path) -> Result<(), String>,
    tx: &Sender<SftpEvent>,
    cancel: &AtomicBool,
) -> Result<u64, TransferError>
where
    R: Read,
    W: Write,
{
    let mut src = src.map_err(TransferError::Failed)?;
    let part = part_path(dst);
    let mut dst_file = open_part(&part).map_err(TransferError::Failed)?;

    let _ = tx.send(SftpEvent::Progress {
        transferred: 0,
        total,
    });

    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
    let mut last_progress = Instant::now();
    let copy_result: Result<(), TransferError> = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(TransferError::Cancelled);
        }
        let n = match src.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(TransferError::Failed(format!("read: {e}"))),
        };
        if let Err(e) = dst_file.write_all(&buf[..n]) {
            break Err(TransferError::Failed(format!("write: {e}")));
        }
        sent += n as u64;
        if last_progress.elapsed() >= PROGRESS_EVERY {
            let _ = tx.send(SftpEvent::Progress {
                transferred: sent,
                total,
            });
            last_progress = Instant::now();
        }
    };
    drop(dst_file);

    match copy_result {
        Ok(()) => {
            finalize(&part).map_err(TransferError::Failed)?;
            Ok(sent)
        }
        Err(e) => {
            let _ = remove_part(&part);
            Err(e)
        }
    }
}

fn part_path(dst: &Path) -> PathBuf {
    let mut name = dst.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dst.with_file_name(name)
}

fn base64_nopad(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_openssh_style() {
        // echo -n "hello world" | openssl dgst -binary -sha256 | base64 (no pad)
        assert_eq!(base64_nopad(b"hello"), "aGVsbG8");
        assert_eq!(base64_nopad(b"hell"), "aGVsbA");
        assert_eq!(base64_nopad(b"hel"), "aGVs");
        assert_eq!(base64_nopad(b""), "");
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/a/b/file.txt")),
            PathBuf::from("/a/b/file.txt.part")
        );
    }
}
