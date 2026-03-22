//! ANSI terminal screen buffer.
//!
//! Parses VT100/ANSI escape sequences via the `vte` crate and maintains
//! a cell grid that can be queried for content.

use vte::{Params, Parser, Perform};

/// A single cell in the screen buffer.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub attr: CellAttr,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            attr: CellAttr::default(),
        }
    }
}

/// Cell attributes (basic SGR).
#[derive(Debug, Clone, Copy, Default)]
pub struct CellAttr {
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
    pub fg: Color,
    pub bg: Color,
}

/// Basic color representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Index(u8),
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Self::Default
    }
}

/// Terminal screen buffer with VTE parser.
pub struct Screen {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    cursor_x: u16,
    cursor_y: u16,
    attr: CellAttr,
    parser: Parser,
    /// Saved cursor position (for DECSC/DECRC).
    saved_cursor: (u16, u16),
    /// Scroll region (top, bottom) - 0-based, inclusive.
    scroll_top: u16,
    scroll_bottom: u16,
    // ── DEC mode flags ───────────────────────────────────
    /// DECAWM: auto-wrap mode (default on).
    auto_wrap: bool,
    /// DECTCEM: cursor visible.
    cursor_visible: bool,
    /// Whether we are in the alternate screen buffer.
    alt_active: bool,
    /// Saved primary screen state (cells, cursor, scroll region, attr).
    alt_saved: Option<AltSaved>,
    /// Window/icon title set via OSC 0/1/2.
    pub title: String,
}

/// Saved state when switching to alternate screen.
struct AltSaved {
    cells: Vec<Cell>,
    cursor_x: u16,
    cursor_y: u16,
    attr: CellAttr,
    scroll_top: u16,
    scroll_bottom: u16,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("cursor", &(self.cursor_x, self.cursor_y))
            .finish()
    }
}

impl Screen {
    /// Create a new screen buffer.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = cols as usize * rows as usize;
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); size],
            cursor_x: 0,
            cursor_y: 0,
            attr: CellAttr::default(),
            parser: Parser::new(),
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            auto_wrap: true,
            cursor_visible: true,
            alt_active: false,
            alt_saved: None,
            title: String::new(),
        }
    }

    /// Feed raw bytes from the PTY into the screen parser.
    pub fn feed(&mut self, data: &[u8]) {
        // Split borrow: parser vs the rest of the screen state.
        let Screen {
            cols,
            rows,
            ref mut cells,
            ref mut cursor_x,
            ref mut cursor_y,
            ref mut attr,
            ref mut parser,
            ref mut saved_cursor,
            ref mut scroll_top,
            ref mut scroll_bottom,
            ref mut auto_wrap,
            ref mut cursor_visible,
            ref mut alt_active,
            ref mut alt_saved,
            ref mut title,
        } = *self;

        let mut performer = ScreenPerformer {
            cols,
            rows,
            cells,
            cursor_x,
            cursor_y,
            attr,
            saved_cursor,
            scroll_top,
            scroll_bottom,
            auto_wrap,
            cursor_visible,
            alt_active,
            alt_saved,
            title,
        };
        for &byte in data {
            parser.advance(&mut performer, byte);
        }
    }

    /// Get the dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Get cursor position.
    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_x, self.cursor_y)
    }

    /// Get a cell at (col, row).
    pub fn cell(&self, col: u16, row: u16) -> &Cell {
        let idx = row as usize * self.cols as usize + col as usize;
        &self.cells[idx]
    }

    /// Get one line as a string (trimming trailing spaces).
    pub fn line_text(&self, row: u16) -> String {
        let start = row as usize * self.cols as usize;
        let end = start + self.cols as usize;
        let line: String = self.cells[start..end].iter().map(|c| c.ch).collect();
        line.trim_end().to_string()
    }

    /// Get all visible text as a string.
    pub fn text(&self) -> String {
        // Pre-allocate: rough upper bound is rows * (cols + 1 newline)
        let mut result = String::with_capacity(self.rows as usize * (self.cols as usize + 1));
        let mut trailing_empty = 0usize;
        for row in 0..self.rows {
            let line = self.line_text(row);
            if line.is_empty() {
                trailing_empty += 1;
            } else {
                // Flush any buffered empty lines
                for _ in 0..trailing_empty {
                    result.push('\n');
                }
                trailing_empty = 0;
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&line);
            }
        }
        result
    }

    /// Resize the screen. Content is best-effort preserved.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_size = cols as usize * rows as usize;
        let mut new_cells = vec![Cell::default(); new_size];

        let copy_cols = self.cols.min(cols) as usize;
        let copy_rows = self.rows.min(rows) as usize;

        for r in 0..copy_rows {
            let src_start = r * self.cols as usize;
            let dst_start = r * cols as usize;
            new_cells[dst_start..dst_start + copy_cols]
                .copy_from_slice(&self.cells[src_start..src_start + copy_cols]);
        }

        self.cells = new_cells;
        self.cols = cols;
        self.rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        if self.cursor_x >= cols {
            self.cursor_x = cols.saturating_sub(1);
        }
        if self.cursor_y >= rows {
            self.cursor_y = rows.saturating_sub(1);
        }
    }
}

