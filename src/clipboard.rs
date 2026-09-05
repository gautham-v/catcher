//! Copying out of the terminal: OSC 52 first (works over ssh, in Ghostty and
//! inside tmux), with the native clipboard (arboard) as a local fallback and
//! `pbcopy`/`pbpaste` behind that on macOS.

use std::io::Write;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

/// What the system clipboard is currently holding, as far as we care.
pub enum Paste {
    /// PNG-encoded bytes, ready to write to disk.
    Image(Vec<u8>),
    Text(String),
    Empty,
}

/// Read the clipboard: an image if there is one, otherwise text. Never panics;
/// an unavailable clipboard reads as [`Paste::Empty`].
pub fn paste() -> Paste {
    let Ok(mut cb) = arboard::Clipboard::new() else {
        return pbpaste();
    };
    if let Ok(img) = cb.get_image() {
        match png_bytes(&img) {
            Some(bytes) => return Paste::Image(bytes),
            None => return Paste::Empty,
        }
    }
    match cb.get_text() {
        Ok(t) if !t.is_empty() => Paste::Text(t),
        _ => pbpaste(),
    }
}

/// Re-encode arboard's raw RGBA buffer as a PNG.
fn png_bytes(img: &arboard::ImageData) -> Option<Vec<u8>> {
    let buf = image::RgbaImage::from_raw(
        u32::try_from(img.width).ok()?,
        u32::try_from(img.height).ok()?,
        img.bytes.to_vec(),
    )?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// Text-only fallback for when arboard can't open the pasteboard.
#[cfg(target_os = "macos")]
fn pbpaste() -> Paste {
    match Command::new("pbpaste").output() {
        Ok(o) if o.status.success() => match String::from_utf8(o.stdout) {
            Ok(t) if !t.is_empty() => Paste::Text(t),
            _ => Paste::Empty,
        },
        _ => Paste::Empty,
    }
}

#[cfg(not(target_os = "macos"))]
fn pbpaste() -> Paste {
    Paste::Empty
}

/// Put `text` on the system clipboard. Best effort — returns false only if
/// every route failed.
pub fn copy(text: &str) -> bool {
    let osc = osc52(text).is_ok();
    let native = native_copy(text) || pbcopy(text);
    osc || native
}

/// The native clipboard through arboard. On Linux the selection lives in the
/// process that set it, so the setter waits on a detached thread until
/// another clipboard owner takes over.
#[cfg(target_os = "linux")]
fn native_copy(text: &str) -> bool {
    use arboard::SetExtLinux;
    let text = text.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            let _ = tx.send(false);
            return;
        };
        let _ = tx.send(true);
        let _ = cb.set().wait().text(text);
    });
    rx.recv().unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn native_copy(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text))
        .is_ok()
}

fn osc52(text: &str) -> std::io::Result<()> {
    let payload = base64(text.as_bytes());
    let seq = if std::env::var_os("TMUX").is_some() {
        // tmux only forwards OSC sequences wrapped in a passthrough
        format!("\x1bPtmux;\x1b\x1b]52;c;{payload}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{payload}\x07")
    };
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())?;
    out.flush()
}

#[cfg(target_os = "macos")]
fn pbcopy(text: &str) -> bool {
    let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn pbcopy(_text: &str) -> bool {
    false
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let idx = [n >> 18, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, k) in idx.iter().enumerate() {
            out.push(if i > chunk.len() {
                '='
            } else {
                ALPHABET[*k as usize] as char
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn encodes_like_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"hello world"), "aGVsbG8gd29ybGQ=");
    }
}
