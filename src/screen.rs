//! ANSI terminal screen buffer.
//!
//! Parses VT100/ANSI escape sequences via the `vte` crate and maintains
//! a cell grid that can be queried for content.

use crate::consts::{TAB_WIDTH, TRANSCRIPT_BUF_CAPACITY};
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellAttr {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
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
    /// Transcript buffer: captures printable text for session logging.
    transcript: Vec<u8>,
    /// Deferred CR flag: when true, the next printable char truncates the
    /// current transcript line (handling shell line-editing redraws).
    /// A CR followed immediately by LF is treated as a normal line ending.
    transcript_cr_pending: bool,
    /// Pre-CR cursor column saved when `transcript_cr_pending` is set.
    /// Used by the LF snapshot to trim prediction/ghost text that sits
    /// beyond the real cursor position.
    transcript_cr_col: u16,
    /// Suppress transcript recording after alt-screen exit until the
    /// repaint (ConPTY resends the main buffer) is over.  Also set by
    /// `cancel_transcript_line()` (Tab completion, Ctrl+C) so that all
    /// shell-internal redraws are suppressed.  Cleared externally via
    /// `clear_transcript_suppress()` when the next user input arrives.
    transcript_suppress: bool,
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
            transcript: Vec::with_capacity(TRANSCRIPT_BUF_CAPACITY),
            transcript_cr_pending: false,
            transcript_cr_col: 0,
            transcript_suppress: false,
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
            ref mut transcript,
            ref mut transcript_cr_pending,
            ref mut transcript_cr_col,
            ref mut transcript_suppress,
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
            transcript,
            transcript_cr_pending,
            transcript_cr_col,
            transcript_suppress,
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

    /// Drain the transcript buffer, returning accumulated text since last drain.
    /// Preserves the internal Vec allocation for reuse.
    pub fn drain_transcript(&mut self) -> Vec<u8> {
        std::mem::replace(&mut self.transcript, Vec::with_capacity(0))
    }

    /// Access transcript data for reading (e.g. writing to a log file)
    /// without allocating. Call `clear_transcript()` after consuming.
    pub fn transcript(&self) -> &[u8] {
        &self.transcript
    }

    /// Clear transcript buffer, preserving allocated capacity.
    pub fn clear_transcript(&mut self) {
        self.transcript.clear();
    }

    /// End post-alt-screen repaint suppression.
    ///
    /// Called when user input arrives (e.g. `SendData`), signalling that
    /// the ConPTY repaint is over and real output should be recorded.
    pub fn clear_transcript_suppress(&mut self) {
        self.transcript_suppress = false;
    }

    /// Cancel the current incomplete transcript line.
    ///
    /// Called when Tab or Ctrl+C is sent to the PTY — the partially-typed
    /// command should not appear in the log.  Sets `transcript_suppress`
    /// so that all subsequent shell redraws (completion menus, ^C echo)
    /// are also suppressed until the next user input clears it.
    pub fn cancel_transcript_line(&mut self) {
        if let Some(pos) = self.transcript.iter().rposition(|&b| b == b'\n') {
            self.transcript.truncate(pos + 1);
        } else {
            self.transcript.clear();
        }
        self.transcript_cr_pending = false;
        self.transcript_suppress = true;
    }

    /// Flush completed transcript lines (up to the last `\n`) to `w`,
    /// keeping the current incomplete line in the buffer for possible
    /// editing/rewriting by the shell.
    pub fn transcript_flush_lines_to(
        &mut self,
        w: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        if let Some(pos) = self.transcript.iter().rposition(|&b| b == b'\n') {
            w.write_all(&self.transcript[..=pos])?;
            self.transcript.drain(..=pos);
        }
        Ok(())
    }

    /// Flush any remaining transcript content (e.g. at session end).
    pub fn transcript_flush_all_to(
        &mut self,
        w: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        if !self.transcript.is_empty() {
            w.write_all(&self.transcript)?;
            self.transcript.clear();
        }
        Ok(())
    }

    /// Get one line as a string (trimming trailing spaces).
    pub fn line_text(&self, row: u16) -> String {
        let start = row as usize * self.cols as usize;
        let end = start + self.cols as usize;
        let line: String = self.cells[start..end].iter().map(|c| c.ch).collect();
        line.trim_end().to_string()
    }

    /// Get all visible text as a string (plain, no SGR).
    pub fn text(&self) -> String {
        let cols = self.cols as usize;
        let mut result = String::with_capacity(self.rows as usize * (cols + 1));
        let mut trailing_empty = 0usize;
        for row in 0..self.rows as usize {
            let start = row * cols;
            let end = start + cols;
            // Trim trailing spaces in-place without allocating a String.
            let content_len = self.cells[start..end]
                .iter()
                .rposition(|c| c.ch != ' ')
                .map(|i| i + 1)
                .unwrap_or(0);
            if content_len == 0 {
                trailing_empty += 1;
            } else {
                for _ in 0..trailing_empty {
                    result.push('\n');
                }
                trailing_empty = 0;
                if !result.is_empty() {
                    result.push('\n');
                }
                for cell in &self.cells[start..start + content_len] {
                    result.push(cell.ch);
                }
            }
        }
        result
    }

    /// Render screen content as a string with ANSI/SGR escape sequences.
    ///
    /// Unlike `text()`, this preserves colors and attributes (bold, dim,
    /// underline, inverse, fg/bg colors) so that e.g. PowerShell's
    /// PSReadLine inline predictions remain visually distinct.
    pub fn render_ansi(&self) -> String {
        let cols = self.cols as usize;
        let default_attr = CellAttr::default();
        // Generous pre-alloc: text + some SGR overhead per line.
        let mut result = String::with_capacity(self.rows as usize * (cols + 32));
        let mut trailing_empty = 0usize;

        for row in 0..self.rows {
            let start = row as usize * cols;
            let end = start + cols;
            let cells = &self.cells[start..end];

            // Find rightmost cell that is non-trivial (non-space or has attrs).
            let last_content = cells
                .iter()
                .rposition(|c| c.ch != ' ' || c.attr != default_attr)
                .map(|i| i + 1)
                .unwrap_or(0);

            if last_content == 0 {
                trailing_empty += 1;
                continue;
            }

            // Flush buffered empty lines.
            for _ in 0..trailing_empty {
                result.push('\n');
            }
            trailing_empty = 0;
            if !result.is_empty() {
                result.push('\n');
            }

            let mut cur_attr = default_attr;
            for cell in &cells[..last_content] {
                if cell.attr != cur_attr {
                    result.push_str(&sgr_sequence(&cell.attr));
                    cur_attr = cell.attr;
                }
                result.push(cell.ch);
            }
            // Reset at end of line if we changed attrs.
            if cur_attr != default_attr {
                result.push_str("\x1b[0m");
            }
        }
        result
    }

    /// Resize the screen. Content is best-effort preserved.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let old_cols = self.cols as usize;
        let old_rows = self.rows as usize;
        let new_cols = cols as usize;
        let new_rows = rows as usize;
        let new_size = new_cols * new_rows;

        // Resize active cells.
        let mut new_cells = vec![Cell::default(); new_size];
        let copy_cols = old_cols.min(new_cols);
        let copy_rows = old_rows.min(new_rows);
        for r in 0..copy_rows {
            new_cells[r * new_cols..r * new_cols + copy_cols]
                .copy_from_slice(&self.cells[r * old_cols..r * old_cols + copy_cols]);
        }
        self.cells = new_cells;

        // Also resize the saved primary screen so switching back works.
        if let Some(ref mut saved) = self.alt_saved {
            let mut saved_new = vec![Cell::default(); new_size];
            for r in 0..copy_rows {
                let src = r * old_cols;
                let dst = r * new_cols;
                if src + copy_cols <= saved.cells.len() {
                    saved_new[dst..dst + copy_cols]
                        .copy_from_slice(&saved.cells[src..src + copy_cols]);
                }
            }
            saved.cells = saved_new;
            saved.scroll_top = 0;
            saved.scroll_bottom = rows.saturating_sub(1);
            if saved.cursor_x >= cols {
                saved.cursor_x = cols.saturating_sub(1);
            }
            if saved.cursor_y >= rows {
                saved.cursor_y = rows.saturating_sub(1);
            }
        }

        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
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
    transcript: &'a mut Vec<u8>,
    transcript_cr_pending: &'a mut bool,
    transcript_cr_col: &'a mut u16,
    transcript_suppress: &'a mut bool,
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
        self.scroll_up_region(*self.scroll_top as usize, *self.scroll_bottom as usize, n);
    }

    fn scroll_up_region(&mut self, top: usize, bottom: usize, n: u16) {
        let cols = self.cols as usize;
        let n = n as usize;

        if n == 0 || top > bottom {
            return;
        }

        // Move lines up with bulk copy.
        for row in top..=bottom {
            let src_row = row + n;
            if src_row <= bottom {
                let src_start = src_row * cols;
                let dst_start = row * cols;
                self.cells.copy_within(src_start..src_start + cols, dst_start);
            } else {
                let start = row * cols;
                self.cells[start..start + cols].fill(Cell::default());
            }
        }
    }

    fn scroll_down(&mut self, n: u16) {
        self.scroll_down_region(*self.scroll_top as usize, *self.scroll_bottom as usize, n);
    }

    fn scroll_down_region(&mut self, top: usize, bottom: usize, n: u16) {
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
                self.cells.copy_within(src_start..src_start + cols, dst_start);
            } else {
                let start = row * cols;
                self.cells[start..start + cols].fill(Cell::default());
            }
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        let default = Cell::default();
        match mode {
            0 => {
                let start = self.idx(*self.cursor_x, *self.cursor_y);
                self.cells[start..].fill(default);
            }
            1 => {
                let end = self.idx(*self.cursor_x, *self.cursor_y) + 1;
                let len = self.cells.len();
                self.cells[..end.min(len)].fill(default);
            }
            2 | 3 => {
                self.cells.fill(default);
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let row = *self.cursor_y as usize;
        let cols = self.cols as usize;
        let row_start = row * cols;
        let default = Cell::default();
        let len = self.cells.len();
        match mode {
            0 => {
                let start = row_start + *self.cursor_x as usize;
                let end = (row_start + cols).min(len);
                self.cells[start..end].fill(default);
            }
            1 => {
                let end = (row_start + *self.cursor_x as usize + 1).min(len);
                self.cells[row_start..end].fill(default);
            }
            2 => {
                let end = (row_start + cols).min(len);
                self.cells[row_start..end].fill(default);
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
                        2 => self.attr.dim = true,
                        3 => self.attr.italic = true,
                        4 => self.attr.underline = true,
                        7 => self.attr.inverse = true,
                        22 => { self.attr.bold = false; self.attr.dim = false; }
                        23 => self.attr.italic = false,
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
        // Don't record to transcript while on alternate screen (pagers, editors),
        // during post-alt-screen repaint, or when the current cell has dim
        // attributes (PSReadLine inline predictions are rendered dimly).
        if *self.alt_active || *self.transcript_suppress || self.attr.dim {
            return;
        }
        // If CR was pending, the shell is redrawing the current line.
        // Truncate transcript back to the start of the current line so
        // the redrawn content replaces (rather than appends to) the old.
        if *self.transcript_cr_pending {
            if let Some(pos) = self.transcript.iter().rposition(|&b| b == b'\n') {
                self.transcript.truncate(pos + 1);
            } else {
                self.transcript.clear();
            }
            *self.transcript_cr_pending = false;
        }
        // Transcript: record printable characters.
        let mut buf = [0u8; 4];
        self.transcript.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                let mut trimmed = 0usize;
                if !*self.alt_active && !*self.transcript_suppress {
                    // Snapshot screen line content before scroll, replacing the
                    // transcript's current line.  This ensures prediction/redraw
                    // artifacts are excluded — only what the screen actually shows
                    // at commit time gets logged.
                    let row = *self.cursor_y as usize;
                    let cols = self.cols as usize;
                    let start = row * cols;
                    let end = (start + cols).min(self.cells.len());
                    // When CR (or CSI G to col 0) preceded this LF, the saved
                    // pre-CR cursor column marks where real content ends and
                    // prediction / ghost text begins.  Trim within that boundary.
                    let scan_end = if *self.transcript_cr_pending {
                        let cr_col = (*self.transcript_cr_col as usize).min(end - start);
                        start + cr_col
                    } else {
                        end
                    };
                    trimmed = self.cells[start..scan_end]
                        .iter()
                        .rposition(|c| c.ch != ' ')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    // Exclude trailing dim cells (PSReadLine inline prediction).
                    while trimmed > 0 && self.cells[start + trimmed - 1].attr.dim {
                        trimmed -= 1;
                    }
                    let line_start = self.transcript.iter()
                        .rposition(|&b| b == b'\n')
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    self.transcript.truncate(line_start);
                    for cell in &self.cells[start..start + trimmed] {
                        let mut buf = [0u8; 4];
                        self.transcript
                            .extend_from_slice(cell.ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
                self.line_feed();
                if !*self.alt_active && !*self.transcript_suppress {
                    *self.transcript_cr_pending = false;
                    // Suppress \n when the snapshot was empty and the
                    // transcript already ends with \n\n — prevents runs
                    // of blank lines from clear/scroll operations while
                    // allowing at most one blank line in output.
                    let suppress_blank = trimmed == 0 && {
                        let len = self.transcript.len();
                        len >= 2
                            && self.transcript[len - 1] == b'\n'
                            && self.transcript[len - 2] == b'\n'
                    };
                    if !suppress_blank {
                        self.transcript.push(b'\n');
                    }
                }
            }
            b'\r' => {
                if !*self.alt_active && !*self.transcript_suppress {
                    *self.transcript_cr_col = *self.cursor_x;
                    *self.transcript_cr_pending = true;
                }
                *self.cursor_x = 0;
            }
            b'\t' => {
                let next = (*self.cursor_x / TAB_WIDTH + 1) * TAB_WIDTH;
                *self.cursor_x = next.min(self.cols.saturating_sub(1));
                if !*self.alt_active && !*self.transcript_suppress {
                    self.transcript.push(b'\t');
                }
            }
            0x08 => {
                if *self.cursor_x > 0 {
                    *self.cursor_x -= 1;
                }
                if !*self.alt_active && !*self.transcript_suppress {
                    if let Some(&last) = self.transcript.last() {
                        if last != b'\n' {
                            self.transcript.pop();
                        }
                    }
                }
            }
            0x07 => {}
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
                            // Swap cells: save current grid, give alt a fresh one.
                            let cols = self.cols as usize;
                            let rows = self.rows as usize;
                            let mut alt_cells = vec![Cell::default(); cols * rows];
                            std::mem::swap(self.cells, &mut alt_cells);
                            *self.alt_saved = Some(AltSaved {
                                cells: alt_cells,
                                cursor_x: *self.cursor_x,
                                cursor_y: *self.cursor_y,
                                attr: *self.attr,
                                scroll_top: *self.scroll_top,
                                scroll_bottom: *self.scroll_bottom,
                            });
                            *self.cursor_x = 0;
                            *self.cursor_y = 0;
                            *self.alt_active = true;
                            *self.transcript_cr_pending = false;
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
                            // Suppress transcript until repaint is done;
                            // ConPTY will resend the main buffer contents.
                            *self.transcript_suppress = true;
                            *self.transcript_cr_pending = false;
                        }
                    }
                    47 | 1047 => {
                        // Alternate screen (without save/restore cursor)
                        if set && !*self.alt_active {
                            let cols = self.cols as usize;
                            let rows = self.rows as usize;
                            let mut alt_cells = vec![Cell::default(); cols * rows];
                            std::mem::swap(self.cells, &mut alt_cells);
                            *self.alt_saved = Some(AltSaved {
                                cells: alt_cells,
                                cursor_x: *self.cursor_x,
                                cursor_y: *self.cursor_y,
                                attr: *self.attr,
                                scroll_top: *self.scroll_top,
                                scroll_bottom: *self.scroll_bottom,
                            });
                            *self.alt_active = true;
                            *self.transcript_cr_pending = false;
                        } else if !set && *self.alt_active {
                            if let Some(saved) = self.alt_saved.take() {
                                *self.cells = saved.cells;
                                *self.scroll_top = saved.scroll_top;
                                *self.scroll_bottom = saved.scroll_bottom;
                            }
                            *self.alt_active = false;
                            *self.transcript_suppress = true;
                            *self.transcript_cr_pending = false;
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
                let new_x = (n.saturating_sub(1)).min(self.cols.saturating_sub(1));
                // Moving to column 0 is equivalent to CR for transcript.
                if !*self.alt_active && !*self.transcript_suppress && new_x == 0 {
                    *self.transcript_cr_col = *self.cursor_x;
                    *self.transcript_cr_pending = true;
                }
                *self.cursor_x = new_x;
            }
            'H' | 'f' => {
                // Cursor Position
                let row = p(0, 1);
                let col = p(1, 1);
                let old_y = *self.cursor_y;
                let old_x = *self.cursor_x;
                let new_y = (row.saturating_sub(1)).min(self.rows.saturating_sub(1));
                let new_x = (col.saturating_sub(1)).min(self.cols.saturating_sub(1));
                *self.cursor_y = new_y;
                *self.cursor_x = new_x;
                if !*self.alt_active && !*self.transcript_suppress {
                    if new_y == old_y && new_x == 0 {
                        *self.transcript_cr_col = old_x;
                        *self.transcript_cr_pending = true;
                    }
                }
            }
            'J' => {
                // Erase in Display
                self.erase_in_display(p(0, 0));
            }
            'K' => {
                // Erase in Line
                let mode = p(0, 0);
                self.erase_in_line(mode);
                // Transcript: reflect the erase by truncating the current line.
                if !*self.alt_active && !*self.transcript_suppress {
                    match mode {
                        0 => {
                            // Erase cursor → end: keep only cursor_x chars on current line.
                            let line_start = self.transcript.iter()
                                .rposition(|&b| b == b'\n')
                                .map(|p| p + 1)
                                .unwrap_or(0);
                            let target = *self.cursor_x as usize;
                            if self.transcript.len() - line_start > target {
                                self.transcript.truncate(line_start + target);
                            }
                        }
                        2 => {
                            // Erase entire line.
                            let line_start = self.transcript.iter()
                                .rposition(|&b| b == b'\n')
                                .map(|p| p + 1)
                                .unwrap_or(0);
                            self.transcript.truncate(line_start);
                        }
                        _ => {}
                    }
                }
            }
            'L' => {
                // Insert Lines: push lines down from cursor_y within scroll region
                let n = p(0, 1);
                self.scroll_down_region(
                    *self.cursor_y as usize,
                    *self.scroll_bottom as usize,
                    n,
                );
            }
            'M' => {
                // Delete Lines: pull lines up from cursor_y within scroll region
                let n = p(0, 1);
                self.scroll_up_region(
                    *self.cursor_y as usize,
                    *self.scroll_bottom as usize,
                    n,
                );
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
                // DEC spec: cursor moves to home position (0,0), not scroll_top
                *self.cursor_x = 0;
                *self.cursor_y = 0;
            }
            'P' => {
                // Delete Characters — shift left with bulk copy, fill tail.
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                let src_start = row_start + col + n;
                let dst_start = row_start + col;
                if col + n < cols {
                    self.cells.copy_within(src_start..row_start + cols, dst_start);
                    self.cells[row_start + cols - n..row_start + cols].fill(Cell::default());
                } else {
                    self.cells[dst_start..row_start + cols].fill(Cell::default());
                }
            }
            '@' => {
                // Insert Characters — shift right with bulk copy, fill gap.
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                if col + n < cols {
                    let src = row_start + col;
                    let dst = row_start + col + n;
                    self.cells.copy_within(src..row_start + cols - n, dst);
                }
                let fill_end = (row_start + col + n).min(row_start + cols);
                self.cells[row_start + col..fill_end].fill(Cell::default());
            }
            'X' => {
                // Erase Characters
                let n = p(0, 1) as usize;
                let row = *self.cursor_y as usize;
                let col = *self.cursor_x as usize;
                let cols = self.cols as usize;
                let row_start = row * cols;
                let erase_end = (row_start + (col + n).min(cols)).min(self.cells.len());
                self.cells[row_start + col..erase_end].fill(Cell::default());
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

/// Build an SGR escape sequence that sets the given attributes from a
/// reset state.  E.g. bold + red foreground → `\x1b[0;1;31m`.
pub(crate) fn sgr_sequence(attr: &CellAttr) -> String {
    use std::fmt::Write;

    if *attr == CellAttr::default() {
        return "\x1b[0m".to_string();
    }

    let mut seq = String::with_capacity(24);
    seq.push_str("\x1b[0");
    if attr.bold {
        seq.push_str(";1");
    }
    if attr.dim {
        seq.push_str(";2");
    }
    if attr.italic {
        seq.push_str(";3");
    }
    if attr.underline {
        seq.push_str(";4");
    }
    if attr.inverse {
        seq.push_str(";7");
    }
    match attr.fg {
        Color::Default => {}
        Color::Index(n) if n < 8 => { let _ = write!(seq, ";{}", 30 + n); }
        Color::Index(n) if n < 16 => { let _ = write!(seq, ";{}", 90 + n - 8); }
        Color::Index(n) => { let _ = write!(seq, ";38;5;{}", n); }
        Color::Rgb(r, g, b) => { let _ = write!(seq, ";38;2;{};{};{}", r, g, b); }
    }
    match attr.bg {
        Color::Default => {}
        Color::Index(n) if n < 8 => { let _ = write!(seq, ";{}", 40 + n); }
        Color::Index(n) if n < 16 => { let _ = write!(seq, ";{}", 100 + n - 8); }
        Color::Index(n) => { let _ = write!(seq, ";48;5;{}", n); }
        Color::Rgb(r, g, b) => { let _ = write!(seq, ";48;2;{};{};{}", r, g, b); }
    }
    seq.push('m');
    seq
}

#[cfg(test)]
mod tests;
