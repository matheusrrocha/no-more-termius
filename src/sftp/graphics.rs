//! Terminal graphics protocols for pixel-perfect image previews, written by
//! hand (no external tools): Kitty graphics protocol (kitty, Ghostty,
//! WezTerm) and iTerm2 inline images (iTerm2). Sequences are wrapped in a
//! tmux passthrough envelope when running inside tmux (requires
//! `allow-passthrough on`). Callers fall back to half/quadrant-block
//! rendering when no protocol is detected.

use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    Iterm2,
}

/// Detect a supported graphics protocol from the environment. Works inside
/// tmux because the relevant variables leak into the tmux server's
/// environment when it is started from the terminal app.
pub fn detect() -> Option<Protocol> {
    let has = |key: &str| std::env::var_os(key).is_some();
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();

    if has("KITTY_WINDOW_ID")
        || has("GHOSTTY_RESOURCES_DIR")
        || term.contains("kitty")
        || term.contains("ghostty")
        || program == "WezTerm"
        || program == "ghostty"
    {
        Some(Protocol::Kitty)
    } else if has("ITERM_SESSION_ID") || program == "iTerm.app" {
        Some(Protocol::Iterm2)
    } else {
        None
    }
}

/// A PNG-encoded image plus the cell box it should occupy on screen.
pub struct EncodedImage {
    b64: String,
    pub cols: u16,
    pub rows: u16,
}

/// Encode for transmission and compute the cell box that fits `box_cols` ×
/// `box_rows` while preserving aspect ratio (terminal cells are ~1:2).
pub fn prepare(img: &image::DynamicImage, box_cols: u16, box_rows: u16) -> Option<EncodedImage> {
    let (cols, rows) = fit_cells(img.width(), img.height(), box_cols, box_rows);

    // Bound the payload: previews never need more than ~1500px.
    let capped;
    let to_encode = if img.width() > 1500 || img.height() > 1500 {
        capped = img.thumbnail(1500, 1500);
        &capped
    } else {
        img
    };
    let mut png = Vec::new();
    to_encode
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(EncodedImage {
        b64: base64(&png),
        cols,
        rows,
    })
}

/// Fit `w`×`h` pixels into a cell box, assuming a cell is 1 unit wide and
/// 2 units tall.
fn fit_cells(w: u32, h: u32, box_cols: u16, box_rows: u16) -> (u16, u16) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (box_cols.max(1) as f32 / w).min(box_rows.max(1) as f32 * 2.0 / h);
    let cols = ((w * scale).floor() as u16).clamp(1, box_cols.max(1));
    let rows = ((h * scale / 2.0).ceil() as u16).clamp(1, box_rows.max(1));
    (cols, rows)
}

/// Draw the image with its top-left corner at cell (x, y), 0-based.
pub fn emit(
    protocol: Protocol,
    img: &EncodedImage,
    x: u16,
    y: u16,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let tmux = in_tmux();
    // Cursor positioning is a normal sequence: tmux translates it itself.
    write!(out, "\x1b[{};{}H", y + 1, x + 1)?;
    match protocol {
        Protocol::Kitty => {
            for seq in kitty_sequences(img) {
                out.write_all(wrap_tmux(&seq, tmux).as_bytes())?;
            }
        }
        Protocol::Iterm2 => {
            out.write_all(wrap_tmux(&iterm2_sequence(img), tmux).as_bytes())?;
        }
    }
    out.flush()
}

/// Remove previously drawn images (Kitty keeps them on a separate layer;
/// iTerm2 images live in cells and are overwritten by normal redraws).
pub fn clear(protocol: Protocol, out: &mut impl Write) -> std::io::Result<()> {
    if protocol == Protocol::Kitty {
        out.write_all(wrap_tmux("\x1b_Ga=d,d=A,q=2\x1b\\", in_tmux()).as_bytes())?;
        out.flush()?;
    }
    Ok(())
}

/// Kitty payloads are chunked at 4096 bytes; control keys go on the first
/// chunk only. `q=2` suppresses replies, which would corrupt tmux's input.
fn kitty_sequences(img: &EncodedImage) -> Vec<String> {
    const CHUNK: usize = 4096;
    let data = img.b64.as_bytes();
    let mut sequences = Vec::with_capacity(data.len().div_ceil(CHUNK));
    let mut start = 0;
    while start < data.len() {
        let end = (start + CHUNK).min(data.len());
        let chunk = &img.b64[start..end];
        let more = if end < data.len() { 1 } else { 0 };
        if start == 0 {
            sequences.push(format!(
                "\x1b_Ga=T,f=100,q=2,c={},r={},m={};{}\x1b\\",
                img.cols, img.rows, more, chunk
            ));
        } else {
            sequences.push(format!("\x1b_Gm={};{}\x1b\\", more, chunk));
        }
        start = end;
    }
    sequences
}

fn iterm2_sequence(img: &EncodedImage) -> String {
    format!(
        "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=1:{}\x07",
        img.cols, img.rows, img.b64
    )
}

fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// tmux passthrough envelope: DCS-wrapped with every ESC doubled.
fn wrap_tmux(seq: &str, tmux: bool) -> String {
    if !tmux {
        return seq.to_string();
    }
    format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
}

/// Standard base64 with padding.
fn base64(data: &[u8]) -> String {
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
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_correctly() {
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"hell"), "aGVsbA==");
        assert_eq!(base64(b"hel"), "aGVs");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn tmux_wrapping_doubles_escapes() {
        assert_eq!(wrap_tmux("\x1b_Gx\x1b\\", false), "\x1b_Gx\x1b\\");
        assert_eq!(
            wrap_tmux("\x1b_Gx\x1b\\", true),
            "\x1bPtmux;\x1b\x1b_Gx\x1b\x1b\\\x1b\\"
        );
    }

    #[test]
    fn kitty_single_chunk_has_all_controls() {
        let img = EncodedImage {
            b64: "QUJD".into(),
            cols: 40,
            rows: 20,
        };
        let seqs = kitty_sequences(&img);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], "\x1b_Ga=T,f=100,q=2,c=40,r=20,m=0;QUJD\x1b\\");
    }

    #[test]
    fn kitty_chunking_splits_payload() {
        let img = EncodedImage {
            b64: "A".repeat(5000),
            cols: 10,
            rows: 5,
        };
        let seqs = kitty_sequences(&img);
        assert_eq!(seqs.len(), 2);
        assert!(seqs[0].starts_with("\x1b_Ga=T,f=100,q=2,c=10,r=5,m=1;"));
        assert!(seqs[1].starts_with("\x1b_Gm=0;"));
    }

    #[test]
    fn iterm2_format() {
        let img = EncodedImage {
            b64: "QUJD".into(),
            cols: 12,
            rows: 6,
        };
        assert_eq!(
            iterm2_sequence(&img),
            "\x1b]1337;File=inline=1;width=12;height=6;preserveAspectRatio=1:QUJD\x07"
        );
    }

    #[test]
    fn cell_fitting_preserves_aspect() {
        // 200x100 px in an 80x24 box: width-limited → 80 cols, 20 rows.
        assert_eq!(fit_cells(200, 100, 80, 24), (80, 20));
        // Tall image: height-limited.
        let (cols, rows) = fit_cells(100, 400, 80, 24);
        assert_eq!(rows, 24);
        assert!(cols <= 12);
        // Never zero.
        assert_eq!(fit_cells(1, 1, 1, 1), (1, 1));
    }
}
