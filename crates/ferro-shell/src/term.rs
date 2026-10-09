//! A small VT100/ANSI terminal emulator for the Command Prompt window.
//!
//! Supports what shells and common CLI tools emit: cursor movement, erase,
//! insert/delete, SGR colors (16-color + bold), save/restore and device
//! status reports. Text defaults to phosphor green on black. There is no
//! scrollback, to keep each terminal at a few KB.

/// Default text color: classic green-screen phosphor.
pub const DEFAULT_FG: u32 = 0x33FF33;
pub const DEFAULT_BG: u32 = 0x000000;

/// ANSI order (black, red, green, yellow, blue, magenta, cyan, white), VGA tones.
pub const PALETTE: [u32; 16] = [
    0x000000, 0xAA0000, 0x00AA00, 0xAA5500, 0x0000AA, 0xAA00AA, 0x00AAAA, 0xAAAAAA, //
    0x555555, 0xFF5555, 0x55FF55, 0xFFFF55, 0x5555FF, 0xFF55FF, 0x55FFFF, 0xFFFFFF,
];

/// Color slot meaning "the terminal default" rather than a palette entry.
const DEFAULT: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u8,
    fg: u8,
    bg: u8,
}

impl Cell {
    const BLANK: Cell = Cell { ch: b' ', fg: DEFAULT, bg: DEFAULT };

    pub fn fg(&self) -> u32 {
        if self.fg == DEFAULT {
            DEFAULT_FG
        } else {
            PALETTE[self.fg as usize]
        }
    }

    pub fn bg(&self) -> u32 {
        if self.bg == DEFAULT {
            DEFAULT_BG
        } else {
            PALETTE[self.bg as usize]
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

pub struct Term {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    cx: usize,
    cy: usize,
    /// Set after writing the last column; the next char wraps first.
    wrap_pending: bool,
    fg: u8,
    bg: u8,
    bold: bool,
    reverse: bool,
    saved: (usize, usize),
    pub cursor_visible: bool,
    state: State,
    params: Vec<u16>,
    private: bool,
    /// Bytes still expected in the current UTF-8 sequence.
    utf8_left: u8,
    /// Answers to queries (e.g. cursor position) for the host to send back.
    pub replies: Vec<u8>,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Self {
            cols,
            rows,
            cells: vec![Cell::BLANK; cols * rows],
            cx: 0,
            cy: 0,
            wrap_pending: false,
            fg: DEFAULT,
            bg: DEFAULT,
            bold: false,
            reverse: false,
            saved: (0, 0),
            cursor_visible: true,
            state: State::Ground,
            params: Vec::new(),
            private: false,
            utf8_left: 0,
            replies: Vec::new(),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cx, self.cy)
    }

    pub fn cell(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.cols + x]
    }

    /// Text of row `y`, trailing spaces trimmed (for tests and copy).
    pub fn row_text(&self, y: usize) -> String {
        let row = &self.cells[y * self.cols..(y + 1) * self.cols];
        String::from_utf8_lossy(&row.iter().map(|c| c.ch).collect::<Vec<_>>()).trim_end().to_owned()
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        // Keep the bottom of the old screen, where the prompt is.
        let skip = (self.cy + 1).saturating_sub(rows);
        let mut cells = vec![Cell::BLANK; cols * rows];
        for y in 0..rows.min(self.rows - skip) {
            for x in 0..cols.min(self.cols) {
                cells[y * cols + x] = self.cells[(y + skip) * self.cols + x];
            }
        }
        self.cells = cells;
        self.cols = cols;
        self.rows = rows;
        self.cy = (self.cy - skip).min(rows - 1);
        self.cx = self.cx.min(cols - 1);
        self.wrap_pending = false;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Ground => self.ground(b),
            State::Esc => self.escape(b),
            State::Csi => match b {
                b'0'..=b'9' => {
                    if self.params.is_empty() {
                        self.params.push(0);
                    }
                    let p = self.params.last_mut().unwrap();
                    *p = p.saturating_mul(10).saturating_add(u16::from(b - b'0'));
                }
                b';' => {
                    if self.params.is_empty() {
                        self.params.push(0);
                    }
                    self.params.push(0);
                }
                b'?' | b'>' | b'=' => self.private = true,
                0x40..=0x7E => {
                    self.state = State::Ground;
                    self.csi(b);
                }
                0x18 | 0x1A => self.state = State::Ground,
                _ => {} // intermediates
            },
            State::Osc => match b {
                0x07 => self.state = State::Ground,
                0x1B => self.state = State::OscEsc,
                _ => {}
            },
            State::OscEsc => self.state = if b == b'\\' { State::Ground } else { State::Osc },
        }
    }

