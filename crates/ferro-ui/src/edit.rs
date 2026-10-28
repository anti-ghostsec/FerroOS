//! A line of text being typed, as Windows' text boxes do it: a selection
//! (Shift with the arrows, Home and End, or the mouse), Ctrl+A, Ctrl+C,
//! Ctrl+X, Ctrl+V and Ctrl+Z, Ctrl with the arrows by words, and the same
//! clipboard every other program uses. Renaming on the desktop and in the
//! file manager both type in one.

use crate::Key;
use ferro_gfx::{color, text_width, Rect, Surface, GLYPH_H, GLYPH_W};

/// What a key did to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The text or the selection changed: draw it again.
    Changed,
    /// Enter: done.
    Done,
    /// Escape: let it be.
    Cancel,
    /// Not a key for the text.
    Ignored,
}

/// One line of text, the cursor in it and what's selected (from `anchor` to
/// the cursor). Places are byte offsets, always between characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    text: String,
    cursor: usize,
    anchor: usize,
    undo: Vec<(String, usize, usize)>,
}

impl TextEdit {
    /// `text`, all of it selected.
    pub fn new(text: &str) -> Self {
        TextEdit { text: text.to_owned(), cursor: text.len(), anchor: 0, undo: Vec::new() }
    }

    /// `text` with `from..to` selected, as a file's name without its
    /// extension is when renaming starts.
    pub fn with_selection(text: &str, from: usize, to: usize) -> Self {
        let mut e = TextEdit::new(text);
        e.select(from, to);
        e
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// What's selected, start before end (empty: nothing is).
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[a..b]
    }

