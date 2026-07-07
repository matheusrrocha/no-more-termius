use std::process::Command;

use crate::model::Connection;

/// Build the interactive `ssh` command for a connection. System ssh is used on
/// purpose: it picks up ~/.ssh/config, the agent, ProxyJump, etc. for free.
pub fn build_command(conn: &Connection) -> Command {
    let mut cmd = Command::new("ssh");
    if conn.port != 22 {
        cmd.arg("-p").arg(conn.port.to_string());
    }
    if let Some(key) = &conn.identity_file {
        cmd.arg("-i").arg(key);
    }
    match &conn.user {
        Some(user) => cmd.arg(format!("{user}@{}", conn.host)),
        None => cmd.arg(&conn.host),
    };
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn args(conn: &Connection) -> Vec<String> {
        build_command(conn)
            .get_args()
            .map(|a: &OsStr| a.to_string_lossy().into_owned())
            .collect()
    }

    fn base() -> Connection {
        Connection {
            name: "x".into(),
            host: "example.com".into(),
            port: 22,
            user: None,
            identity_file: None,
            favorite: false,
            last_used: None,
        }
    }

    #[test]
    fn bare_host() {
        assert_eq!(args(&base()), vec!["example.com"]);
    }

    #[test]
    fn full_options() {
        let conn = Connection {
            port: 2222,
            user: Some("root".into()),
            identity_file: Some(PathBuf::from("/k/id")),
            ..base()
        };
        assert_eq!(
            args(&conn),
            vec!["-p", "2222", "-i", "/k/id", "root@example.com"]
        );
    }

    #[test]
    fn user_only() {
        let conn = Connection {
            user: Some("admin".into()),
            ..base()
        };
        assert_eq!(args(&conn), vec!["admin@example.com"]);
    }
}
