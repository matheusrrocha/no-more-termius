//! Portable snapshot of the store: every connection plus the private keys
//! they point at, sealed into one file so a second machine is one passphrase
//! away from the same setup.
//!
//! Keys are addressed by their path on the exporting machine and restored by
//! file name into one folder on the importing machine, because the two rarely
//! agree on where keys live (`~/.ssh` here, `~/chaves` there). Importing never
//! destroys anything on its own: connections that collide are added under a
//! new name, and key files that collide need an explicit yes.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::model::{now_epoch, unique_name, Connection};

pub const BUNDLE_VERSION: u32 = 1;
/// Extension we suggest, so bundles are recognizable in a file listing.
pub const BUNDLE_EXT: &str = "nmt";

const KEY_MODE: u32 = 0o600;
const PUB_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o700;

#[derive(Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub version: u32,
    #[serde(default)]
    pub exported_at: u64,
    #[serde(default, rename = "connection")]
    pub connections: Vec<Connection>,
    #[serde(default, rename = "key")]
    pub keys: Vec<BundledKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundledKey {
    /// Path on the exporting machine — how connections refer to this key.
    pub source: String,
    /// File name to restore under the import folder; unique within a bundle.
    pub name: String,
    /// base64 of the private key file.
    pub data: String,
    /// base64 of the sibling `.pub`, when the exporter had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<String>,
}

/// Key files that could not be read at export time. Their connections still
/// travel, they just arrive without a key.
pub struct ExportReport {
    pub connections: usize,
    pub keys: usize,
    pub unreadable: Vec<String>,
}

/// Collect the connections and every key file they point at.
pub fn build(connections: &[Connection]) -> (Bundle, Vec<String>) {
    let mut keys: Vec<BundledKey> = Vec::new();
    let mut unreadable = Vec::new();

    for conn in connections {
        let Some(path) = &conn.identity_file else {
            continue;
        };
        let source = path.display().to_string();
        if keys.iter().any(|k| k.source == source) {
            continue; // several connections share one key
        }
        match fs::read(path) {
            Ok(data) => {
                let name = unique_file_name(path, &keys);
                keys.push(BundledKey {
                    source,
                    name,
                    data: B64.encode(data),
                    public: fs::read(pub_sibling(path)).ok().map(|b| B64.encode(b)),
                });
            }
            Err(e) => unreadable.push(format!("{source} ({e})")),
        }
    }

    let bundle = Bundle {
        version: BUNDLE_VERSION,
        exported_at: now_epoch(),
        connections: connections.to_vec(),
        keys,
    };
    (bundle, unreadable)
}

/// Build, encrypt and write the bundle. The file holds private keys, so it is
/// created 0600 and only moved into place once it is complete.
pub fn export_to(path: &Path, passphrase: &str, connections: &[Connection]) -> Result<ExportReport> {
    let (bundle, unreadable) = build(connections);
    let report = ExportReport {
        connections: bundle.connections.len(),
        keys: bundle.keys.len(),
        unreadable,
    };

    let plain = toml::to_string_pretty(&bundle).context("serializing the bundle")?;
    let armored = crypto::seal(passphrase, plain.as_bytes())?;

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write_private(path, armored.as_bytes(), KEY_MODE)?;
    Ok(report)
}

