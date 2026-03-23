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
