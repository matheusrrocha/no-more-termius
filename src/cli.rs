//! Non-interactive subcommands: read and manage connections from scripts and
//! agents (the Claude Code plugin drives the app through these).
//!
//! Stored passwords never leave the store through here: output goes through
//! `ConnectionView`, which only says whether one is set, and a password comes
//! in on stdin (`--password-stdin`) rather than on the command line.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::model::Connection;
use crate::ssh;
use crate::store::Store;

pub const USAGE: &str = "\
usage: no-more-termius [command]

Without a command the TUI starts. Commands:
  list                         every connection (TOML, passwords redacted)
  show <name>                  one connection, plus the ssh command for it
  add --name <n> --host <h> [options]
  edit <name> [options]        change only the given fields
  rm <name>                    delete a connection
  run <name> [--] <command>    run a remote command (stored password answered)
  help                         this text

Options for add/edit:
  --name <n>  --host <h>  --port <p>  --user <u>  --key <file>
  --sftp-dir <dir>  --favorite  --password-stdin (reads one line from stdin)
Edit only, to clear a field:
  --no-user  --no-key  --no-sftp-dir  --no-password  --no-favorite
";

/// What `list` / `show` print. A separate type so a password cannot reach the
/// output by someone adding a field to `Connection`.
#[derive(Serialize)]
struct ConnectionView<'a> {
    name: &'a str,
    host: &'a str,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sftp_dir: Option<&'a str>,
    favorite: bool,
    has_password: bool,
    /// Equivalent command line. For a password connection it only works when
    /// a human types the password; `run` answers it instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh: Option<String>,
}

impl<'a> ConnectionView<'a> {
    fn new(conn: &'a Connection, with_command: bool) -> Self {
        ConnectionView {
            name: &conn.name,
            host: &conn.host,
            port: conn.port,
            user: conn.user.as_deref(),
            identity_file: conn.identity_file.as_ref().map(|p| p.display().to_string()),
            sftp_dir: conn.sftp_dir.as_deref(),
            favorite: conn.favorite,
            has_password: conn.password.is_some(),
            ssh: with_command.then(|| command_line(conn)),
        }
    }
}

#[derive(Serialize)]
struct ListView<'a> {
    connection: Vec<ConnectionView<'a>>,
}