    fn fit(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    pub fn select(&mut self, from: usize, to: usize) {
        self.anchor = self.fit(from);
        self.cursor = self.fit(to);
    }

    pub fn select_all(&mut self) {
        self.select(0, self.text.len());
    }

    fn remember(&mut self) {
        if self.undo.last().is_none_or(|(t, _, _)| *t != self.text) {
            self.undo.push((self.text.clone(), self.cursor, self.anchor));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
    }

    /// Types `s` over the selection. A name is one line: what's after a line
    /// break, and control characters, are left out.
    pub fn insert(&mut self, s: &str) {
        let line: String = s.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
        let (a, b) = self.selection();
        if line.is_empty() && a == b {
            return;
        }
        self.remember();
        self.text.replace_range(a..b, &line);
        self.cursor = a + line.len();
        self.anchor = self.cursor;
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.remember();
        self.text.replace_range(a..b, "");
        (self.cursor, self.anchor) = (a, a);
        true
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }

    /// The start of the word before `at`, or the end of the one after it.
    fn word(&self, at: usize, back: bool) -> usize {
        let word = |c: char| c.is_alphanumeric();
        if back {
            let before: Vec<(usize, char)> = self.text[..at].char_indices().collect();
            let mut i = before.len();
            while i > 0 && !word(before[i - 1].1) {
                i -= 1;
            }
            while i > 0 && word(before[i - 1].1) {
                i -= 1;
            }
            before.get(i).map_or(at, |&(p, _)| p)
        } else {
            let mut p = at;
            let mut chars = self.text[at..].chars().peekable();
            while chars.peek().is_some_and(|&c| !word(c)) {
                p += chars.next().map_or(0, char::len_utf8);
            }
            while chars.peek().is_some_and(|&c| word(c)) {
                p += chars.next().map_or(0, char::len_utf8);
            }
            p
        }
    }

    fn go(&mut self, to: usize, extend: bool) {
        self.cursor = to;
        if !extend {
            self.anchor = to;
        }
    }

    pub fn backspace(&mut self) {
        if !self.delete_selection() && self.cursor > 0 {
            self.remember();
            let p = self.prev(self.cursor);
            self.text.replace_range(p..self.cursor, "");
            (self.cursor, self.anchor) = (p, p);
        }
    }

    pub fn delete(&mut self) {
        if !self.delete_selection() && self.cursor < self.text.len() {
            self.remember();
            let n = self.next(self.cursor);
            self.text.replace_range(self.cursor..n, "");
            self.anchor = self.cursor;
        }
    }

    /// Ctrl+C: the selection onto the clipboard.
    pub fn copy(&self) {
        if !self.selected().is_empty() {
            clipboard::set(self.selected());
        }
    }

    /// Ctrl+X.
    pub fn cut(&mut self) {
        self.copy();
        self.delete_selection();
    }

    /// Ctrl+V: what's on the clipboard, over the selection.
    pub fn paste(&mut self) {
        if let Some(t) = clipboard::get() {
            self.insert(&t);
        }
    }

    /// Ctrl+Z.
    pub fn undo(&mut self) {
        if let Some((t, c, a)) = self.undo.pop() {
            (self.text, self.cursor, self.anchor) = (t, c, a);
        }
    }

    /// Whether Ctrl+Z has anything to undo, and the selection anything to
    /// copy (for a menu's greyed-out items).
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// A key, with Ctrl and Shift held or not.
    pub fn key(&mut self, key: Key, ctrl: bool, shift: bool) -> Edit {
        match key {
            Key::Enter => return Edit::Done,
            Key::Escape => return Edit::Cancel,
            Key::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.select_all(),
                'c' => self.copy(),
                'x' => self.cut(),
                'v' => self.paste(),
                'z' => self.undo(),
                _ => return Edit::Ignored,
            },
            Key::Char(c) if !c.is_control() => self.insert(c.encode_utf8(&mut [0; 4])),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete(),
            Key::Left | Key::Right => {
                let back = key == Key::Left;
                let (a, b) = self.selection();
                let to = if ctrl {
                    self.word(self.cursor, back)
                } else if !shift && a != b {
                    // Off a selection: to its start or end.
                    if back {
                        a
                    } else {
                        b
                    }
                } else if back {
                    self.prev(self.cursor)
                } else {
                    self.next(self.cursor)
                };
                self.go(to, shift);
            }
            Key::Home => self.go(0, shift),
            Key::End => self.go(self.text.len(), shift),
            _ => return Edit::Ignored,
        }
        Edit::Changed
    }

    /// The place nearest `x` pixels into the text as drawn from `scroll`.
    fn place_at(&self, x: i32, scroll: usize) -> usize {
        let mut at = scroll;
        let mut px = 0;
        for c in self.text[scroll..].chars() {
            if px + GLYPH_W / 2 > x {
                break;
            }
            px += GLYPH_W;
            at += c.len_utf8();
        }
        at
    }

    /// A click `x` pixels into `area` (as `draw` laid it out): the cursor
    /// there; with Shift, the selection stretched to there.
    pub fn click(&mut self, area: Rect, x: i32, shift: bool) {
        let at = self.place_at(x - area.x - 3, self.scroll(area));
        self.go(at, shift);
    }

    /// The mouse dragged to `x` with the button held: selects from where it
    /// was pressed.
    pub fn drag(&mut self, area: Rect, x: i32) {
        let at = self.place_at(x - area.x - 3, self.scroll(area));
        self.go(at, true);
    }

    /// A double click: the word there.
    pub fn select_word(&mut self, area: Rect, x: i32) {
        let at = self.place_at(x - area.x - 3, self.scroll(area));
        let start = self.word(self.next(at), true);
        let end = self.word(start, false);
        self.select(start, end);
    }

    /// Where the text shown in `area` starts, so the cursor is in sight.
    fn scroll(&self, area: Rect) -> usize {
        let room = ((area.w - 6) / GLYPH_W).max(1) as usize;
        let before = self.text[..self.cursor].chars().count();
        if before < room {
            return 0;
        }
        self.text.char_indices().nth(before + 1 - room).map_or(0, |(i, _)| i)
    }

    /// Drawn in `area`: white, the text, the selection in navy and the
    /// cursor, scrolled so the cursor shows.
    pub fn draw(&self, s: &mut Surface, area: Rect) {
        s.fill(area, color::WHITE);
        let from = self.scroll(area);
        let room = ((area.w - 6) / GLYPH_W).max(0) as usize;
        let shown: String = self.text[from..].chars().take(room).collect();
        let (y, x0) = (area.y + (area.h - GLYPH_H) / 2, area.x + 3);
        s.text(x0, y, &shown, color::BLACK);
        let col = |at: usize| x0 + text_width(&self.text[from..at.max(from)]);
        let (a, b) = self.selection();
        let end = from + shown.len();
        if a != b && b > from && a < end {
            let (sa, sb) = (a.max(from), b.min(end));
            let sel = &self.text[sa..sb];
            s.fill(Rect::new(col(sa), y - 1, text_width(sel), GLYPH_H + 2), color::NAVY);
            s.text(col(sa), y, sel, color::WHITE);
        }
        if self.cursor >= from && self.cursor <= end {
            s.vline(col(self.cursor), y - 1, GLYPH_H + 2, color::BLACK);
        }
    }
}

/// The clipboard every program shares (X's CLIPBOARD), through xclip.
pub mod clipboard {
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// What's on it, as text.
    pub fn get() -> Option<String> {
        use std::io::Read;
        // Read, then wait: a program started with its finished children
        // ignored can't be told how xclip ended, but still gets what it said.
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut text = Vec::new();
        child.stdout.take()?.read_to_end(&mut text).ok()?;
        let _ = child.wait();
        (!text.is_empty()).then(|| String::from_utf8_lossy(&text).into_owned())
    }

