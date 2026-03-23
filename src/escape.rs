//! C-style escape sequence processing.

/// Parse C-style escape sequences in a string (`\n`, `\r`, `\t`, `\b`, `\f`,
/// `\\`, `\e`, `\xHH`, `\uXXXX`).
pub fn unescape(s: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('r') => result.push(b'\r'),
                Some('t') => result.push(b'\t'),
                Some('b') => result.push(0x08),
                Some('f') => result.push(0x0C),
                Some('\\') => result.push(b'\\'),
                Some('0') => result.push(0),
                Some('a') => result.push(0x07),
                Some('e') | Some('E') => result.push(0x1B),
                Some('u') => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(&next) = chars.as_str().as_bytes().first() {
                            if (next as char).is_ascii_hexdigit() {
                                hex.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            let mut buf = [0u8; 4];
                            result.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                    } else {
                        result.push(b'\\');
                        result.push(b'u');
                        result.extend(hex.as_bytes());
                    }
                }
                Some('x') => {
                    let mut hex = String::new();
                    for _ in 0..2 {
                        if let Some(&next) = chars.as_str().as_bytes().first() {
                            if (next as char).is_ascii_hexdigit() {
                                hex.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                    } else {
                        result.push(b'\\');
                        result.push(b'x');
                        result.extend(hex.as_bytes());
                    }
                }
                Some(other) => {
                    result.push(b'\\');
                    let mut buf = [0u8; 4];
                    result.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => result.push(b'\\'),
            }
        } else {
            let mut buf = [0u8; 4];
            result.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_newline_cr_tab() {
        assert_eq!(unescape(r"hello\nworld"), b"hello\nworld");
        assert_eq!(unescape(r"a\rb"), b"a\rb");
        assert_eq!(unescape(r"a\tb"), b"a\tb");
    }

    #[test]
    fn unescape_backspace_formfeed_bell_nul() {
        assert_eq!(unescape(r"\b"), vec![0x08]);
        assert_eq!(unescape(r"\f"), vec![0x0C]);
        assert_eq!(unescape(r"\a"), vec![0x07]);
        assert_eq!(unescape(r"\0"), vec![0x00]);
    }

    #[test]
    fn unescape_escape_char() {
        assert_eq!(unescape(r"\e[31m"), vec![0x1B, b'[', b'3', b'1', b'm']);
        assert_eq!(unescape(r"\E[0m"), vec![0x1B, b'[', b'0', b'm']);
    }

    #[test]
    fn unescape_hex() {
        assert_eq!(unescape(r"\x0a"), vec![0x0A]);
        assert_eq!(unescape(r"\x1b"), vec![0x1B]);
        assert_eq!(unescape(r"\xff"), vec![0xFF]);
    }

    #[test]
    fn unescape_unicode() {
        assert_eq!(unescape(r"\u0041"), b"A");
        assert_eq!(unescape(r"\u4F60"), "你".as_bytes());
    }

    #[test]
    fn unescape_backslash_literal() {
        assert_eq!(unescape(r"a\\b"), b"a\\b");
    }

    #[test]
    fn unescape_unknown_escape_passthrough() {
        assert_eq!(unescape(r"\z"), b"\\z");
    }

    #[test]
    fn unescape_combined() {
        assert_eq!(unescape(r"ls\n"), vec![b'l', b's', b'\n']);
        assert_eq!(unescape(r"echo hello\r"), b"echo hello\r");
    }

    #[test]
    fn unescape_trailing_backslash() {
        assert_eq!(unescape("trail\\"), b"trail\\");
    }

    #[test]
    fn unescape_empty() {
        assert_eq!(unescape(""), b"");
    }
}
