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

#[test]
fn transcript_cr_lf_preserves_line() {
    // CR+LF is a normal line ending: content before \r should be preserved.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"prompt> ls\r\noutput.txt");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "prompt> ls\noutput.txt");
}

#[test]
fn transcript_cr_redraw_replaces_line() {
    // CR followed by printable text (shell redraw) should replace the line.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"old command\rnew cmd");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "new cmd");
}

#[test]
fn transcript_backspace_erases() {
    // BS-SPACE-BS pattern (terminal visual erase) should remove char from transcript.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"cat\x08 \x08car");
    let t = screen.drain_transcript();
    // 'cat' → BS pops 't' → 'ca' → space appends ' ' → 'ca '
    // → BS pops ' ' → 'ca' → 'car' → 'cacar'
    // Actually: after BS-SPACE-BS we have 'ca', then 'car' appended → 'cacar'
    // Wait, 'car' overwrites starting at cursor pos. In transcript:
    // 'c','a','t' → pop('t') → 'ca' → push(' ') → 'ca ' → pop(' ') → 'ca' → push('c','a','r') → 'cacar'
    // The transcript is linear, so 'car' appends. But the first two chars of 'car' visually
    // overwrite 'ca' on screen. For a pure append log this is acceptable.
    assert_eq!(String::from_utf8_lossy(&t), "cacar");
}

#[test]
fn transcript_csi_cursor_down_no_newlines() {
    // CSI B (cursor down) is positioning, not content — no transcript newlines.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"line1\x1b[Bline2");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "line1line2");
}

#[test]
fn transcript_csi_cursor_position_no_newlines() {
    // CSI H cursor positioning is layout, not content — no transcript newlines.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"line1\x1b[3;1Hline3");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "line1line3");
}

#[test]
fn transcript_flush_lines() {
    use std::io::Cursor;
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"complete line\r\npartial");

    let mut buf = Cursor::new(Vec::new());
    screen.transcript_flush_lines_to(&mut buf).unwrap();
    // Only the completed line (up to \n) should be flushed.
    assert_eq!(String::from_utf8_lossy(buf.get_ref()), "complete line\n");

    // The partial content remains in the buffer.
    let remaining = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&remaining), "partial");
}

#[test]
fn transcript_alt_screen_suppressed() {
    // Alt screen content (pagers, editors) should NOT appear in transcript.
    // After exit, ConPTY repaints the main buffer — that repaint is also
    // suppressed until user input arrives (clear_transcript_suppress).
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // Before alt screen: a committed line + prompt
    screen.feed(b"before\r\n");
    // Enter alt screen
    screen.feed(b"\x1b[?1049h");
    screen.feed(b"alt content\r\nmore alt");
    // Exit alt screen — repaint suppression starts
    screen.feed(b"\x1b[?1049l");
    // Simulate ConPTY repaint (suppressed)
    screen.feed(b"repaint text");
    // User input clears suppression (simulates Session::write_data)
    screen.clear_transcript_suppress();
    screen.feed(b"after");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "before\nafter");
}

#[test]
fn transcript_csi_g_col0_triggers_redraw() {
    // CSI G to column 1 (= col 0) acts like CR for transcript line replacement.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // Simulate: print "old text", then CSI 1G (go to col 0), then print "new".
    screen.feed(b"old text\x1b[1Gnew");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "new");
}

#[test]
fn transcript_csi_k_erase_truncates() {
    // CSI K mode 0 erases from cursor to end of line.
    // Transcript should be truncated to match.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    // Print "hello world", move cursor to col 5, erase to end, print " there".
    screen.feed(b"hello world\x1b[6G\x1b[K there");
    let t = screen.drain_transcript();
    // "hello world" → CSI 6G (col 5) → CSI K truncates to 5 → "hello" → " there" → "hello there"
    assert_eq!(String::from_utf8_lossy(&t), "hello there");
}

#[test]
fn transcript_csi_k_mode2_erases_line() {
    // CSI 2K erases entire line content from transcript.
    let mut screen = Screen::new(DEFAULT_COLS, DEFAULT_ROWS);
    screen.feed(b"first line\r\nold content\x1b[2Knew");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "first line\nnew");
}