/// `ssh ...` as one line a shell can run; askpass env is left out on purpose.
fn command_line(conn: &Connection) -> String {
    let cmd = ssh::build_command(conn);
    std::iter::once("ssh".to_string())
        .chain(cmd.get_args().map(|a| local_quote(&a.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(" ")
}

fn local_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Field changes from add/edit flags. `Some(None)` clears an optional field.
#[derive(Debug, Default, PartialEq)]
struct Changes {
    name: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<Option<String>>,
    key: Option<Option<PathBuf>>,
    sftp_dir: Option<Option<String>>,
    favorite: Option<bool>,
    clear_password: bool,
    password_stdin: bool,
}

fn parse_changes(args: &[String]) -> Result<Changes> {
    let mut ch = Changes::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .with_context(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--name" => ch.name = Some(value()?),
            "--host" => ch.host = Some(value()?),
            "--port" => {
                let raw = value()?;
                let port = raw
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .with_context(|| format!("port must be between 1 and 65535, got {raw}"))?;
                ch.port = Some(port);
            }
            "--user" => ch.user = Some(Some(value()?)),
            "--key" => ch.key = Some(Some(expand_tilde(&value()?))),
            "--sftp-dir" => ch.sftp_dir = Some(Some(value()?)),
            "--favorite" => ch.favorite = Some(true),
            "--password-stdin" => ch.password_stdin = true,
            "--no-user" => ch.user = Some(None),
            "--no-key" => ch.key = Some(None),
            "--no-sftp-dir" => ch.sftp_dir = Some(None),
            "--no-favorite" => ch.favorite = Some(false),
            "--no-password" => ch.clear_password = true,
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    if ch.password_stdin && ch.clear_password {
        bail!("--password-stdin and --no-password contradict each other");
    }
    Ok(ch)
}

fn expand_tilde(raw: &str) -> PathBuf {
    match (raw.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Apply `ch` to `conn`, checking what the TUI form checks. `others` are the
/// remaining connections, for the unique-name rule.
fn apply(
    conn: &mut Connection,
    ch: Changes,
    password: Option<String>,
    others: &[&Connection],
) -> Result<()> {
    if let Some(name) = ch.name {
        conn.name = name.trim().to_string();
    }
    if let Some(host) = ch.host {
        conn.host = host.trim().to_string();
    }
    if let Some(port) = ch.port {
        conn.port = port;
    }
    if let Some(user) = ch.user {
        conn.user = user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    }
    if let Some(key) = ch.key {
        if let Some(path) = &key
            && !path.is_file()
        {
            bail!("key file not found: {}", path.display());
        }
        conn.identity_file = key;
    }
    if let Some(dir) = ch.sftp_dir {
        conn.sftp_dir = dir.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    }
    if let Some(favorite) = ch.favorite {
        conn.favorite = favorite;
    }
    if ch.clear_password {
        conn.password = None;
    }
    if password.is_some() {
        conn.password = password;
    }
    if conn.name.is_empty() {
        bail!("name is required");
    }
    if conn.host.is_empty() {
        bail!("host is required");
    }
    if others.iter().any(|c| c.name == conn.name) {
        bail!("a connection named \"{}\" already exists", conn.name);
    }
    Ok(())
}

/// One line from stdin, without the newline. Spaces are kept: they can be
/// part of a password.
fn read_password() -> Result<Option<String>> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("reading password from stdin")?;
    let line = raw.lines().next().unwrap_or_default();
    if line.is_empty() {
        bail!("--password-stdin got an empty password");
    }
    Ok(Some(line.to_string()))
}

fn load_store() -> Result<Store> {
    let path = Store::default_path()?;
    Ok(Store::load(path.clone())?.unwrap_or_else(|| Store::new_empty(path)))
}

fn find(store: &Store, name: &str) -> Result<usize> {
    store
        .connections
        .iter()
        .position(|c| c.name == name)
        .with_context(|| format!("no connection named \"{name}\" (see `no-more-termius list`)"))
}

fn one_name(args: &[String], cmd: &str) -> Result<String> {
    match args {
        [name] => Ok(name.clone()),
        _ => bail!("usage: no-more-termius {cmd} <name>"),
    }
}

/// `Some(exit code)` when `args` named a command, `None` to start the TUI.
pub fn dispatch(args: &[String]) -> Option<Result<i32>> {
    let (cmd, rest) = args.split_first()?;
    let rest = rest.to_vec();
    let result = match cmd.as_str() {
        "list" => list(&rest),
        "show" => show(&rest),
        "add" => add(&rest),
        "edit" => edit(&rest),
        "rm" => rm(&rest),
        "run" => run(&rest),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(0)
        }
        _ => Err(anyhow::anyhow!("unknown command {cmd}\n\n{USAGE}")),
    };
    Some(result)
}

fn list(args: &[String]) -> Result<i32> {
    if !args.is_empty() {
        bail!("usage: no-more-termius list");
    }
    let store = load_store()?;
    let view = ListView {
        connection: store
            .connections
            .iter()
            .map(|c| ConnectionView::new(c, false))
            .collect(),
    };
    print!("{}", toml::to_string_pretty(&view)?);
    Ok(0)
}

fn show(args: &[String]) -> Result<i32> {
    let name = one_name(args, "show")?;
    let store = load_store()?;
    let conn = &store.connections[find(&store, &name)?];
    print!(
        "{}",
        toml::to_string_pretty(&ConnectionView::new(conn, true))?
    );
    Ok(0)
}

fn add(args: &[String]) -> Result<i32> {
    let ch = parse_changes(args)?;
    if ch.name.is_none() || ch.host.is_none() {
        bail!("add needs --name and --host");
    }
    let clears = ch.clear_password
        || ch.user == Some(None)
        || ch.key == Some(None)
        || ch.sftp_dir == Some(None);
    if clears {
        bail!("the --no-* options are for edit");
    }
    let password = if ch.password_stdin {
        read_password()?
    } else {
        None
    };
    let mut store = load_store()?;
    let mut conn = Connection {
        name: String::new(),
        host: String::new(),
        port: 22,
        user: None,
        identity_file: None,
        password: None,
        sftp_dir: None,
        favorite: false,
        last_used: None,
    };
    let others: Vec<&Connection> = store.connections.iter().collect();
    apply(&mut conn, ch, password, &others)?;
    println!("added \"{}\"", conn.name);
    store.connections.push(conn);
    store.save()?;
    Ok(0)
}

fn edit(args: &[String]) -> Result<i32> {
    let (name, flags) = args
        .split_first()
        .context("usage: no-more-termius edit <name> [options]")?;
    let ch = parse_changes(flags)?;
    if ch == Changes::default() {
        bail!("nothing to change\n\n{USAGE}");
    }
    let password = if ch.password_stdin {
        read_password()?
    } else {
        None
    };
    let mut store = load_store()?;
    let idx = find(&store, name)?;
    let mut conn = store.connections[idx].clone();
    let others: Vec<&Connection> = store
        .connections
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, c)| c)
        .collect();
    apply(&mut conn, ch, password, &others)?;
    println!("updated \"{}\"", conn.name);
    store.connections[idx] = conn;
    store.save()?;
    Ok(0)
}

fn rm(args: &[String]) -> Result<i32> {
    let name = one_name(args, "rm")?;
    let mut store = load_store()?;
    let idx = find(&store, &name)?;
    store.connections.remove(idx);
    store.save()?;
    println!("removed \"{name}\"");
    Ok(0)
}

/// Run one remote command and pass its exit code through. ssh joins the
/// words with spaces for the remote shell, as `ssh host cmd...` would.
fn run(args: &[String]) -> Result<i32> {
    let (name, command) = args
        .split_first()
        .context("usage: no-more-termius run <name> [--] <command>")?;
    let command = match command.first().map(String::as_str) {
        Some("--") => &command[1..],
        _ => command,
    };
    if command.is_empty() {
        bail!(
            "run needs a remote command; for an interactive shell use `ssh` from the `show` output in a terminal"
        );
    }
    let store = load_store()?;
    let conn = &store.connections[find(&store, name)?];
    let mut cmd = ssh::build_command(conn);
    cmd.arg("--").args(command.iter().map(OsString::from));
    let status = cmd.status().context("starting ssh")?;
    Ok(status.code().unwrap_or(255))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn conn() -> Connection {
        Connection {
            name: "web".into(),
            host: "10.0.0.1".into(),
            port: 2222,
            user: Some("deploy".into()),
            identity_file: None,
            password: Some("hunter2".into()),
            sftp_dir: Some("/var/www".into()),
            favorite: true,
            last_used: Some(1),
        }
    }

    #[test]
    fn views_never_carry_the_password() {
        let c = conn();
        let one = toml::to_string_pretty(&ConnectionView::new(&c, true)).unwrap();
        let all = toml::to_string_pretty(&ListView {
            connection: vec![ConnectionView::new(&c, false)],
        })
        .unwrap();
        for out in [&one, &all] {
            assert!(!out.contains("hunter2"), "{out}");
            assert!(out.contains("has_password = true"), "{out}");
        }
        assert!(one.contains("ssh = "), "{one}");
        assert!(!all.contains("ssh = "), "{all}");
    }

    #[test]
    fn command_line_is_copy_pastable() {
        let c = Connection {
            password: None,
            ..conn()
        };
        assert!(command_line(&c).ends_with("-p 2222 deploy@10.0.0.1"));
        assert_eq!(local_quote("a b"), "'a b'");
        assert_eq!(local_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn parses_flags() {
        let ch = parse_changes(&s(&[
            "--host",
            "h",
            "--port",
            "2200",
            "--no-user",
            "--favorite",
        ]))
        .unwrap();
        assert_eq!(ch.host.as_deref(), Some("h"));
        assert_eq!(ch.port, Some(2200));
        assert_eq!(ch.user, Some(None));
        assert_eq!(ch.favorite, Some(true));

        assert!(parse_changes(&s(&["--port", "0"])).is_err());
        assert!(parse_changes(&s(&["--port"])).is_err());
        assert!(parse_changes(&s(&["--bogus"])).is_err());
        assert!(parse_changes(&s(&["--password-stdin", "--no-password"])).is_err());
    }

    #[test]
    fn apply_edits_and_validates() {
        let mut c = conn();
        let other = Connection {
            name: "db".into(),
            ..conn()
        };
        let ch = parse_changes(&s(&["--no-password", "--no-sftp-dir", "--user", "root"])).unwrap();
        apply(&mut c, ch, None, &[&other]).unwrap();
        assert_eq!(c.password, None);
        assert_eq!(c.sftp_dir, None);
        assert_eq!(c.user.as_deref(), Some("root"));

        let clash = parse_changes(&s(&["--name", "db"])).unwrap();
        assert!(apply(&mut c, clash, None, &[&other]).is_err());

        let missing = parse_changes(&s(&["--key", "/definitely/not/here"])).unwrap();
        assert!(apply(&mut c, missing, None, &[]).is_err());

        let mut c = conn();
        apply(&mut c, Changes::default(), Some("new pw".into()), &[]).unwrap();
        assert_eq!(c.password.as_deref(), Some("new pw"));
    }

    #[test]
    fn no_args_means_tui() {
        assert!(dispatch(&[]).is_none());
    }
}
