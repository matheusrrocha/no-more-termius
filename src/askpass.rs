//! `SSH_ASKPASS` helper: the other half of stored passwords.
//!
//! When a connection has a password, ssh is started with this binary as its
//! askpass program (see `ssh::push_askpass`). ssh then runs us with the prompt
//! as the only argument and reads the answer from our stdout. The first
//! password prompt gets the stored password, read from the store by
//! connection name so it never sits in an environment variable. Everything
//! else — host-key questions, key passphrases, 2FA codes, and a retry after a
//! wrong stored password — is asked on the terminal, the way ssh would.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::store::Store;

/// Older markers belong to an ssh long gone whose pid got reused.
const MARKER_TTL: Duration = Duration::from_secs(300);

/// Answer one prompt; returns the process exit code (non-zero = cancelled).
pub fn run(conn_name: &str) -> i32 {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    if is_password_prompt(&prompt)
        && first_password_prompt()
        && let Some(password) = stored_password(conn_name)
    {
        println!("{password}");
        return 0;
    }
    match ask_on_tty(&prompt) {
        Ok(Some(answer)) => {
            println!("{answer}");
            0
        }
        Ok(None) => 0,
        Err(_) => 1,
    }
}

fn is_password_prompt(prompt: &str) -> bool {
    let p = prompt.to_lowercase();
    p.contains("password") && !p.contains("new password")
}

fn stored_password(conn_name: &str) -> Option<String> {
    let store = Store::load(Store::default_path().ok()?).ok()??;
    store
        .connections
        .into_iter()
        .find(|c| c.name == conn_name)?
        .password
}

/// True only for the first password prompt of the ssh that spawned us, so a
/// stale stored password costs one failed attempt instead of all of them.
fn first_password_prompt() -> bool {
    let marker = marker_path();
    if let Ok(meta) = std::fs::metadata(&marker) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < MARKER_TTL);
        if fresh {
            return false;
        }
        let _ = std::fs::remove_file(&marker);
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .is_ok()
}

fn marker_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_default();
    std::env::temp_dir().join(format!(
        "no-more-termius-askpass-{user}-{}",
        std::os::unix::process::parent_id()
    ))
}

/// Ask on the controlling terminal. `Ok(None)` is a notice that needs no
/// answer (ssh sets SSH_ASKPASS_PROMPT=none for those).
fn ask_on_tty(prompt: &str) -> std::io::Result<Option<String>> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    write!(tty, "{prompt}")?;
    tty.flush()?;
    let kind = std::env::var("SSH_ASKPASS_PROMPT").unwrap_or_default();
    if kind == "none" {
        writeln!(tty)?;
        return Ok(None);
    }
    let echo = kind == "confirm" || prompt.contains("yes/no");
    if !echo {
        set_echo(&tty, false);
    }
    let mut line = String::new();
    let read = BufReader::new(tty.try_clone()?).read_line(&mut line);
    if !echo {
        set_echo(&tty, true);
        writeln!(tty)?; // the Enter was not echoed
    }
    read?;
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

fn set_echo(tty: &File, on: bool) {
    if let Ok(stdin) = tty.try_clone() {
        let _ = Command::new("stty")
            .arg(if on { "echo" } else { "-echo" })
            .stdin(Stdio::from(stdin))
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_password_prompts() {
        assert!(is_password_prompt("root@10.0.0.1's password: "));
        assert!(is_password_prompt("(deploy@web) Password: "));
        assert!(is_password_prompt("Password:"));
        assert!(!is_password_prompt("Enter passphrase for key '/k/id': "));
        assert!(!is_password_prompt(
            "Are you sure you want to continue connecting (yes/no/[fingerprint])? "
        ));
        assert!(!is_password_prompt("Verification code: "));
        assert!(!is_password_prompt("New password: "));
    }
}