/// Read and decrypt a bundle file.
pub fn read_bundle(path: &Path, passphrase: &str) -> Result<Bundle> {
    let armored =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let plain = crypto::open(passphrase, &armored)?;
    let text = String::from_utf8(plain).context("bundle contents are not valid UTF-8")?;
    let bundle: Bundle = toml::from_str(&text).context("parsing the bundle")?;
    if bundle.version > BUNDLE_VERSION {
        anyhow::bail!(
            "bundle was written by a newer version (format {}) — upgrade first",
            bundle.version
        );
    }
    Ok(bundle)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyState {
    /// Nothing at the target path yet.
    New,
    /// Already there, byte for byte — nothing to write.
    Same,
    /// Already there with different contents — needs an explicit overwrite.
    Conflict,
}

pub struct PlannedKey {
    pub target: PathBuf,
    pub state: KeyState,
    data: Vec<u8>,
    public: Option<Vec<u8>>,
}

/// Everything importing this bundle would do, decided before anything is
/// written so the user can be shown the damage first.
pub struct ImportPlan {
    pub keys_dir: PathBuf,
    pub keys: Vec<PlannedKey>,
    /// Connections to append; names de-conflicted and key paths rewritten.
    pub additions: Vec<Connection>,
    /// Bundle entries the store already has, identically configured.
    pub duplicates: Vec<String>,
    /// `(bundle name, name it will be stored under)`.
    pub renamed: Vec<(String, String)>,
}

impl ImportPlan {
    pub fn conflicts(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| k.state == KeyState::Conflict)
            .count()
    }

    /// Keys that will actually be written, given an overwrite decision.
    pub fn writes(&self, overwrite: bool) -> usize {
        self.keys
            .iter()
            .filter(|k| match k.state {
                KeyState::New => true,
                KeyState::Conflict => overwrite,
                KeyState::Same => false,
            })
            .count()
    }
}

/// Work out where every key lands and which connections survive the merge.
/// Pure: touches the filesystem only to compare against what is already there.
pub fn plan(bundle: &Bundle, keys_dir: &Path, existing: &[Connection]) -> Result<ImportPlan> {
    let mut keys = Vec::with_capacity(bundle.keys.len());
    for key in &bundle.keys {
        let data = B64
            .decode(&key.data)
            .with_context(|| format!("decoding key \"{}\"", key.name))?;
        let public = key
            .public
            .as_deref()
            .map(|p| B64.decode(p))
            .transpose()
            .with_context(|| format!("decoding key \"{}.pub\"", key.name))?;
        let target = keys_dir.join(&key.name);
        let state = match fs::read(&target) {
            Ok(current) if current == data => KeyState::Same,
            Ok(_) => KeyState::Conflict,
            Err(_) => KeyState::New,
        };
        keys.push(PlannedKey {
            target,
            state,
            data,
            public,
        });
    }

    // Where each exporting-machine path ends up here.
    let relocated: Vec<(&str, PathBuf)> = bundle
        .keys
        .iter()
        .zip(&keys)
        .map(|(k, planned)| (k.source.as_str(), planned.target.clone()))
        .collect();

    let mut additions: Vec<Connection> = Vec::new();
    let mut duplicates = Vec::new();
    let mut renamed = Vec::new();

    for conn in &bundle.connections {
        let mut conn = conn.clone();
        if let Some(old) = &conn.identity_file {
            let old = old.display().to_string();
            if let Some((_, target)) = relocated.iter().find(|(source, _)| *source == old) {
                conn.identity_file = Some(target.clone());
            }
        }

        let clash = existing.iter().find(|c| c.name == conn.name);
        if let Some(clash) = clash {
            if same_endpoint(clash, &conn) {
                duplicates.push(conn.name.clone());
                continue;
            }
            let taken = |candidate: &str| {
                existing.iter().any(|c| c.name == candidate)
                    || additions.iter().any(|c| c.name == candidate)
            };
            let fresh = unique_name(&conn.name, "imported", taken);
            renamed.push((conn.name.clone(), fresh.clone()));
            conn.name = fresh;
        } else if additions.iter().any(|c| c.name == conn.name) {
            continue; // duplicate inside the bundle itself
        }
        additions.push(conn);
    }

    Ok(ImportPlan {
        keys_dir: keys_dir.to_path_buf(),
        keys,
        additions,
        duplicates,
        renamed,
    })
}

/// Write the planned key files. Returns how many were written.
pub fn apply_keys(plan: &ImportPlan, overwrite: bool) -> Result<usize> {
    if plan.keys.is_empty() {
        return Ok(0);
    }
    // Only tighten a folder we are creating: an existing ~/.ssh keeps whatever
    // mode its owner chose, which is not ours to change behind their back.
    let existed = plan.keys_dir.is_dir();
    fs::create_dir_all(&plan.keys_dir)
        .with_context(|| format!("creating {}", plan.keys_dir.display()))?;
    if !existed {
        fs::set_permissions(&plan.keys_dir, fs::Permissions::from_mode(DIR_MODE))
            .with_context(|| format!("securing {}", plan.keys_dir.display()))?;
    }

    let mut written = 0;
    for key in &plan.keys {
        match key.state {
            KeyState::Same => continue,
            KeyState::Conflict if !overwrite => continue,
            _ => {}
        }
        write_private(&key.target, &key.data, KEY_MODE)?;
        if let Some(public) = &key.public {
            write_private(&pub_sibling(&key.target), public, PUB_MODE)?;
        }
        written += 1;
    }
    Ok(written)
}

