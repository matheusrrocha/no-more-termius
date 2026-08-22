//! Cross-platform clipboard copy. We shell out to whatever helper the OS
//! ships (pbcopy / wl-copy / xclip / xsel / clip) instead of linking a GUI
//! toolkit, and fall back to the OSC 52 terminal sequence when no helper is
//! available — which is also what makes `y` work over ssh, where the
//! clipboard we want to reach is the *local* one.

use std::io::Write;
use std::process::{Command, Stdio};

use super::graphics::{base64, in_tmux, wrap_tmux};

/// How the text actually reached the clipboard. OSC 52 is best-effort: the
/// sequence leaves for the terminal but nothing reports back whether it was
/// honoured, so callers say so rather than claiming a plain success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Helper,
    Osc52,
}

/// Terminals cap OSC 52 payloads; kitty's limit (~74994 base64 bytes) is the
/// most generous common one, so anything past it would be silently dropped.
const OSC52_LIMIT: usize = 74_994;

pub fn copy(text: &str) -> Result<Method, String> {
    let mut last_err = None;
    for (program, args) in candidates(&Env::current()) {
        match run(program, args, text) {
            Ok(()) => return Ok(Method::Helper),
            // Helper not installed: keep going down the list.
            Err(Failure::Missing) => {}
            Err(Failure::Failed(e)) => last_err = Some(format!("{program}: {e}")),
        }
    }
    match osc52(text) {
        Ok(()) => Ok(Method::Osc52),
        Err(e) => Err(last_err.unwrap_or(e)),
    }
}

/// The bits of the environment that decide which helper is likely to work.
struct Env {
    wayland: bool,
    x11: bool,
}

impl Env {
    fn current() -> Self {
        Env {
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
            x11: std::env::var_os("DISPLAY").is_some(),
        }
    }
}

/// Helpers to try, in order. Spawning a missing binary is cheap, so the list
/// stays broad; the display server only decides what goes first.
fn candidates(env: &Env) -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }
    if cfg!(windows) {
        return vec![
            ("clip", &[]),
            ("powershell", &["-NoProfile", "-Command", "$input | Set-Clipboard"]),
        ];
    }
    let wayland: &[(&str, &[&str])] = &[("wl-copy", &[])];
    let x11: &[(&str, &[&str])] = &[
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let mut out = Vec::new();
    if env.wayland || !env.x11 {
        out.extend_from_slice(wayland);
        out.extend_from_slice(x11);
    } else {
        out.extend_from_slice(x11);
        out.extend_from_slice(wayland);
    }
    // WSL: no display server, but the Windows clipboard is a pipe away.
    out.push(("clip.exe", &[]));
    out
}

enum Failure {
    Missing,
    Failed(String),
}

fn run(program: &str, args: &[&str], text: &str) -> Result<(), Failure> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Failure::Missing,
            _ => Failure::Failed(e.to_string()),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| Failure::Failed(e.to_string()))?;
        // Closing the pipe is what tells the helper it has the whole text.
        drop(stdin);
    }
    let status = child.wait().map_err(|e| Failure::Failed(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::Failed(format!("exited with {status}")))
    }
}

fn osc52(text: &str) -> Result<(), String> {
    let seq = osc52_sequence(text, in_tmux()).ok_or("text too large for OSC 52")?;
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// `ESC ] 52 ; c ; <base64> BEL`, tmux-wrapped when needed. `None` when the
/// payload exceeds what terminals accept.
fn osc52_sequence(text: &str, tmux: bool) -> Option<String> {
    let b64 = base64(text.as_bytes());
    if b64.len() > OSC52_LIMIT {
        return None;
    }
    Some(wrap_tmux(&format!("\x1b]52;c;{b64}\x07"), tmux))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_helper_goes_first_under_wayland() {
        let env = Env { wayland: true, x11: true };
        assert_eq!(candidates(&env)[0].0, "wl-copy");
    }

    #[test]
    fn x11_helper_goes_first_under_plain_x11() {
        let env = Env { wayland: false, x11: true };
        assert_eq!(candidates(&env)[0].0, "xclip");
    }

    #[test]
    fn headless_still_offers_every_helper() {
        let env = Env { wayland: false, x11: false };
        let names: Vec<&str> = candidates(&env).iter().map(|c| c.0).collect();
        for want in ["wl-copy", "xclip", "xsel", "clip.exe"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
    }

    #[test]
    fn osc52_sequence_is_base64_in_a_c_selection() {
        assert_eq!(
            osc52_sequence("hi", false).unwrap(),
            "\x1b]52;c;aGk=\x07"
        );
    }

    #[test]
    fn osc52_sequence_is_wrapped_for_tmux() {
        let seq = osc52_sequence("hi", true).unwrap();
        assert!(seq.starts_with("\x1bPtmux;\x1b\x1b]52;c;"));
        assert!(seq.ends_with("\x1b\\"));
    }

    #[test]
    fn osc52_refuses_oversized_payloads() {
        let big = "x".repeat(OSC52_LIMIT);
        assert!(osc52_sequence(&big, false).is_none());
    }

    #[test]
    fn missing_helper_is_distinguished_from_failure() {
        let err = run("no-more-termius-not-a-real-binary", &[], "x");
        assert!(matches!(err, Err(Failure::Missing)));
    }
}

