use std::io::{self, Write};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b = (input[i] as u32) << 16 | (input[i + 1] as u32) << 8 | (input[i + 2] as u32);
        out.push(B64[((b >> 18) & 0x3f) as usize] as char);
        out.push(B64[((b >> 12) & 0x3f) as usize] as char);
        out.push(B64[((b >> 6) & 0x3f) as usize] as char);
        out.push(B64[(b & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let b = (input[i] as u32) << 16;
        out.push(B64[((b >> 18) & 0x3f) as usize] as char);
        out.push(B64[((b >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b = (input[i] as u32) << 16 | (input[i + 1] as u32) << 8;
        out.push(B64[((b >> 18) & 0x3f) as usize] as char);
        out.push(B64[((b >> 12) & 0x3f) as usize] as char);
        out.push(B64[((b >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let encoded = base64_encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{encoded}\x1b\\");
    let sequence = if std::env::var_os("TMUX").is_some() {
        // tmux passthrough: wrap in DCS, escape inner ESC bytes
        let inner = osc.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{inner}\x1b\\")
    } else {
        osc
    };
    let mut stdout = io::stdout().lock();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn test_base64_known_vectors() {
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_change_id() {
        assert_eq!(base64_encode(b"xyzqrsk"), "eHl6cXJzaw==");
    }
}