/// Create with restrictive permissions from the first byte and only then move
/// it into place: no window where the key is readable or half-written.
fn write_private(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let tmp = tmp_sibling(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(file);
    // `.mode()` only applies when the file is new; force it either way.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
        .with_context(|| format!("securing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn pub_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".pub");
    PathBuf::from(name)
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".nmt-tmp-{}", std::process::id()));
    PathBuf::from(name)
}

/// Two entries describe the same server if they agree on everything that
/// affects the ssh invocation.
fn same_endpoint(a: &Connection, b: &Connection) -> bool {
    a.host == b.host && a.port == b.port && a.user == b.user
}

/// `id_rsa`, then `id_rsa-2`, … — `~/.ssh/id_rsa` and `~/chaves/id_rsa` are
/// different keys and must not land on top of each other.
fn unique_file_name(path: &Path, taken: &[BundledKey]) -> String {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "key".into());
    if !taken.iter().any(|k| k.name == base) {
        return base;
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (base.clone(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !taken.iter().any(|k| &k.name == candidate))
        .unwrap()
}

/// Expand a leading `~` in a path the user typed.
pub fn expand_tilde(input: &str) -> PathBuf {
    let input = input.trim();
    let Some(home) = dirs::home_dir() else {
        return PathBuf::from(input);
    };
    match input.strip_prefix('~') {
        Some("") => home,
        Some(rest) => match rest.strip_prefix('/') {
            Some(rest) => home.join(rest),
            None => PathBuf::from(input), // ~user/… is not ours to resolve
        },
        None => PathBuf::from(input),
    }
}

/// Where imported keys go unless the user says otherwise.
pub fn default_keys_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".ssh"))
        .unwrap_or_else(|| PathBuf::from(".ssh"))
}

