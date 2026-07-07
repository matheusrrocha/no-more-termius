use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<PathBuf>,
    /// Remote directory the SFTP browser opens in (`~` = remote home).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sftp_dir: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

fn default_port() -> u16 {
    22
}

impl Connection {
    /// Username to authenticate as: explicit user, or the local login name.
    pub fn ssh_user(&self) -> String {
        self.user
            .clone()
            .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".into()))
    }

    /// `user@host:port` display string.
    pub fn label(&self) -> String {
        let user = self
            .user
            .as_deref()
            .map(|u| format!("{u}@"))
            .unwrap_or_default();
        format!("{user}{}:{}", self.host, self.port)
    }

    /// Text the fuzzy matcher runs against.
    pub fn search_text(&self) -> String {
        format!("{} {}", self.name, self.label())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoreFile {
    #[serde(default, rename = "connection")]
    pub connections: Vec<Connection>,
}

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