#[test]
fn transcript_cr_col_trims_prediction() {
    // Simulate PSReadLine: type "cmd" (cursor at col 3), then prediction
    // "cmdarg" fills cols 3..6 on screen, then Enter sends CR+LF.
    // The saved pre-CR cursor col (3) should trim the prediction.
    let mut screen = Screen::new(20, 5);
    // Print the typed text "cmd" — cursor advances to col 3.
    screen.feed(b"cmd");
    // The prediction text appears on screen (PSReadLine prints it as chars)
    // but with cursor repositioned back to col 3 afterwards.
    // Simulate: print "arg", then CSI 4G (move cursor back to col 3+1=4?
    // Actually, let's use a simpler simulation: directly place prediction
    // text in screen cells and then send CR+LF.
    screen.feed(b"arg");          // screen now shows "cmdarg", cursor at 6
    screen.feed(b"\x1b[4G");      // cursor back to col 3 (CSI 4G = col 4, 0-indexed = 3)
    // Now Enter: CR (saves cursor_x=3) then LF (snapshot trims to col 3)
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    // Should contain "cmd\n" — the prediction "arg" is trimmed.
    assert_eq!(String::from_utf8_lossy(&t), "cmd\n");
}

#[test]
fn transcript_csi_g_col0_trims_prediction() {
    // Same as above but using CSI 1G (cursor to col 0) instead of CR.
    let mut screen = Screen::new(20, 5);
    screen.feed(b"cmd");
    screen.feed(b"arg");          // prediction text on screen
    screen.feed(b"\x1b[4G");      // cursor back to col 3
    // PSReadLine sends CSI 1G then LF (instead of CR+LF)
    screen.feed(b"\x1b[1G\n");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "cmd\n");
}

#[test]
fn transcript_psreadline_prediction_trimmed() {
    // Simulate a realistic PSReadLine flow with inline predictions:
    //
    // 1. User types "Get-ChildItem" char by char (PSReadLine may show
    //    inline prediction after each keystroke but let's simplify).
    // 2. After the full command is typed, cursor is at col 13.
    // 3. PSReadLine shows prediction "Get-ChildItem" (same text, printed
    //    dimly starting at col 13).  Screen has "Get-ChildItemGet-ChildItem"
    //    in cells 0..26.
    // 4. CSI 14G — cursor repositioned to col 13 (after typed text).
    // 5. User presses Enter → ConPTY sends CR + LF.
    //    CR saves transcript_cr_col=13, then LF snapshot trims to col 13.
    //
    // Expected transcript: "Get-ChildItem\n"
    let mut screen = Screen::new(80, 24);
    // Step 1-2: typed text
    screen.feed(b"Get-ChildItem");
    // Step 3: prediction echoed as printable chars
    screen.feed(b"Get-ChildItem");
    // Step 4: cursor back to end of typed text
    screen.feed(b"\x1b[14G");   // col 13 (1-based: 14)
    // Step 5: Enter
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "Get-ChildItem\n");
}

#[test]
fn transcript_psreadline_csi_g_redraw_with_prediction() {
    // Simulate the full PSReadLine keystroke cycle:
    // 1. User types "G" → PSReadLine shows "Get-ChildItem" as prediction.
    //    ConPTY: print 'G', then print "et-ChildItem" (prediction), then
    //    CSI 2G (cursor back to col 1, after 'G').
    // 2. Eventually user types all of "Get-ChildItem".
    //    ConPTY: CSI 1G (line redraw), CSI 0K, "Get-ChildItem" then
    //    prediction chars "Get-ChildItem" again, CSI 14G.
    // 3. Enter: CR + LF.
    let mut screen = Screen::new(80, 24);
    // Simulate step 2 (the important one):
    screen.feed(b"\x1b[1G\x1b[0K");  // cursor to col 0 + erase line
    screen.feed(b"Get-ChildItem");     // typed text, 13 chars
    screen.feed(b"Get-ChildItem");     // prediction text, 13 more chars
    screen.feed(b"\x1b[14G");          // cursor back to col 13
    // Enter:
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "Get-ChildItem\n");
}