    fn ground(&mut self, b: u8) {
        match b {
            0x1B => self.state = State::Esc,
            b'\r' => {
                self.cx = 0;
                self.wrap_pending = false;
            }
            b'\n' | 0x0B | 0x0C => self.linefeed(),
            0x08 => {
                self.cx = self.cx.saturating_sub(1);
                self.wrap_pending = false;
            }
            b'\t' => {
                self.cx = ((self.cx / 8 + 1) * 8).min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x20..=0x7E => self.put(b),
            0xC0..=0xF7 => {
                // Start of a multi-byte char: our font is ASCII-only.
                self.utf8_left = if b >= 0xF0 {
                    3
                } else if b >= 0xE0 {
                    2
                } else {
                    1
                };
                self.put(b'?');
            }
            0x80..=0xBF if self.utf8_left > 0 => self.utf8_left -= 1,
            _ => {}
        }
    }

    fn escape(&mut self, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.private = false;
            }
            b']' => self.state = State::Osc,
            b'7' => self.saved = (self.cx, self.cy),
            b'8' => (self.cx, self.cy) = self.saved,
            b'c' => *self = Term::new(self.cols, self.rows),
            b'D' => self.linefeed(),
            b'E' => {
                self.cx = 0;
                self.linefeed();
            }
            b'M' => {
                if self.cy == 0 {
                    self.scroll_down(1);
                } else {
                    self.cy -= 1;
                }
            }
            _ => {}
        }
    }

    fn put(&mut self, ch: u8) {
        if self.wrap_pending {
            self.cx = 0;
            self.linefeed();
        }
        let (fg, bg) = self.effective_colors();
        let i = self.cy * self.cols + self.cx;
        self.cells[i] = Cell { ch, fg, bg };
        if self.cx + 1 == self.cols {
            self.wrap_pending = true;
        } else {
            self.cx += 1;
        }
    }

    fn effective_colors(&self) -> (u8, u8) {
        let fg = if self.bold && self.fg < 8 { self.fg + 8 } else { self.fg };
        if self.reverse {
            // Swapping "default" slots means default background as text.
            let swap = |c: u8, other_default: u8| if c == DEFAULT { other_default } else { c };
            (swap(self.bg, 0), swap(fg, 10))
        } else {
            (fg, self.bg)
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: b' ', fg: DEFAULT, bg: self.bg }
    }

    fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cy + 1 == self.rows {
            self.scroll_up(1);
        } else {
            self.cy += 1;
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.rows);
        let blank = self.blank();
        self.cells.copy_within(n * self.cols.., 0);
        let len = self.cells.len();
        self.cells[len - n * self.cols..].fill(blank);
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.rows);
        let blank = self.blank();
        let len = self.cells.len();
        self.cells.copy_within(..len - n * self.cols, n * self.cols);
        self.cells[..n * self.cols].fill(blank);
    }

    fn param(&self, i: usize, default: u16) -> usize {
        usize::from(self.params.get(i).copied().filter(|&v| v != 0).unwrap_or(default))
    }

    fn csi(&mut self, fin: u8) {
        let n = self.param(0, 1);
        let (cols, rows) = (self.cols, self.rows);
        self.wrap_pending = false;
        let row_start = self.cy * cols;
        match fin {
            b'A' => self.cy = self.cy.saturating_sub(n),
            b'B' | b'e' => self.cy = (self.cy + n).min(rows - 1),
            b'C' | b'a' => self.cx = (self.cx + n).min(cols - 1),
            b'D' => self.cx = self.cx.saturating_sub(n),
            b'E' => (self.cx, self.cy) = (0, (self.cy + n).min(rows - 1)),
            b'F' => (self.cx, self.cy) = (0, self.cy.saturating_sub(n)),
            b'G' | b'`' => self.cx = (n - 1).min(cols - 1),
            b'd' => self.cy = (n - 1).min(rows - 1),
            b'H' | b'f' => {
                self.cy = (self.param(0, 1) - 1).min(rows - 1);
                self.cx = (self.param(1, 1) - 1).min(cols - 1);
            }
            b'J' => {
                let blank = self.blank();
                let at = row_start + self.cx;
                match self.params.first().copied().unwrap_or(0) {
                    0 => self.cells[at..].fill(blank),
                    1 => self.cells[..=at].fill(blank),
                    _ => self.cells.fill(blank),
                }
            }
            b'K' => {
                let blank = self.blank();
                let (start, at, end) = (row_start, row_start + self.cx, row_start + cols);
                match self.params.first().copied().unwrap_or(0) {
                    0 => self.cells[at..end].fill(blank),
                    1 => self.cells[start..=at].fill(blank),
                    _ => self.cells[start..end].fill(blank),
                }
            }
            b'P' => {
                let n = n.min(cols - self.cx);
                let blank = self.blank();
                let line = &mut self.cells[row_start + self.cx..row_start + cols];
                line.copy_within(n.., 0);
                let len = line.len();
                line[len - n..].fill(blank);
            }
            b'@' => {
                let n = n.min(cols - self.cx);
                let blank = self.blank();
                let line = &mut self.cells[row_start + self.cx..row_start + cols];
                let len = line.len();
                line.copy_within(..len - n, n);
                line[..n].fill(blank);
            }
            b'X' => {
                let blank = self.blank();
                let end = (self.cx + n).min(cols);
                self.cells[row_start + self.cx..row_start + end].fill(blank);
            }
            b'L' | b'M' => {
                // Insert/delete lines within the region from the cursor down.
                let n = n.min(rows - self.cy);
                let blank = self.blank();
                let region = &mut self.cells[row_start..];
                let len = region.len();
                if fin == b'L' {
                    region.copy_within(..len - n * cols, n * cols);
                    region[..n * cols].fill(blank);
                } else {
                    region.copy_within(n * cols.., 0);
                    region[len - n * cols..].fill(blank);
                }
            }
            b'S' => self.scroll_up(n),
            b'T' => self.scroll_down(n),
            b'm' => self.sgr(),
            b's' => self.saved = (self.cx, self.cy),
            b'u' => (self.cx, self.cy) = self.saved,
            b'h' | b'l' if self.private && self.params.contains(&25) => self.cursor_visible = fin == b'h',
            b'n' if self.param(0, 0) == 6 => {
                let reply = format!("\x1b[{};{}R", self.cy + 1, self.cx + 1);
                self.replies.extend_from_slice(reply.as_bytes());
            }
            b'n' if self.param(0, 0) == 5 => self.replies.extend_from_slice(b"\x1b[0n"),
            _ => {}
        }
    }

    fn sgr(&mut self) {
        if self.params.is_empty() {
            self.params.push(0);
        }
        let mut i = 0;
        while i < self.params.len() {
            match self.params[i] {
                0 => {
                    self.fg = DEFAULT;
                    self.bg = DEFAULT;
                    self.bold = false;
                    self.reverse = false;
                }
                1 => self.bold = true,
                22 => self.bold = false,
                7 => self.reverse = true,
                27 => self.reverse = false,
                p @ 30..=37 => self.fg = (p - 30) as u8,
                39 => self.fg = DEFAULT,
                p @ 40..=47 => self.bg = (p - 40) as u8,
                49 => self.bg = DEFAULT,
                p @ 90..=97 => self.fg = (p - 90 + 8) as u8,
                p @ 100..=107 => self.bg = (p - 100 + 8) as u8,
                p @ (38 | 48) => {
                    // 256-color / truecolor: map the 16 base colors, skip the rest.
                    let color = match self.params.get(i + 1) {
                        Some(5) => {
                            i += 2;
                            self.params.get(i).filter(|&&c| c < 16).map(|&c| c as u8)
                        }
                        Some(2) => {
                            i += 4;
                            None
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if p == 38 {
                            self.fg = c;
                        } else {
                            self.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Bytes a terminal sends for a key press (xterm conventions).
pub fn encode_key(key: crate::Key, mods: crate::Mods) -> Vec<u8> {
    use crate::Key;
    match key {
        Key::Char(c) if mods.ctrl && c.is_ascii_alphabetic() => vec![(c.to_ascii_lowercase() as u8) & 0x1F],
        Key::Char(c) => {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf).as_bytes().to_vec();
            if mods.alt {
                [vec![0x1B], s].concat()
            } else {
                s
            }
        }
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => vec![0x7F],
        Key::Tab => b"\t".to_vec(),
        Key::Escape => vec![0x1B],
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        Key::F(n) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24].get(usize::from(n.saturating_sub(5))).copied().unwrap_or(24);
            format!("\x1b[{code}~").into_bytes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_wraps_and_scrolls() {
        let mut t = Term::new(5, 3);
        t.feed(b"hello world\r\nab\r\ncd");
        // "hello" filled row 0, " worl" wrapped... then scrolled.
        assert_eq!(t.row_text(0), "d");
        assert_eq!(t.row_text(1), "ab");
        assert_eq!(t.row_text(2), "cd");
        assert_eq!(t.cursor(), (2, 2));
    }

    #[test]
    fn default_is_green_and_sgr_changes_it() {
        let mut t = Term::new(10, 1);
        t.feed(b"a\x1b[31mb\x1b[1mc\x1b[0md\x1b[44me");
        assert_eq!(t.cell(0, 0).fg(), DEFAULT_FG);
        assert_eq!(t.cell(0, 0).bg(), DEFAULT_BG);
        assert_eq!(t.cell(1, 0).fg(), PALETTE[1]);
        assert_eq!(t.cell(2, 0).fg(), PALETTE[9], "bold brightens");
        assert_eq!(t.cell(3, 0).fg(), DEFAULT_FG);
        assert_eq!(t.cell(4, 0).bg(), PALETTE[4]);
    }

    #[test]
    fn cursor_moves_and_erases() {
        let mut t = Term::new(10, 3);
        t.feed(b"xxxxxxxxxx\r\nyyyyyyyyyy\r\nzzzzzzzzzz");
        t.feed(b"\x1b[2;3H\x1b[K");
        assert_eq!(t.row_text(1), "yy");
        t.feed(b"\x1b[1;1H\x1b[2P");
        assert_eq!(t.row_text(0), "xxxxxxxx");
        t.feed(b"\x1b[2J");
        assert!((0..3).all(|y| t.row_text(y).is_empty()));
        t.feed(b"\x1b[3;4H\x1b[6n");
        assert_eq!(t.replies, b"\x1b[3;4R");
    }

    #[test]
    fn utf8_and_osc_do_not_corrupt() {
        let mut t = Term::new(10, 1);
        t.feed("\x1b]0;title\x07é!".as_bytes());
        assert_eq!(t.row_text(0), "?!");
    }

    #[test]
    fn resize_keeps_prompt_line() {
        let mut t = Term::new(10, 4);
        t.feed(b"1\r\n2\r\n3\r\nC:\\>");
        t.resize(20, 2);
        assert_eq!(t.row_text(0), "3");
        assert_eq!(t.row_text(1), "C:\\>");
        assert_eq!(t.cursor(), (4, 1));
    }

    #[test]
    fn keys_encode_like_xterm() {
        use crate::{Key, Mods};
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        assert_eq!(encode_key(Key::Char('c'), ctrl), [3]);
        assert_eq!(encode_key(Key::Up, Mods::default()), b"\x1b[A");
        assert_eq!(encode_key(Key::F(1), Mods::default()), b"\x1bOP");
        assert_eq!(encode_key(Key::F(5), Mods::default()), b"\x1b[15~");
    }
}