/// Where an export lands unless the user says otherwise.
pub fn default_export_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!("no-more-termius.{BUNDLE_EXT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "nmt-bundle-{tag}-{}-{:p}",
                std::process::id(),
                &tag
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn write(&self, name: &str, body: &str) -> PathBuf {
            let path = self.join(name);
            fs::write(&path, body).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn conn(name: &str, host: &str, key: Option<PathBuf>) -> Connection {
        Connection {
            name: name.into(),
            host: host.into(),
            port: 22,
            user: Some("root".into()),
            identity_file: key,
            password: None,
            sftp_dir: None,
            favorite: false,
            last_used: None,
        }
    }

    #[test]
    fn export_import_round_trip_restores_keys_and_repoints_connections() {
        let src = TempDir::new("src");
        let key = src.write("id_ed25519", "PRIVATE");
        src.write("id_ed25519.pub", "PUBLIC");
        let out = TempDir::new("out");
        let dest = TempDir::new("dest");

        let conns = vec![conn("web", "10.0.0.1", Some(key.clone()))];
        let file = out.join("bundle.nmt");
        let report = export_to(&file, "passphrase123", &conns).unwrap();
        assert_eq!((report.connections, report.keys), (1, 1));
        assert!(report.unreadable.is_empty());

        let bundle = read_bundle(&file, "passphrase123").unwrap();
        let plan = plan(&bundle, &dest.0, &[]).unwrap();
        assert_eq!(plan.writes(false), 1);
        assert_eq!(plan.conflicts(), 0);
        assert_eq!(apply_keys(&plan, false).unwrap(), 1);

        let restored = dest.join("id_ed25519");
        assert_eq!(fs::read_to_string(&restored).unwrap(), "PRIVATE");
        assert_eq!(
            fs::read_to_string(dest.join("id_ed25519.pub")).unwrap(),
            "PUBLIC"
        );
        // The connection must point at the copy on THIS machine.
        assert_eq!(plan.additions[0].identity_file, Some(restored));
    }

    #[test]
    fn restored_private_keys_are_not_group_or_world_readable() {
        let src = TempDir::new("perm-src");
        let key = src.write("id_rsa", "PRIVATE");
        let out = TempDir::new("perm-out");
        let dest = TempDir::new("perm-dest");

        let file = out.join("b.nmt");
        export_to(&file, "passphrase123", &[conn("a", "h", Some(key))]).unwrap();
        let bundle = read_bundle(&file, "passphrase123").unwrap();
        apply_keys(&plan(&bundle, &dest.0, &[]).unwrap(), false).unwrap();

        let mode = fs::metadata(dest.join("id_rsa")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, KEY_MODE, "restored key is {:o}", mode & 0o777);
        // The bundle itself carries the same keys and needs the same care.
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, KEY_MODE, "bundle is {:o}", mode & 0o777);
    }

    /// A folder we create is ours to lock down; one that was already there
    /// belongs to the user.
    #[test]
    fn a_created_key_folder_is_private_and_an_existing_one_is_left_as_is() {
        let src = TempDir::new("dirmode-src");
        let key = src.write("id_rsa", "K");
        let (bundle, _) = build(&[conn("a", "h", Some(key))]);

        let parent = TempDir::new("dirmode");
        let fresh = parent.join("new-keys");
        apply_keys(&plan(&bundle, &fresh, &[]).unwrap(), false).unwrap();
        let mode = fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, DIR_MODE, "created folder is {:o}", mode & 0o777);

        let loose = parent.join("loose-keys");
        fs::create_dir(&loose).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
        apply_keys(&plan(&bundle, &loose, &[]).unwrap(), false).unwrap();
        let mode = fs::metadata(&loose).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "existing folder was changed");
    }

    #[test]
    fn same_file_name_from_different_folders_stays_distinct() {
        let a = TempDir::new("dedup-a");
        let b = TempDir::new("dedup-b");
        let dest = TempDir::new("dedup-dest");
        let ka = a.write("id_rsa", "KEY-A");
        let kb = b.write("id_rsa", "KEY-B");

        let (bundle, _) = build(&[conn("a", "ha", Some(ka)), conn("b", "hb", Some(kb))]);
        assert_eq!(bundle.keys[0].name, "id_rsa");
        assert_eq!(bundle.keys[1].name, "id_rsa-2");

        let plan = plan(&bundle, &dest.0, &[]).unwrap();
        apply_keys(&plan, false).unwrap();
        assert_eq!(fs::read_to_string(dest.join("id_rsa")).unwrap(), "KEY-A");
        assert_eq!(fs::read_to_string(dest.join("id_rsa-2")).unwrap(), "KEY-B");
        assert_eq!(
            plan.additions[1].identity_file,
            Some(dest.join("id_rsa-2"))
        );
    }

    #[test]
    fn dotted_key_names_keep_their_extension() {
        let a = TempDir::new("ext-a");
        let b = TempDir::new("ext-b");
        let ka = a.write("prod.pem", "A");
        let kb = b.write("prod.pem", "B");
        let (bundle, _) = build(&[conn("a", "ha", Some(ka)), conn("b", "hb", Some(kb))]);
        assert_eq!(bundle.keys[1].name, "prod-2.pem");
    }

    #[test]
    fn a_key_shared_by_two_connections_travels_once() {
        let src = TempDir::new("shared");
        let key = src.write("id_rsa", "K");
        let (bundle, _) = build(&[
            conn("a", "ha", Some(key.clone())),
            conn("b", "hb", Some(key)),
        ]);
        assert_eq!(bundle.keys.len(), 1);
    }

    #[test]
    fn unreadable_keys_are_reported_and_their_connections_still_travel() {
        let src = TempDir::new("missing");
        let gone = src.join("not-there");
        let out = TempDir::new("missing-out");
        let file = out.join("b.nmt");

        let report = export_to(&file, "passphrase123", &[conn("a", "h", Some(gone.clone()))]).unwrap();
        assert_eq!(report.keys, 0);
        assert_eq!(report.unreadable.len(), 1);

        let bundle = read_bundle(&file, "passphrase123").unwrap();
        // Nothing to repoint it at, so it keeps the path it was exported with.
        assert_eq!(bundle.connections[0].identity_file, Some(gone));
    }

    #[test]
    fn an_existing_different_key_at_the_target_needs_an_explicit_overwrite() {
        let src = TempDir::new("clash-src");
        let dest = TempDir::new("clash-dest");
        let key = src.write("id_rsa", "NEW");
        dest.write("id_rsa", "OLD");

        let (bundle, _) = build(&[conn("a", "h", Some(key))]);
        let plan = plan(&bundle, &dest.0, &[]).unwrap();
        assert_eq!(plan.conflicts(), 1);

        assert_eq!(apply_keys(&plan, false).unwrap(), 0);
        assert_eq!(fs::read_to_string(dest.join("id_rsa")).unwrap(), "OLD");

        assert_eq!(apply_keys(&plan, true).unwrap(), 1);
        assert_eq!(fs::read_to_string(dest.join("id_rsa")).unwrap(), "NEW");
    }

    #[test]
    fn an_identical_key_already_in_place_is_left_alone() {
        let src = TempDir::new("same-src");
        let dest = TempDir::new("same-dest");
        let key = src.write("id_rsa", "SAME");
        dest.write("id_rsa", "SAME");

        let (bundle, _) = build(&[conn("a", "h", Some(key))]);
        let plan = plan(&bundle, &dest.0, &[]).unwrap();
        assert_eq!(plan.keys[0].state, KeyState::Same);
        assert_eq!(plan.writes(true), 0);
    }

    #[test]
    fn merging_skips_identical_entries_and_renames_clashing_ones() {
        let dest = TempDir::new("merge");
        let existing = vec![
            conn("web", "10.0.0.1", None),  // same endpoint → skip
            conn("db", "10.0.0.2", None),   // same name, other host → rename
        ];
        let (bundle, _) = build(&[
            conn("web", "10.0.0.1", None),
            conn("db", "10.9.9.9", None),
            conn("cache", "10.0.0.3", None),
        ]);

        let plan = plan(&bundle, &dest.0, &existing).unwrap();
        assert_eq!(plan.duplicates, vec!["web"]);
        assert_eq!(plan.renamed, vec![("db".to_string(), "db (imported)".to_string())]);
        let names: Vec<&str> = plan.additions.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["db (imported)", "cache"]);
    }

    #[test]
    fn importing_the_same_bundle_twice_adds_nothing_the_second_time() {
        let dest = TempDir::new("idempotent");
        let (bundle, _) = build(&[conn("web", "10.0.0.1", None)]);

        let first = plan(&bundle, &dest.0, &[]).unwrap();
        assert_eq!(first.additions.len(), 1);
        let second = plan(&bundle, &dest.0, &first.additions).unwrap();
        assert!(second.additions.is_empty());
        assert_eq!(second.duplicates, vec!["web"]);
    }

    #[test]
    fn a_wrong_passphrase_does_not_open_the_bundle() {
        let out = TempDir::new("pw");
        let file = out.join("b.nmt");
        export_to(&file, "passphrase123", &[conn("a", "h", None)]).unwrap();
        assert!(read_bundle(&file, "passphrase124").is_err());
    }

    #[test]
    fn nothing_readable_leaks_into_the_exported_file() {
        let src = TempDir::new("leak-src");
        let key = src.write("id_rsa", "SUPER SECRET KEY MATERIAL");
        let out = TempDir::new("leak-out");
        let file = out.join("b.nmt");
        export_to(&file, "passphrase123", &[conn("prod-db", "10.0.0.9", Some(key))]).unwrap();

        let raw = fs::read_to_string(&file).unwrap();
        for secret in ["SUPER SECRET", "prod-db", "10.0.0.9", "id_rsa"] {
            assert!(!raw.contains(secret), "{secret:?} is readable in the export");
        }
    }

    #[test]
    fn tilde_expands_only_for_the_current_user() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/.ssh"), home.join(".ssh"));
        assert_eq!(expand_tilde("  ~/keys  "), home.join("keys"));
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("~other/x"), PathBuf::from("~other/x"));
    }
}