#[test]
fn transcript_clear_collapses_blank_lines() {
    // Simulate "clear" scrolling: many LFs on blank rows should collapse
    // to at most one blank line, not fill the transcript with 24+ newlines.
    let mut screen = Screen::new(80, 24);
    screen.feed(b"prompt> clear\r\n");
    // clear scrolls by sending many LFs (each row scrolls off)
    for _ in 0..24 {
        screen.feed(b"\n");
    }
    // After clear, new prompt drawn at row 0 via CSI H
    screen.feed(b"\x1b[1;1H");
    screen.feed(b"prompt> ");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    // Should NOT have 24 blank lines; at most 1 blank line between
    // the clear command and the new prompt.
    let newline_count = text.matches('\n').count();
    assert!(
        newline_count <= 3,
        "Too many newlines after clear: got {}, text = {:?}",
        newline_count,
        text,
    );
    // Should contain the clear command and the new prompt
    assert!(text.contains("clear"), "missing clear command");
    assert!(text.contains("prompt> "), "missing new prompt");
}

#[test]
fn transcript_alt_screen_repaint_suppressed() {
    // After alt screen exit (e.g. `less` quits), ConPTY repaints the main
    // buffer.  The repaint content should not appear in the transcript —
    // it was already logged before the pager launched.
    let mut screen = Screen::new(80, 24);
    // Before alt screen: user ran `ls` which printed output + prompt
    screen.feed(b"file1.txt\r\nfile2.txt\r\nPS> cat Cargo.toml | less\r\n");
    // Enter alt screen (less starts)
    screen.feed(b"\x1b[?1049h");
    screen.feed(b"pager content line 1\r\npager line 2");
    // Exit alt screen (less quits) — triggers repaint suppression
    screen.feed(b"\x1b[?1049l");
    // ConPTY repaints main buffer (cursor positioning + chars + CR LF)
    screen.feed(b"\x1b[1;1Hfile1.txt\r\n");
    screen.feed(b"\x1b[2;1Hfile2.txt\r\n");
    screen.feed(b"\x1b[3;1HPS> cat Cargo.toml | less\r\n");
    // User input clears suppression (simulates Session::write_data)
    screen.clear_transcript_suppress();
    // New command
    screen.feed(b"PS> cat README.md\r\n");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    // Repaint content should NOT be duplicated.
    // Count occurrences of "file1.txt"
    let count = text.matches("file1.txt").count();
    assert_eq!(count, 1, "file1.txt should appear only once, text = {:?}", text);
    // The new command after repaint should be present
    assert!(text.contains("cat README.md"), "missing post-repaint command, text = {:?}", text);
}

#[test]
fn transcript_dim_prediction_not_recorded() {
    // PSReadLine inline predictions are rendered with dim attribute.
    // They should NOT appear in the transcript.
    let mut screen = Screen::new(80, 24);
    // User types "Get" — normal text
    screen.feed(b"Get");
    // PSReadLine shows prediction "-ChildItem" in dim
    screen.feed(b"\x1b[2m-ChildItem\x1b[0m");
    // Cursor back to after typed text
    screen.feed(b"\x1b[4G");
    let t = screen.drain_transcript();
    // Only "Get" should be recorded, not the dim prediction
    assert_eq!(String::from_utf8_lossy(&t), "Get");
}

#[test]
fn transcript_dim_prediction_trimmed_at_lf() {
    // When LF fires, cells with dim attribute at end of row are trimmed.
    let mut screen = Screen::new(20, 5);
    // Type "cmd" (3 chars, normal attr)
    screen.feed(b"cmd");
    // Prediction "arg" in dim (3 chars at cols 3-5)
    screen.feed(b"\x1b[2marg\x1b[0m");
    // Cursor back to col 3
    screen.feed(b"\x1b[4G");
    // Enter: CR + LF — snapshot should trim dim cells
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    assert_eq!(String::from_utf8_lossy(&t), "cmd\n");
}

#[test]
fn transcript_dim_prediction_wrap_suppressed() {
    // PSReadLine prediction wraps past column boundary.
    // The wrapped continuation line (all dim cells) should be suppressed.
    let mut screen = Screen::new(10, 5);
    // User types "ls" (2 chars, normal)
    screen.feed(b"ls");
    // Prediction: 12 dim chars that wrap past col 10
    screen.feed(b"\x1b[2m -la /tmp\x1b[0m");
    // At this point, screen row 0 has "ls -la /tm" (10 cols),
    // row 1 has "p " (wrapped).  The dim text on row 0 occupies cols 2-9.
    // The LF caused by auto-wrap should not commit the dim-heavy line.
    // Final cursor position back to col 2
    screen.feed(b"\x1b[1;3H"); // row 1 col 3 = (0,2)
    // User presses Enter: CR + LF
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    // Should NOT contain the prediction text "-la /tmp"
    assert!(!text.contains("-la /tmp"), "prediction should not appear: {:?}", text);
    // "ls" should be present
    assert!(text.contains("ls"), "typed text should appear: {:?}", text);
}

