use super::*;
use crate::consts::{DEFAULT_COLS, DEFAULT_ROWS};

#[test]
fn basic_text() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello, World!");
    assert_eq!(screen.line_text(0), "Hello, World!");
}

#[test]
fn cursor_movement() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello, World!\x1b[1;1HWorld");
    assert_eq!(screen.line_text(0), "World, World!");
}

#[test]
fn line_feed_and_cr() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Line1\r\nLine2");
    assert_eq!(screen.line_text(0), "Line1");
    assert_eq!(screen.line_text(1), "Line2");
}

#[test]
fn erase_in_display() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello\x1b[2J");
    assert_eq!(screen.line_text(0), "");
}

#[test]
fn sgr_colors() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"\x1b[31mRed\x1b[0mNormal");
    assert_eq!(screen.line_text(0), "RedNormal");
    assert_eq!(screen.cell(0, 0).attr.fg, Color::Index(1));
    assert_eq!(screen.cell(3, 0).attr.fg, Color::Default);
}

#[test]
fn resize() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello");
    screen.resize(40, 12);
    assert_eq!(screen.size(), (40, 12));
    assert_eq!(screen.line_text(0), "Hello");
}

#[test]
fn scroll_region() {
    let mut screen = Screen::new(DEFAULT_COLS, 5);
    screen.feed(b"Line0\r\nLine1\r\nLine2\r\nLine3\r\nLine4");
    // Set scroll region to rows 2-4 and add a line
    screen.feed(b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(screen.line_text(0), "Line0");
}

#[test]
fn full_text() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Line1\r\nLine2\r\nLine3");
    let text = screen.text();
    assert_eq!(text, "Line1\nLine2\nLine3");
}

#[test]
fn rgb_colors() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // ESC[38;2;255;128;0m  — set fg to RGB(255,128,0)
    screen.feed(b"\x1b[38;2;255;128;0mHi");
    assert_eq!(screen.cell(0, 0).attr.fg, Color::Rgb(255, 128, 0));
}

#[test]
fn alternate_screen_buffer() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Primary");
    assert_eq!(screen.line_text(0), "Primary");
    // Switch to alt screen
    screen.feed(b"\x1b[?1049h");
    assert!(screen.alt_active);
    assert_eq!(screen.line_text(0), ""); // alt screen is blank
    screen.feed(b"Alt");
    assert_eq!(screen.line_text(0), "Alt");
    // Switch back to primary
    screen.feed(b"\x1b[?1049l");
    assert!(!screen.alt_active);
    assert_eq!(screen.line_text(0), "Primary");
}

#[test]
fn resize_during_alt_screen() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Primary");
    // Switch to alt screen
    screen.feed(b"\x1b[?1049h");
    screen.feed(b"Alt");
    // Resize while in alt screen
    screen.resize(40, 12);
    assert_eq!(screen.size(), (40, 12));
    assert_eq!(screen.line_text(0), "Alt");
    // Switch back — primary should be resized too
    screen.feed(b"\x1b[?1049l");
    assert_eq!(screen.size(), (40, 12));
    assert_eq!(screen.line_text(0), "Primary");
}

#[test]
fn auto_wrap_mode() {
    // DECAWM on by default — text wraps
    let mut screen = Screen::new(5, 3);
    screen.feed(b"12345X");
    assert_eq!(screen.line_text(0), "12345");
    assert_eq!(screen.line_text(1), "X");
    // Turn off auto-wrap
    let mut screen2 = Screen::new(5, 3);
    screen2.feed(b"\x1b[?7l"); // DECRST 7
    screen2.feed(b"12345X");
    assert_eq!(screen2.line_text(0), "1234X"); // X overwrites col 4
    assert_eq!(screen2.line_text(1), "");
}

#[test]
fn osc_title() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // OSC 0; title ST
    screen.feed(b"\x1b]0;My Window\x1b\\");
    assert_eq!(screen.title, "My Window");
}

#[test]
fn render_ansi_plain() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello");
    // No attributes → render_ansi should equal text()
    assert_eq!(screen.render_ansi(), "Hello");
}

#[test]
fn render_ansi_colors() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"\x1b[31mRed\x1b[0mNormal");
    let ansi = screen.render_ansi();
    // Should contain SGR for red fg, then reset, then "Normal"
    assert!(ansi.contains("\x1b[0;31m"), "expected red SGR in: {}", ansi);
    assert!(ansi.contains("Red"));
    assert!(ansi.contains("Normal"));
}

#[test]
fn render_ansi_dim() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // Simulate PSReadLine-like: "Get-Ch" normal + "ildItem" dim
    screen.feed(b"Get-Ch\x1b[2mildItem\x1b[0m");
    let ansi = screen.render_ansi();
    assert!(ansi.contains("Get-Ch"), "expected plain prefix");
    assert!(ansi.contains("\x1b[0;2m"), "expected dim SGR in: {}", ansi);
    assert!(ansi.contains("ildItem"));
}

#[test]
fn sgr_dim_parse() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"\x1b[2mDim\x1b[22mNormal");
    assert!(screen.cell(0, 0).attr.dim);
    assert!(!screen.cell(0, 0).attr.bold);
    // SGR 22 resets both bold and dim
    assert!(!screen.cell(3, 0).attr.dim);
    assert!(!screen.cell(3, 0).attr.bold);
}

#[test]
fn sgr_sequence_helper() {
    let default = CellAttr::default();
    assert_eq!(sgr_sequence(&default), "\x1b[0m");

    let bold = CellAttr { bold: true, ..default };
    assert_eq!(sgr_sequence(&bold), "\x1b[0;1m");

    let dim_red = CellAttr { dim: true, fg: Color::Index(1), ..default };
    assert_eq!(sgr_sequence(&dim_red), "\x1b[0;2;31m");

    let bright_fg = CellAttr { fg: Color::Index(10), ..default };
    assert_eq!(sgr_sequence(&bright_fg), "\x1b[0;92m");

    let rgb_bg = CellAttr { bg: Color::Rgb(0, 128, 255), ..default };
    assert_eq!(sgr_sequence(&rgb_bg), "\x1b[0;48;2;0;128;255m");
}

#[test]
fn transcript_captures_printable_text() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"Hello\r\nWorld");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "Hello\nWorld");
    // Second drain returns empty.
    assert!(screen.drain_transcript().is_empty());
}

#[test]
fn transcript_ignores_escape_sequences() {
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"\x1b[31mRed\x1b[0m Normal\x1b[2J");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "Red Normal");
}
