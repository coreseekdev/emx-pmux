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
mod tests;