// --- VTE Perform implementation ---

struct ScreenPerformer<'a> {
    cols: u16,
    rows: u16,
    cells: &'a mut Vec<Cell>,
    cursor_x: &'a mut u16,
    cursor_y: &'a mut u16,
    attr: &'a mut CellAttr,
    saved_cursor: &'a mut (u16, u16),
    scroll_top: &'a mut u16,
    scroll_bottom: &'a mut u16,
    auto_wrap: &'a mut bool,
    cursor_visible: &'a mut bool,
    alt_active: &'a mut bool,
    alt_saved: &'a mut Option<AltSaved>,
    title: &'a mut String,
}

impl<'a> ScreenPerformer<'a> {
    fn idx(&self, col: u16, row: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    fn put_char(&mut self, ch: char) {
        if *self.cursor_x >= self.cols {
            if *self.auto_wrap {
                *self.cursor_x = 0;
                self.line_feed();
            } else {
                // No wrap — overwrite last column
                *self.cursor_x = self.cols.saturating_sub(1);
            }
        }
        let idx = self.idx(*self.cursor_x, *self.cursor_y);
        if idx < self.cells.len() {
            self.cells[idx] = Cell {
                ch,
                attr: *self.attr,
            };
        }
        *self.cursor_x += 1;
    }

    fn line_feed(&mut self) {
        if *self.cursor_y >= *self.scroll_bottom {
            self.scroll_up(1);
        } else {
            *self.cursor_y += 1;
        }
    }

    fn scroll_up(&mut self, n: u16) {
        let top = *self.scroll_top as usize;
        let bottom = *self.scroll_bottom as usize;
        let cols = self.cols as usize;
        let n = n as usize;

        if n == 0 || top > bottom {
            return;
        }

        // Move lines up
        for row in top..=bottom {
            let src_row = row + n;
            if src_row <= bottom {
                let src_start = src_row * cols;
                let dst_start = row * cols;
                for c in 0..cols {
                    self.cells[dst_start + c] = self.cells[src_start + c];
                }
            } else {
                // Clear the line
                let start = row * cols;
                for c in 0..cols {
                    self.cells[start + c] = Cell::default();
                }
            }
        }
    }

    fn scroll_down(&mut self, n: u16) {
        let top = *self.scroll_top as usize;
        let bottom = *self.scroll_bottom as usize;
        let cols = self.cols as usize;
        let n = n as usize;

        if n == 0 || top > bottom {
            return;
        }

        for row in (top..=bottom).rev() {
            if row >= top + n {
                let src_row = row - n;
                let src_start = src_row * cols;
                let dst_start = row * cols;
                for c in 0..cols {
                    self.cells[dst_start + c] = self.cells[src_start + c];
                }
            } else {
                let start = row * cols;
                for c in 0..cols {
                    self.cells[start + c] = Cell::default();
                }
            }
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Erase from cursor to end of screen
                let start = self.idx(*self.cursor_x, *self.cursor_y);
                for i in start..self.cells.len() {
                    self.cells[i] = Cell::default();
                }
            }
            1 => {
                // Erase from start to cursor
                let end = self.idx(*self.cursor_x, *self.cursor_y) + 1;
                for i in 0..end.min(self.cells.len()) {
                    self.cells[i] = Cell::default();
                }
            }
            2 | 3 => {
                // Erase entire screen
                for cell in self.cells.iter_mut() {
                    *cell = Cell::default();
                }
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let row = *self.cursor_y as usize;
        let cols = self.cols as usize;
        let row_start = row * cols;
        match mode {
            0 => {
                // Erase from cursor to end of line
                let start = row_start + *self.cursor_x as usize;
                let end = row_start + cols;
                for i in start..end.min(self.cells.len()) {
                    self.cells[i] = Cell::default();
                }
            }
            1 => {
                // Erase from start of line to cursor
                let end = row_start + *self.cursor_x as usize + 1;
                for i in row_start..end.min(self.cells.len()) {
                    self.cells[i] = Cell::default();
                }
            }
            2 => {
                // Erase entire line
                for i in row_start..row_start + cols {
                    if i < self.cells.len() {
                        self.cells[i] = Cell::default();
                    }
                }
            }
            _ => {}
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        loop {
            match iter.next() {
                None => break,
                Some(param) => {
                    let p = param[0];
                    match p {
                        0 => *self.attr = CellAttr::default(),
                        1 => self.attr.bold = true,
                        4 => self.attr.underline = true,
                        7 => self.attr.inverse = true,
                        22 => self.attr.bold = false,
                        24 => self.attr.underline = false,
                        27 => self.attr.inverse = false,
                        30..=37 => self.attr.fg = Color::Index((p - 30) as u8),
                        38 => {
                            // Extended foreground
                            if let Some(next) = iter.next() {
                                if next[0] == 5 {
                                    if let Some(idx) = iter.next() {
                                        self.attr.fg = Color::Index(idx[0] as u8);
                                    }
                                } else if next[0] == 2 {
                                    let r = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    let g = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    let b = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    self.attr.fg = Color::Rgb(r, g, b);
                                }
                            }
                        }
                        39 => self.attr.fg = Color::Default,
                        40..=47 => self.attr.bg = Color::Index((p - 40) as u8),
                        48 => {
                            if let Some(next) = iter.next() {
                                if next[0] == 5 {
                                    if let Some(idx) = iter.next() {
                                        self.attr.bg = Color::Index(idx[0] as u8);
                                    }
                                } else if next[0] == 2 {
                                    let r = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    let g = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    let b = iter.next().map(|v| v[0] as u8).unwrap_or(0);
                                    self.attr.bg = Color::Rgb(r, g, b);
                                }
                            }
                        }
                        49 => self.attr.bg = Color::Default,
                        90..=97 => self.attr.fg = Color::Index((p - 90 + 8) as u8),
                        100..=107 => self.attr.bg = Color::Index((p - 100 + 8) as u8),
                        _ => {} // ignore unknown
                    }
                }
            }
        }
    }
}

impl<'a> Perform for ScreenPerformer<'a> {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                // LF, VT, FF
                self.line_feed();
            }
            b'\r' => {
                *self.cursor_x = 0;
            }
            b'\t' => {
                // Tab: move to next 8-column boundary
                let next = (*self.cursor_x / 8 + 1) * 8;
                *self.cursor_x = next.min(self.cols.saturating_sub(1));
            }
            0x08 => {
                // Backspace
                if *self.cursor_x > 0 {
                    *self.cursor_x -= 1;
                }
            }
            0x07 => {
                // Bell - ignore
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // Handle DEC private modes (CSI ? Pn h/l)
        if intermediates == [b'?'] && (action == 'h' || action == 'l') {
            let set = action == 'h';
            for param in params.iter() {
                match param[0] {
                    1 => {} // DECCKM – application cursor keys (no-op, not relevant for buffer)
                    7 => *self.auto_wrap = set,   // DECAWM
                    25 => *self.cursor_visible = set, // DECTCEM
                    1049 => {
                        // Alternate screen buffer (save cursor + switch)
                        if set && !*self.alt_active {
                            *self.alt_saved = Some(AltSaved {
                                cells: self.cells.clone(),
                                cursor_x: *self.cursor_x,
                                cursor_y: *self.cursor_y,
                                attr: *self.attr,
                                scroll_top: *self.scroll_top,
                                scroll_bottom: *self.scroll_bottom,
                            });
                            // Clear the alternate screen
                            for cell in self.cells.iter_mut() {
                                *cell = Cell::default();
                            }
                            *self.cursor_x = 0;
                            *self.cursor_y = 0;
                            *self.alt_active = true;
                        } else if !set && *self.alt_active {
                            if let Some(saved) = self.alt_saved.take() {
                                *self.cells = saved.cells;
                                *self.cursor_x = saved.cursor_x;
                                *self.cursor_y = saved.cursor_y;
                                *self.attr = saved.attr;
                                *self.scroll_top = saved.scroll_top;
                                *self.scroll_bottom = saved.scroll_bottom;
                            }
                            *self.alt_active = false;
                        }
                    }
                    47 | 1047 => {
                        // Alternate screen (without save/restore cursor)
                        if set && !*self.alt_active {
                            *self.alt_saved = Some(AltSaved {
                                cells: self.cells.clone(),
                                cursor_x: *self.cursor_x,
                                cursor_y: *self.cursor_y,
                                attr: *self.attr,
                                scroll_top: *self.scroll_top,
                                scroll_bottom: *self.scroll_bottom,
                            });
                            for cell in self.cells.iter_mut() {
                                *cell = Cell::default();
                            }
                            *self.alt_active = true;
                        } else if !set && *self.alt_active {
                            if let Some(saved) = self.alt_saved.take() {
                                *self.cells = saved.cells;
                                *self.scroll_top = saved.scroll_top;
                                *self.scroll_bottom = saved.scroll_bottom;
                            }
                            *self.alt_active = false;
                        }
                    }
                    _ => {} // ignore other DEC modes
                }
            }
            return;
        }

        let p = |i: usize, default: u16| -> u16 {
            params.iter().nth(i).map_or(default, |v| if v[0] == 0 { default } else { v[0] as u16 })
        };

        match action {
            'A' => {
                // Cursor Up
                let n = p(0, 1);
                *self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'B' | 'e' => {
                // Cursor Down
                let n = p(0, 1);
                *self.cursor_y = (*self.cursor_y + n).min(self.rows.saturating_sub(1));
            }
            'C' | 'a' => {
                // Cursor Forward
                let n = p(0, 1);
                *self.cursor_x = (*self.cursor_x + n).min(self.cols.saturating_sub(1));
            }
            'D' => {
                // Cursor Backward
                let n = p(0, 1);
                *self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            'E' => {
                // Cursor Next Line
                let n = p(0, 1);
                *self.cursor_x = 0;
                *self.cursor_y = (*self.cursor_y + n).min(self.rows.saturating_sub(1));
            }
            'F' => {
                // Cursor Previous Line
                let n = p(0, 1);
                *self.cursor_x = 0;
                *self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'G' | '`' => {
                // Cursor Horizontal Absolute
                let n = p(0, 1);
                *self.cursor_x = (n.saturating_sub(1)).min(self.cols.saturating_sub(1));
            }
            'H' | 'f' => {
                // Cursor Position
                let row = p(0, 1);
                let col = p(1, 1);
                *self.cursor_y = (row.saturating_sub(1)).min(self.rows.saturating_sub(1));
                *self.cursor_x = (col.saturating_sub(1)).min(self.cols.saturating_sub(1));
            }
            'J' => {
                // Erase in Display
                self.erase_in_display(p(0, 0));
            }
            'K' => {
                // Erase in Line
                self.erase_in_line(p(0, 0));
            }
            'L' => {
                // Insert Lines
                let n = p(0, 1);
                self.scroll_down(n);
            }
            'M' => {
                // Delete Lines
                let n = p(0, 1);
                self.scroll_up(n);
            }
            'S' => {
                // Scroll Up
                let n = p(0, 1);
                self.scroll_up(n);
            }
            'T' => {
                // Scroll Down
                let n = p(0, 1);
                self.scroll_down(n);
            }
            'd' => {
                // Vertical Position Absolute
                let n = p(0, 1);
                *self.cursor_y = (n.saturating_sub(1)).min(self.rows.saturating_sub(1));
            }
            'm' => {
                // SGR - Select Graphic Rendition
                self.apply_sgr(params);
            }
            'r' => {
                // Set Scrolling Region (DECSTBM)
                let top = p(0, 1);
                let bottom = p(1, self.rows);
                *self.scroll_top = top.saturating_sub(1);
                *self.scroll_bottom = (bottom.saturating_sub(1)).min(self.rows.saturating_sub(1));
                *self.cursor_x = 0;
                *self.cursor_y = *self.scroll_top;
            }
            'P' => {
                // Delete Characters
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                // Shift characters left
                for c in col..cols {
                    let src = c + n;
                    let cell = if src < cols {
                        self.cells[row_start + src]
                    } else {
                        Cell::default()
                    };
                    self.cells[row_start + c] = cell;
                }
            }
            '@' => {
                // Insert Characters
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                // Shift characters right
                for c in (col..cols).rev() {
                    if c + n < cols {
                        self.cells[row_start + c + n] = self.cells[row_start + c];
                    }
                    if c >= col && c < col + n {
                        self.cells[row_start + c] = Cell::default();
                    }
                }
            }
            'X' => {
                // Erase Characters
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                for c in col..(col + n).min(cols) {
                    self.cells[row_start + c] = Cell::default();
                }
            }
            _ => {} // Ignore unhandled CSI sequences
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => {
                // DECSC: Save cursor
                *self.saved_cursor = (*self.cursor_x, *self.cursor_y);
            }
            b'8' => {
                // DECRC: Restore cursor
                *self.cursor_x = self.saved_cursor.0;
                *self.cursor_y = self.saved_cursor.1;
            }
            b'D' => {
                // Index: move cursor down, scroll if at bottom
                self.line_feed();
            }
            b'M' => {
                // Reverse Index: move cursor up, scroll if at top
                if *self.cursor_y <= *self.scroll_top {
                    self.scroll_down(1);
                } else {
                    *self.cursor_y -= 1;
                }
            }
            b'c' => {
                // RIS: Full reset
                *self.attr = CellAttr::default();
                *self.cursor_x = 0;
                *self.cursor_y = 0;
                *self.scroll_top = 0;
                *self.scroll_bottom = self.rows.saturating_sub(1);
                for cell in self.cells.iter_mut() {
                    *cell = Cell::default();
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 / 1 / 2 — set window/icon title
        if let Some(&first) = params.first() {
            if first == b"0" || first == b"1" || first == b"2" {
                if let Some(&title_bytes) = params.get(1) {
                    if let Ok(t) = std::str::from_utf8(title_bytes) {
                        *self.title = t.to_string();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_text() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Hello, World!");
        assert_eq!(screen.line_text(0), "Hello, World!");
    }

    #[test]
    fn cursor_movement() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Hello, World!\x1b[1;1HWorld");
        assert_eq!(screen.line_text(0), "World, World!");
    }

    #[test]
    fn line_feed_and_cr() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Line1\r\nLine2");
        assert_eq!(screen.line_text(0), "Line1");
        assert_eq!(screen.line_text(1), "Line2");
    }

    #[test]
    fn erase_in_display() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Hello\x1b[2J");
        assert_eq!(screen.line_text(0), "");
    }

    #[test]
    fn sgr_colors() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"\x1b[31mRed\x1b[0mNormal");
        assert_eq!(screen.line_text(0), "RedNormal");
        assert_eq!(screen.cell(0, 0).attr.fg, Color::Index(1));
        assert_eq!(screen.cell(3, 0).attr.fg, Color::Default);
    }

    #[test]
    fn resize() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Hello");
        screen.resize(40, 12);
        assert_eq!(screen.size(), (40, 12));
        assert_eq!(screen.line_text(0), "Hello");
    }

    #[test]
    fn scroll_region() {
        let mut screen = Screen::new(80, 5);
        screen.feed(b"Line0\r\nLine1\r\nLine2\r\nLine3\r\nLine4");
        // Set scroll region to rows 2-4 and add a line
        screen.feed(b"\x1b[2;4r\x1b[4;1H\n");
        assert_eq!(screen.line_text(0), "Line0");
    }

    #[test]
    fn full_text() {
        let mut screen = Screen::new(80, 24);
        screen.feed(b"Line1\r\nLine2\r\nLine3");
        let text = screen.text();
        assert_eq!(text, "Line1\nLine2\nLine3");
    }

    #[test]
    fn rgb_colors() {
        let mut screen = Screen::new(80, 24);
        // ESC[38;2;255;128;0m  — set fg to RGB(255,128,0)
        screen.feed(b"\x1b[38;2;255;128;0mHi");
        assert_eq!(screen.cell(0, 0).attr.fg, Color::Rgb(255, 128, 0));
    }

    #[test]
    fn alternate_screen_buffer() {
        let mut screen = Screen::new(80, 24);
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
        let mut screen = Screen::new(80, 24);
        // OSC 0; title ST
        screen.feed(b"\x1b]0;My Window\x1b\\");
        assert_eq!(screen.title, "My Window");
    }
}
