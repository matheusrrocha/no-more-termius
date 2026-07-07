use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{Connection, StoreFile};

pub struct Store {
    path: PathBuf,
    pub connections: Vec<Connection>,
}

#[derive(Serialize)]
struct StoreFileRef<'a> {
    connection: &'a [Connection],
}

/// One-time migration from the project's old name.
fn migrate_legacy_dir(home: &std::path::Path) {
    let old = home.join(".config/termius-tui");
    let new = home.join(".config/no-more-termius");
    if old.is_dir() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

impl Store {
    pub fn default_path() -> Result<PathBuf> {
        // dirs::config_dir() is ~/Library/Application Support on macOS; we want ~/.config.
        let home = dirs::home_dir().context("could not determine home directory")?;
        migrate_legacy_dir(&home);
        Ok(home.join(".config/no-more-termius/connections.toml"))
    }

    /// `Ok(None)` means the store file does not exist yet (first run).
    pub fn load(path: PathBuf) -> Result<Option<Store>> {
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let file: StoreFile = toml::from_str(&raw)
            .with_context(|| format!("parsing {} (fix or remove the file)", path.display()))?;
        Ok(Some(Store {
            path,
            connections: file.connections,
        }))
    }

    pub fn new_empty(path: PathBuf) -> Store {
        Store {
            path,
            connections: Vec::new(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("store path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let body = toml::to_string_pretty(&StoreFileRef {
            connection: &self.connections,
        })
        .context("serializing connections")?;
        let tmp = self.path.with_extension("toml.tmp");
        fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Connection> {
        vec![
            Connection {
                name: "web".into(),
                host: "10.0.0.1".into(),
                port: 2222,
                user: Some("deploy".into()),
                identity_file: Some(PathBuf::from("/home/x/.ssh/id_ed25519")),
                favorite: true,
                last_used: Some(1_700_000_000),
            },
            Connection {
                name: "db".into(),
                host: "db.example.com".into(),
                port: 22,
                user: None,
                identity_file: None,
                favorite: false,
                last_used: None,
            },
        ]
    }

    #[test]
    fn toml_round_trip() {
        let conns = sample();
        let body = toml::to_string_pretty(&StoreFileRef { connection: &conns }).unwrap();
        let parsed: StoreFile = toml::from_str(&body).unwrap();
        assert_eq!(parsed.connections, conns);
    }

    #[test]
    fn missing_fields_get_defaults() {
        let raw = "[[connection]]\nname = \"a\"\nhost = \"b\"\n";
        let parsed: StoreFile = toml::from_str(raw).unwrap();
        assert_eq!(parsed.connections[0].port, 22);
        assert!(!parsed.connections[0].favorite);
        assert!(parsed.connections[0].user.is_none());
    }

    #[test]
    fn disk_round_trip() {
        let path = std::env::temp_dir()
            .join(format!("termius-tui-test-{}", std::process::id()))
            .join("connections.toml");
        let mut store = Store::new_empty(path.clone());
        store.connections = sample();
        store.save().unwrap();
        let loaded = Store::load(path.clone()).unwrap().unwrap();
        assert_eq!(loaded.connections, sample());
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn load_missing_is_none() {
        let path = std::env::temp_dir().join("termius-tui-definitely-missing.toml");
        assert!(Store::load(path).unwrap().is_none());
    }
}
