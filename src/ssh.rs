use std::path::Path;
use std::process::Command;

use crate::model::Connection;

/// Set on ssh's environment when a stored password should be answered; holds
/// the connection name and turns this binary into the askpass helper.
pub const ASKPASS_ENV: &str = "NO_MORE_TERMIUS_ASKPASS";

/// Keepalives: without them a link that died while the machine slept leaves
/// ssh waiting forever on a dead socket. 15s × 3 misses gives up in ~45s.
const KEEPALIVE: [&str; 4] = [
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
];

/// Build the interactive `ssh` command for a connection. System ssh is used on
/// purpose: it picks up ~/.ssh/config, the agent, ProxyJump, etc. for free.
pub fn build_command(conn: &Connection) -> Command {
    let mut cmd = Command::new("ssh");
    push_common(&mut cmd, conn);
    cmd
}

/// Interactive shell already `cd`-ed into `dir` on the remote host (the SFTP
/// browser's `!`). `-t` forces a tty because a remote command is given.
pub fn build_shell_command(conn: &Connection, dir: &Path) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.arg("-t");
    push_common(&mut cmd, conn);
    cmd.arg(format!(
        "cd {} && exec $SHELL -l",
        shell_quote(&dir.to_string_lossy())
    ));
    cmd
}

fn push_common(cmd: &mut Command, conn: &Connection) {
    push_askpass(cmd, conn);
    cmd.args(KEEPALIVE);
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
}

/// System ssh cannot take a password as an argument, so point SSH_ASKPASS at
/// ourselves; see `askpass.rs` for the other half.
fn push_askpass(cmd: &mut Command, conn: &Connection) {
    if conn.password.is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return; // ssh just asks on the terminal as usual
    };
    cmd.env("SSH_ASKPASS", exe)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(ASKPASS_ENV, &conn.name);
}

/// Single-quote `s` for the remote POSIX shell; embedded quotes become `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    /// Args after the keepalive options every command starts with.
    fn args(conn: &Connection) -> Vec<String> {
        strip_keepalive(&build_command(conn))
    }

    fn strip_keepalive(cmd: &Command) -> Vec<String> {
        let all: Vec<String> = cmd
            .get_args()
            .map(|a: &OsStr| a.to_string_lossy().into_owned())
            .collect();
        let at = all.iter().position(|a| a == "-o").expect("keepalive options");
        assert_eq!(all[at..at + KEEPALIVE.len()], KEEPALIVE);
        let mut rest = all[..at].to_vec();
        rest.extend_from_slice(&all[at + KEEPALIVE.len()..]);
        rest
    }

    fn env(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    fn base() -> Connection {
        Connection {
            name: "x".into(),
            host: "example.com".into(),
            port: 22,
            user: None,
            identity_file: None,
            password: None,
            sftp_dir: None,
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

    #[test]
    fn shell_command_cds_into_dir() {
        let conn = Connection {
            user: Some("root".into()),
            ..base()
        };
        let args = strip_keepalive(&build_shell_command(&conn, Path::new("/var/www")));
        assert_eq!(
            args,
            vec!["-t", "root@example.com", "cd '/var/www' && exec $SHELL -l"]
        );
    }

    #[test]
    fn keepalive_is_always_on() {
        let args: Vec<String> = build_command(&base())
            .get_args()
            .map(|a: &OsStr| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"ServerAliveInterval=15".to_string()));
        assert_eq!(args.last().unwrap(), "example.com");
    }

    #[test]
    fn password_enables_askpass_without_leaking_it() {
        let plain = build_command(&base());
        assert_eq!(env(&plain, "SSH_ASKPASS"), None);

        let conn = Connection {
            password: Some("hunter2".into()),
            ..base()
        };
        let cmd = build_command(&conn);
        assert_eq!(env(&cmd, "SSH_ASKPASS_REQUIRE").as_deref(), Some("force"));
        assert_eq!(env(&cmd, ASKPASS_ENV).as_deref(), Some("x"));
        assert!(env(&cmd, "SSH_ASKPASS").is_some());
        let leaked = cmd.get_args().any(|a| a.to_string_lossy().contains("hunter2"))
            || cmd
                .get_envs()
                .any(|(_, v)| v.is_some_and(|v| v.to_string_lossy().contains("hunter2")));
        assert!(!leaked);
    }

    #[test]
    fn shell_quote_escapes_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("/plain/dir"), "'/plain/dir'");
    }
}