#[test]
fn transcript_cancel_line_removes_partial_input() {
    // Ctrl+C cancels the current input.  The partially-typed command
    // should NOT appear in the transcript.  cancel_transcript_line() sets
    // transcript_suppress, so everything is suppressed until the next
    // user input (clear_transcript_suppress).
    let mut screen = Screen::new(80, 24);
    // User ran a command first.
    screen.feed(b"first\r\n");
    // User types "cat C" (echoed via print)
    screen.feed(b"cat C");
    // Ctrl+C arrives via write_data -> cancel_transcript_line()
    screen.cancel_transcript_line();
    // Shell responds with CR+LF (suppressed)
    screen.feed(b"\r\n");
    // Next user input clears suppress (simulates write_data)
    screen.clear_transcript_suppress();
    screen.feed(b"next cmd\r\n");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    assert!(!text.contains("cat C"), "cancelled input must not appear: {:?}", text);
    assert!(text.contains("next cmd"), "subsequent command should appear: {:?}", text);
}

#[test]
fn transcript_repaint_cr_does_not_contaminate() {
    // During post-alt-screen repaint, CRs in the repaint data must NOT
    // set transcript_cr_pending -- that would corrupt the next line's
    // snapshot (off-by-1 cursor column).
    let mut screen = Screen::new(80, 24);
    // Output + command before alt screen
    screen.feed(b"file1.txt\r\nPS> cat Cargo.toml | less\r\n");
    // Enter alt screen
    screen.feed(b"\x1b[?1049h");
    screen.feed(b"pager content");
    // Exit alt screen -> suppress
    screen.feed(b"\x1b[?1049l");
    // Repaint: CSI H + text + CR + LF (all suppressed)
    screen.feed(b"\x1b[1;1Hfile1.txt\r\n");
    screen.feed(b"\x1b[2;1HPS> cat Cargo.toml | less\r\n");
    // End suppression (simulates user input via write_data)
    screen.clear_transcript_suppress();
    // User types next command + Enter
    screen.feed(b"PS> echo hello\r\n");
    screen.feed(b"hello\r\n");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    // "file1.txt" appears exactly once (not duplicated by repaint)
    let count = text.matches("file1.txt").count();
    assert_eq!(count, 1, "file1.txt must appear once, text = {:?}", text);
    assert!(text.contains("echo hello"), "new command should appear: {:?}", text);
    assert!(text.contains("hello\n"), "command output should appear: {:?}", text);
}

#[test]
fn transcript_tab_completion_suppressed() {
    // Tab completion triggers shell redraws (completion menu, etc.) that
    // include LFs.  The incomplete command and all redraws should be
    // suppressed until the next non-Tab user input.
    let mut screen = Screen::new(80, 24);
    // User types "cat C"
    screen.feed(b"cat C");
    // User presses Tab -> cancel_transcript_line() via write_data
    screen.cancel_transcript_line();
    // Shell draws completion menu (multiple LFs) — all suppressed
    screen.feed(b"\r\n");
    screen.feed(b"Cargo.toml\r\n");
    screen.feed(b"CHANGELOG.md\r\n");
    // Shell redraws prompt with completed text (suppressed)
    screen.feed(b"\x1b[1;1Hcat .\\Cargo.toml");
    // User presses Enter → write_data clears suppress, then shell echoes
    screen.clear_transcript_suppress();
    screen.feed(b"\r\n");
    let t = screen.drain_transcript();
    let text = String::from_utf8_lossy(&t);
    // "cat C" must NOT appear (was truncated on Tab)
    assert!(!text.contains("cat C"), "incomplete text must not appear: {:?}", text);
    // Completion menu entries must NOT appear
    assert!(!text.contains("CHANGELOG"), "menu entries must not appear: {:?}", text);
    // The committed line (screen snapshot on Enter's LF) SHOULD appear
    assert!(text.contains("cat .\\Cargo.toml"), "completed command should appear: {:?}", text);
}
