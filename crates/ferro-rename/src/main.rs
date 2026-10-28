//! ferro-rename: FerroOS Heavy's clean retro Rename Dialog.
//!
//! Provides a consistent, Windows 95-themed rename dialog for both Desktop
//! icons and File Explorer files/folders. Supports arrow key navigation,
//! cursor editing, text selection, and clipboard copy/paste (Ctrl+C, Ctrl+V,
//! Ctrl+X, Ctrl+A) without having to delete the entire name.

use ferro_gfx::{color, Rect, Surface, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, text_field_with_cursor, Key, Keymap, Res};
use std::env;
use std::path::{Path, PathBuf};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 380;
const H: i32 = 138;
const TEXT_BOX: Rect = Rect::new(14, 34, W - 28, 24);
const OK_BTN: Rect = Rect::new(W - 175, 96, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 90, 96, 75, 24);

fn copy_to_clipboard(text: &str) {
    if let Ok(mut child) = std::process::Command::new("xclip").args(["-selection", "clipboard", "-in"]).stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn paste_from_clipboard() -> Option<String> {
    if let Ok(out) = std::process::Command::new("xclip").args(["-selection", "clipboard", "-out"]).output() {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let trimmed = s.trim_matches(|c| c == '\r' || c == '\n');
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

struct State {
    target_path: PathBuf,
    parent_dir: PathBuf,
    original_name: String,
    text: String,
    cursor: usize,
    selection: Option<(usize, usize)>,
    error: Option<String>,
}

impl State {
    fn new(path: PathBuf) -> Self {
        let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let parent = abs.parent().unwrap_or_else(|| Path::new("/")).to_path_buf();
        let original_name = abs.file_name().unwrap_or_default().to_string_lossy().into_owned();

        // Place cursor before the extension if it's a file with an extension,
        // otherwise at the end of the text.
        let cursor = if abs.is_file() || original_name.contains('.') {
            if let Some(dot_pos) = original_name.rfind('.') {
                if dot_pos > 0 {
                    dot_pos
                } else {
                    original_name.chars().count()
                }
            } else {
                original_name.chars().count()
            }
        } else {
            original_name.chars().count()
        };

        Self { target_path: abs, parent_dir: parent, text: original_name.clone(), original_name, cursor, selection: None, error: None }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.selection.take() {
            let (start, end) = (a.min(b), a.max(b));
            let mut chars: Vec<char> = self.text.chars().collect();
            if start < chars.len() {
                let actual_end = end.min(chars.len());
                chars.drain(start..actual_end);
                self.text = chars.into_iter().collect();
                self.cursor = start;
                return true;
            }
        }
        false
    }

    fn insert_text(&mut self, s: &str) {
        self.delete_selection();
        let mut chars: Vec<char> = self.text.chars().collect();
        let cur = self.cursor.min(chars.len());
        for (i, ch) in s.chars().enumerate() {
            if ch != '/' && ch != '\0' && ch != '\r' && ch != '\n' {
                chars.insert(cur + i, ch);
            }
        }
        self.cursor = cur + s.chars().filter(|&c| c != '/' && c != '\0' && c != '\r' && c != '\n').count();
        self.text = chars.into_iter().collect();
        self.error = None;
    }

    fn submit(&mut self) -> bool {
        let new_name = self.text.trim();
        if new_name.is_empty() {
            self.error = Some("A file name cannot be empty.".into());
            return false;
        }
        if new_name.contains('/') || new_name.contains('\0') {
            self.error = Some("A file name cannot contain '/'.".into());
            return false;
        }
        if new_name == self.original_name {
            return true;
        }
        let dest = self.parent_dir.join(new_name);
        if dest.exists() {
            self.error = Some(format!("'{new_name}' already exists."));
            return false;
        }
        match std::fs::rename(&self.target_path, &dest) {
            Ok(()) => true,
            Err(e) => {
                self.error = Some(format!("Error: {e}"));
                false
            }
        }
    }
}

fn draw(s: &mut Surface, state: &State) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);

    // Label
    let label = format!("Rename '{}' to:", state.original_name);
    let room = ((W - 28) / GLYPH_W).max(1) as usize;
    s.text(14, 14, &ferro_gfx::ellipsize(&label, room), color::BLACK);

    // Text box with cursor & selection
    text_field_with_cursor(s, TEXT_BOX, &state.text, state.cursor, state.selection, true);

    // Error or hint message
    if let Some(err) = &state.error {
        let room = ((W - 28) / GLYPH_W).max(1) as usize;
        s.text(14, 66, &ferro_gfx::ellipsize(err, room), color::RED);
    }

    // OK and Cancel buttons
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
}

fn main() -> Res<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: ferro-rename <path-to-file-or-folder>");
        std::process::exit(1);
    }

    let target_path = PathBuf::from(&args[1]);
    let mut state = State::new(target_path);

    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (screen_w, screen_h) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));

    let win_x = (screen_w - W) / 2;
    let win_y = (screen_h - H) / 2;

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;

    let values = CreateWindowAux::new()
        .background_pixel(color::FACE)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY);

    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        win_x as i16,
        win_y as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;

    // Window title and dialog atoms
    let wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    let net_wm_window_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let net_wm_window_type_dialog = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE_DIALOG")?.reply()?.atom;

    let title = "Rename";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8_string, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, net_wm_window_type, AtomEnum::ATOM, &[net_wm_window_type_dialog])?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete_window])?;

    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut surface = Surface::new(W as usize, H as usize);

    let repaint = |surface: &mut Surface, state: &State| -> Res<()> {
        draw(surface, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, surface, W, H)?;
        Ok(())
    };

    repaint(&mut surface, &state)?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(_) => {
                repaint(&mut surface, &state)?;
            }
            Event::ClientMessage(cm) => {
                if cm.data.as_data32()[0] == wm_delete_window {
                    std::process::exit(1);
                }
            }
            Event::ButtonPress(bp) => {
                let (px, py) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if OK_BTN.contains(px, py) {
                    if state.submit() {
                        std::process::exit(0);
                    }
                    ferro_ui::sound("error");
                    repaint(&mut surface, &state)?;
                } else if CANCEL_BTN.contains(px, py) {
                    std::process::exit(1);
                } else if TEXT_BOX.contains(px, py) {
                    // Position cursor by click X
                    let rel_x = px - (TEXT_BOX.x + 5);
                    let char_idx = (rel_x / GLYPH_W).max(0) as usize;
                    state.cursor = char_idx.min(state.text.chars().count());
                    state.selection = None;
                    repaint(&mut surface, &state)?;
                }
            }
            Event::KeyPress(kp) => {
                let ctrl = kp.state.contains(KeyButMask::CONTROL);
                let shift = kp.state.contains(KeyButMask::SHIFT);
                let k = keymap.typed(&kp);

                if ctrl {
                    match k {
                        Key::Char('a' | 'A') => {
                            state.selection = Some((0, state.text.chars().count()));
                            state.cursor = state.text.chars().count();
                        }
                        Key::Char('c' | 'C') => {
                            if let Some((a, b)) = state.selection {
                                let (start, end) = (a.min(b), a.max(b));
                                let chars: Vec<char> = state.text.chars().collect();
                                let sel_str: String = chars[start..end.min(chars.len())].iter().collect();
                                copy_to_clipboard(&sel_str);
                            } else {
                                copy_to_clipboard(&state.text);
                            }
                        }
                        Key::Char('x' | 'X') => {
                            if let Some((a, b)) = state.selection {
                                let (start, end) = (a.min(b), a.max(b));
                                let chars: Vec<char> = state.text.chars().collect();
                                let sel_str: String = chars[start..end.min(chars.len())].iter().collect();
                                copy_to_clipboard(&sel_str);
                                state.delete_selection();
                            } else {
                                copy_to_clipboard(&state.text);
                                state.text.clear();
                                state.cursor = 0;
                            }
                        }
                        Key::Char('v' | 'V') => {
                            if let Some(pasted) = paste_from_clipboard() {
                                state.insert_text(&pasted);
                            }
                        }
                        _ => {}
                    }
                    repaint(&mut surface, &state)?;
                    continue;
                }

                match k {
                    _ if dialog_answer(k) == Some(true) => {
                        if state.submit() {
                            std::process::exit(0);
                        }
                        ferro_ui::sound("error");
                    }
                    _ if dialog_answer(k) == Some(false) => {
                        std::process::exit(1);
                    }
                    Key::Left => {
                        if shift {
                            let anchor = state.selection.map(|(a, _)| a).unwrap_or(state.cursor);
                            let new_cur = state.cursor.saturating_sub(1);
                            state.cursor = new_cur;
                            state.selection = Some((anchor, new_cur));
                        } else {
                            state.selection = None;
                            state.cursor = state.cursor.saturating_sub(1);
                        }
                    }
                    Key::Right => {
                        let total = state.text.chars().count();
                        if shift {
                            let anchor = state.selection.map(|(a, _)| a).unwrap_or(state.cursor);
                            let new_cur = (state.cursor + 1).min(total);
                            state.cursor = new_cur;
                            state.selection = Some((anchor, new_cur));
                        } else {
                            state.selection = None;
                            state.cursor = (state.cursor + 1).min(total);
                        }
                    }
                    Key::Home => {
                        if shift {
                            let anchor = state.selection.map(|(a, _)| a).unwrap_or(state.cursor);
                            state.cursor = 0;
                            state.selection = Some((anchor, 0));
                        } else {
                            state.selection = None;
                            state.cursor = 0;
                        }
                    }
                    Key::End => {
                        let total = state.text.chars().count();
                        if shift {
                            let anchor = state.selection.map(|(a, _)| a).unwrap_or(state.cursor);
                            state.cursor = total;
                            state.selection = Some((anchor, total));
                        } else {
                            state.selection = None;
                            state.cursor = total;
                        }
                    }
                    Key::Backspace => {
                        if !state.delete_selection() && state.cursor > 0 {
                            let mut chars: Vec<char> = state.text.chars().collect();
                            chars.remove(state.cursor - 1);
                            state.cursor -= 1;
                            state.text = chars.into_iter().collect();
                        }
                        state.error = None;
                    }
                    Key::Delete => {
                        if !state.delete_selection() {
                            let mut chars: Vec<char> = state.text.chars().collect();
                            if state.cursor < chars.len() {
                                chars.remove(state.cursor);
                                state.text = chars.into_iter().collect();
                            }
                        }
                        state.error = None;
                    }
                    Key::Char(c) if !c.is_control() => {
                        let s = c.to_string();
                        state.insert_text(&s);
                    }
                    _ => {}
                }
                repaint(&mut surface, &state)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_positioning() {
        let state1 = State::new(PathBuf::from("/tmp/test_file.txt"));
        assert_eq!(state1.cursor, 9); // before ".txt"
        assert_eq!(state1.text, "test_file.txt");

        let state2 = State::new(PathBuf::from("/tmp/my_folder"));
        assert_eq!(state2.cursor, 9); // at the end of "my_folder"
    }

    #[test]
    fn text_editing_and_selection() {
        let mut s = State::new(PathBuf::from("/tmp/document.pdf"));
        // "document.pdf", cursor is at 8 (before ".pdf")
        assert_eq!(s.cursor, 8);
        s.insert_text("_v2");
        assert_eq!(s.text, "document_v2.pdf");
        assert_eq!(s.cursor, 11);

        // Select all and replace
        s.selection = Some((0, s.text.chars().count()));
        s.insert_text("new_name");
        assert_eq!(s.text, "new_name");
        assert_eq!(s.cursor, 8);
    }
}