    /// Puts `text` on it (xclip stays to hand it to whoever pastes).
    pub fn set(text: &str) {
        let child =
            Command::new("xclip").args(["-selection", "clipboard", "-i"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = child {
            if let Some(mut input) = child.stdin.take() {
                let _ = input.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(e: &mut TextEdit, s: &str) {
        for c in s.chars() {
            e.key(Key::Char(c), false, false);
        }
    }

    #[test]
    fn types_over_the_selection() {
        let mut e = TextEdit::with_selection("Holiday.jpg", 0, 7);
        assert_eq!(e.selected(), "Holiday");
        typed(&mut e, "Beach");
        assert_eq!(e.text(), "Beach.jpg");
        assert_eq!(e.selection(), (5, 5));
    }

    #[test]
    fn shift_and_ctrl_move_and_select() {
        let mut e = TextEdit::new("my old file");
        e.key(Key::End, false, false);
        e.key(Key::Left, true, true);
        assert_eq!(e.selected(), "file");
        e.key(Key::Left, true, true);
        assert_eq!(e.selected(), "old file");
        e.key(Key::Home, false, true);
        assert_eq!(e.selected(), "my old file");
        e.key(Key::Right, false, false);
        assert_eq!((e.cursor(), e.selected()), (11, ""));
        e.key(Key::Char('a'), true, false);
        assert_eq!(e.selected(), "my old file");
    }

    #[test]
    fn letters_beyond_ascii_and_undo() {
        let mut e = TextEdit::new("café");
        e.key(Key::End, false, false);
        e.key(Key::Backspace, false, false);
        assert_eq!(e.text(), "caf");
        typed(&mut e, "é ünïcode");
        e.key(Key::Left, false, true);
        assert_eq!(e.selected(), "e");
        e.key(Key::Char('z'), true, false);
        assert_eq!(e.text(), "café ünïcod");
        e.undo();
        e.undo();
        e.undo();
        assert!(e.text().starts_with("caf"));
    }

    #[test]
    fn a_pasted_name_is_one_line() {
        let mut e = TextEdit::new("x");
        e.insert("first line\nsecond\tline");
        assert_eq!(e.text(), "first line");
        e.select_all();
        e.insert("tab\there");
        assert_eq!(e.text(), "tabhere");
    }

    #[test]
    fn clicks_land_between_letters() {
        let area = Rect::new(10, 0, 200, 14);
        let mut e = TextEdit::new("abcdef");
        e.click(area, 10 + 3 + 2 * GLYPH_W + 1, false);
        assert_eq!(e.cursor(), 2);
        e.drag(area, 10 + 3 + 5 * GLYPH_W);
        assert_eq!(e.selected(), "cde");
        e.select_word(area, 14);
        assert_eq!(e.selected(), "abcdef");
    }
}
