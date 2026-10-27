//! ferro-snip: FerroOS's Snipping Tool (Print Screen, or Shift+Windows+S).
//!
//! The screen is taken as it is, then shown dimmed over everything; drag a
//! box and what's inside it is kept. It asks where to save it, and saves a
//! PNG of just the pixels: no date, no machine or program name, no text of
//! any kind, nothing but the picture. Esc or a right-click cancels.

use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, text_field_with_cursor, Key, Keymap, Res};
use std::path::{Path, PathBuf};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const HINT: &str = "Drag a box around what to keep. Esc cancels.";

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-snip: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (w, h) = (usize::from(screen.width_in_pixels), usize::from(screen.height_in_pixels));
    // The screen as it is, before anything of ours is on it.
    let img = conn.get_image(ImageFormat::Z_PIXMAP, screen.root, 0, 0, w as u16, h as u16, !0)?.reply()?;
    if img.data.len() < w * h * 4 {
        return Err("the screen isn't 32 bits a pixel".into());
    }
    let shot: Vec<u32> = img.data.as_chunks::<4>().0.iter().take(w * h).map(|c| u32::from_le_bytes([c[0], c[1], c[2], 0])).collect();
    let Some(area) = select(&conn, &screen, &shot, w, h)? else { return Ok(()) };
    let picture = crop(&shot, w, area);
    save_dialog(&conn, &screen, area, &picture)
}

/// Sends `px` (`w` wide) to a window or pixmap, in pieces the server takes.
fn put(conn: &RustConnection, to: u32, gc: Gcontext, depth: u8, px: &[u32], w: usize, (x, y): (i16, i16)) -> Res<()> {
    let h = px.len() / w.max(1);
    let rows_per = (48 * 1024 / (w * 4)).max(1);
    let mut data = Vec::with_capacity(rows_per * w * 4);
    let mut row = 0;
    while row < h {
        let n = rows_per.min(h - row);
        data.clear();
        for p in &px[row * w..(row + n) * w] {
            data.extend_from_slice(&p.to_le_bytes());
        }
        conn.put_image(ImageFormat::Z_PIXMAP, to, gc, w as u16, n as u16, x, y + row as i16, 0, depth, &data)?;
        row += n;
    }
    Ok(())
}

/// The box between two corners, inclusive of both.
fn span(a: (i32, i32), b: (i32, i32)) -> Rect {
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    Rect::new(x0, y0, (a.0 - b.0).abs() + 1, (a.1 - b.1).abs() + 1)
}

/// The screen dimmed, with the hint along the top.
fn dimmed(shot: &[u32], w: usize, h: usize) -> Vec<u32> {
    let mut dim: Vec<u32> = shot.iter().map(|p| (p >> 1) & 0x7f7f7f).collect();
    let mut s = Surface::borrowed(&mut dim, w, h, w);
    let tw = text_width(HINT) + 24;
    let r = Rect::new((w as i32 - tw) / 2, 12, tw, GLYPH_H + 14);
    s.fill(r, color::FACE);
    s.bevel(r, Bevel::Raised);
    s.text(r.x + 12, r.y + 7, HINT, color::BLACK);
    dim
}

/// Shows the screen dimmed over everything and lets a box be dragged on it:
/// the box shows bright. The box, or `None` if cancelled.
fn select(conn: &RustConnection, screen: &Screen, shot: &[u32], w: usize, h: usize) -> Res<Option<Rect>> {
    let depth = screen.root_depth;
    let (bright, dim, gc, win) = (conn.generate_id()?, conn.generate_id()?, conn.generate_id()?, conn.generate_id()?);
    conn.create_pixmap(depth, bright, screen.root, w as u16, h as u16)?;
    conn.create_pixmap(depth, dim, screen.root, w as u16, h as u16)?;
    conn.create_gc(gc, bright, &CreateGCAux::new().graphics_exposures(0))?;
    put(conn, bright, gc, depth, shot, w, (0, 0))?;
    put(conn, dim, gc, depth, &dimmed(shot, w, h), w, (0, 0))?;
    // A crosshair, from the X server's own cursor font.
    let (font, cross) = (conn.generate_id()?, conn.generate_id()?);
    conn.open_font(font, b"cursor")?;
    conn.create_glyph_cursor(cross, font, font, 34, 35, 0, 0, 0, 0xffff, 0xffff, 0xffff)?;
    let events = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::KEY_PRESS | EventMask::EXPOSURE;
    // Its background is the dimmed screen: the server draws it, so moving
    // the box is only ever two copies.
    conn.create_window(
        depth,
        win,
        screen.root,
        0,
        0,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().override_redirect(1).background_pixmap(dim).cursor(cross).event_mask(events),
    )?;
    conn.map_window(win)?;
    ferro_ui::raise(conn, win)?;
    conn.flush()?;
    let pointer = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
    if !ferro_ui::grab_input(conn, win, pointer)? {
        conn.destroy_window(win)?;
        conn.flush()?;
        return Err("the keyboard and mouse are held by something else".into());
    }

    let mut start: Option<(i32, i32)> = None;
    let mut shown: Option<Rect> = None; // the box on screen, border included
    let result = loop {
        let mut event = Some(conn.wait_for_event()?);
        let mut moved_to = None;
        let mut done = None;
        // A whole batch of moves at once: only the last counts.
        while let Some(e) = event {
            match e {
                Event::KeyPress(k) if k.detail == 9 => done = Some(None), // Esc
                Event::ButtonPress(b) if b.detail == 3 => done = Some(None),
                Event::ButtonPress(b) if b.detail == 1 => start = Some((i32::from(b.root_x), i32::from(b.root_y))),
                Event::MotionNotify(m) if start.is_some() => moved_to = Some((i32::from(m.root_x), i32::from(m.root_y))),
                Event::ButtonRelease(b) if b.detail == 1 => {
                    if let Some(a) = start.take() {
                        let r = span(a, (i32::from(b.root_x), i32::from(b.root_y)));
                        // A click, not a box: still waiting for one.
                        if r.w >= 3 && r.h >= 3 {
                            done = Some(Some(r));
                        }
                    }
                }
                Event::Expose(_) => shown = None,
                _ => {}
            }
            event = conn.poll_for_event()?;
        }
        if let Some(d) = done {
            break d;
        }
        let (Some(a), Some(b)) = (start, moved_to) else { continue };
        let r = span(a, b);
        let framed = Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2);
        if let Some(old) = shown.take() {
            // The old box back to dimmed (its label below it too).
            conn.clear_area(false, win, old.x as i16, old.y as i16, old.w as u16, (old.h + GLYPH_H + 10) as u16)?;
        }
        conn.copy_area(bright, win, gc, r.x as i16, r.y as i16, r.x as i16, r.y as i16, r.w as u16, r.h as u16)?;
        conn.change_gc(gc, &ChangeGCAux::new().foreground(color::WHITE))?;
        conn.poly_rectangle(
            win,
            gc,
            &[Rectangle { x: framed.x as i16, y: framed.y as i16, width: (framed.w - 1) as u16, height: (framed.h - 1) as u16 }],
        )?;
        // Its size, under it.
        let label = format!("{} x {}", r.w, r.h);
        let lw = (label.len() as i32) * GLYPH_W + 8;
        let mut ls = Surface::new(lw as usize, (GLYPH_H + 6) as usize);
        ls.fill(Rect::new(0, 0, lw, GLYPH_H + 6), color::FACE);
        ls.text(4, 3, &label, color::BLACK);
        let ly = (framed.bottom() + 2).min(h as i32 - GLYPH_H - 6);
        put(conn, win, gc, depth, ls.pixels(), lw as usize, (framed.x as i16, ly as i16))?;
        shown = Some(Rect::new(framed.x, framed.y, framed.w.max(lw), framed.h));
        conn.flush()?;
    };
    ferro_ui::release_input(conn)?;
    conn.destroy_window(win)?;
    conn.free_pixmap(bright)?;
    conn.free_pixmap(dim)?;
    conn.free_cursor(cross)?;
    conn.close_font(font)?;
    conn.flush()?;
    Ok(result)
}

/// The pixels of `r`, out of the screen `w` wide.
fn crop(shot: &[u32], w: usize, r: Rect) -> Vec<u32> {
    let mut out = Vec::with_capacity((r.w * r.h) as usize);
    for y in r.y..r.bottom() {
        let row = y as usize * w;
        out.extend_from_slice(&shot[row + r.x as usize..row + r.right() as usize]);
    }
    out
}

/// A PNG of only the pixels: IHDR, IDAT and IEND, no text, time, size in
/// inches, colour profile or anything else a picture can carry.
fn encode_png(px: &[u32], w: u32, h: u32) -> Res<Vec<u8>> {
    use image::ImageEncoder;
    let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// `Screenshot 2026-10-27 09.15.02.png`, by the clock here.
fn default_name() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    #[cfg(unix)]
    {
        // SAFETY: localtime_r fills the zeroed struct it's given.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = now.try_into().unwrap_or(0);
        unsafe { libc::localtime_r(&t, &mut tm) };
        format!("Screenshot {:04}-{:02}-{:02} {:02}.{:02}.{:02}.png", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
    #[cfg(not(unix))]
    format!("Screenshot {now}.png")
}

/// Where a typed path is: `C:\Users\you\Pictures\a.png`, or `/home/...`, or
/// `~/...`; `.png` added if it has no extension.
fn resolve(typed: &str, drives: &ferro_path::DriveTable, home: &Path) -> Result<PathBuf, String> {
    let t = typed.trim();
    if t.is_empty() {
        return Err("Type where to save it.".into());
    }
    let mut p = if let Some(rest) = t.strip_prefix("~/") {
        home.join(rest)
    } else if t.starts_with('/') {
        PathBuf::from(t)
    } else if ferro_path::is_windows_path(t) {
        PathBuf::from(drives.to_posix(t).map_err(|_| format!("'{t}' isn't a place on this computer."))?)
    } else {
        return Err("Give the whole path, from the drive: C:\\...".into());
    };
    if p.file_name().is_none() || t.ends_with('\\') || t.ends_with('/') {
        return Err("Give it a name at the end.".into());
    }
    if p.extension().is_none() {
        p.set_extension("png");
    }
    Ok(p)
}

/// Writes the picture to `to`, never over something already there.
fn write_new(to: &Path, png: &[u8]) -> Result<(), String> {
    let dir = to.parent().filter(|d| d.is_dir()).ok_or("That folder doesn't exist.")?;
    if to.exists() {
        return Err("Something with that name is there already.".into());
    }
    let tmp = dir.join(format!(".ferro-snip-{}", std::process::id()));
    std::fs::write(&tmp, png).map_err(|e| format!("It couldn't be saved there: {e}"))?;
    // Linked into place: if a file of that name appeared meanwhile, it stays.
    let linked = std::fs::hard_link(&tmp, to).or_else(|_| {
        if to.exists() {
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        } else {
            std::fs::rename(&tmp, to)
        }
    });
    let _ = std::fs::remove_file(&tmp);
    linked.map_err(|e| format!("It couldn't be saved there: {e}"))
}

const DW: i32 = 460;
const DH: i32 = 132;
const TEXT_BOX: Rect = Rect::new(14, 36, DW - 28, 24);
const SAVE_BTN: Rect = Rect::new(DW - 175, 94, 75, 24);
const CANCEL_BTN: Rect = Rect::new(DW - 90, 94, 75, 24);

struct Field {
    text: Vec<char>,
    cursor: usize,
    /// All of it selected (as when the box opens): typing replaces it.
    all: bool,
}

impl Field {
    fn insert(&mut self, s: &str) {
        if self.all {
            self.text.clear();
            self.cursor = 0;
            self.all = false;
        }
        for c in s.chars().filter(|c| !c.is_control()) {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    fn key(&mut self, k: Key) {
        let n = self.text.len();
        match k {
            Key::Backspace | Key::Delete if self.all => {
                self.text.clear();
                self.cursor = 0;
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            Key::Delete if self.cursor < n => {
                self.text.remove(self.cursor);
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(n),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = n,
            Key::Char(c) => self.insert(&c.to_string()),
            _ => return,
        }
        self.all = false;
    }

    fn string(&self) -> String {
        self.text.iter().collect()
    }
}

/// Asks where to save the picture, and saves it there.
fn save_dialog(conn: &RustConnection, screen: &Screen, area: Rect, picture: &[u32]) -> Res<()> {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let drives = ferro_path::DriveTable::heavy();
    let pictures = home.join("Pictures");
    let folder = if pictures.is_dir() { pictures } else { home.clone() };
    let start = folder.join(default_name());
    let shown = drives.to_windows(&start.to_string_lossy()).unwrap_or_else(|| start.to_string_lossy().into_owned());
    let mut field = Field { cursor: shown.chars().count(), text: shown.chars().collect(), all: false };
    let mut error: Option<String> = None;
    let png = encode_png(picture, area.w as u32, area.h as u32)?;

    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - DW) / 2) as i16,
        ((sh - DH) / 2) as i16,
        DW as u16,
        DH as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "Save Screenshot";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, atom(b"_NET_WM_WINDOW_TYPE")?, AtomEnum::ATOM, &[atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?])?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-snip\0ferro-snip\0")?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(conn)?;

    let paint = |field: &Field, error: &Option<String>| -> Res<()> {
        let mut s = Surface::new(DW as usize, DH as usize);
        s.fill(Rect::new(0, 0, DW, DH), color::FACE);
        let room = ((DW - 28) / GLYPH_W) as usize;
        s.text(14, 16, &ellipsize(&format!("Save the screenshot ({} x {}) as:", area.w, area.h), room), color::BLACK);
        let sel = field.all.then_some((0, field.text.len()));
        text_field_with_cursor(&mut s, TEXT_BOX, &field.string(), field.cursor, sel, true);
        match error {
            Some(e) => s.text(14, 70, &ellipsize(e, room), color::RED),
            None => s.text(14, 70, &ellipsize("Saved as a plain PNG: no date, name or other details in it.", room), color::SHADOW),
        };
        button(&mut s, SAVE_BTN, "Save", true);
        button(&mut s, CANCEL_BTN, "Cancel", true);
        ferro_ui::focus_rect(&mut s, SAVE_BTN.inset(2));
        ferro_ui::show(conn, win, gc, screen.root_depth, &s, DW, DH)
    };

    let save = |field: &Field| -> Result<(), String> { write_new(&resolve(&field.string(), &drives, &home)?, &png) };

    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&field, &error)?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if SAVE_BTN.contains(x, y) {
                    match save(&field) {
                        Ok(()) => return Ok(()),
                        Err(e) => error = Some(e),
                    }
                } else if CANCEL_BTN.contains(x, y) {
                    return Ok(());
                } else if TEXT_BOX.contains(x, y) {
                    field.cursor = (((x - TEXT_BOX.x - 5) / GLYPH_W).max(0) as usize).min(field.text.len());
                    field.all = false;
                }
                paint(&field, &error)?;
            }
            Event::KeyPress(k) => {
                let ctrl = k.state.contains(KeyButMask::CONTROL);
                match keymap.typed(&k) {
                    Key::Enter => match save(&field) {
                        Ok(()) => return Ok(()),
                        Err(e) => error = Some(e),
                    },
                    Key::Escape => return Ok(()),
                    Key::Char('a' | 'A') if ctrl => field.all = true,
                    Key::Char('v' | 'V') if ctrl => {
                        if let Ok(out) = std::process::Command::new("xclip").args(["-selection", "clipboard", "-out"]).output() {
                            field.insert(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']));
                        }
                    }
                    _ if ctrl => {}
                    other => {
                        field.key(other);
                        error = None;
                    }
                }
                paint(&field, &error)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunks a PNG is made of, by name.
    fn chunks(png: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 8;
        while i + 8 <= png.len() {
            let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
            out.push(String::from_utf8_lossy(&png[i + 4..i + 8]).into_owned());
            i += 12 + len;
        }
        out
    }

    #[test]
    fn saves_only_the_pixels() {
        let png = encode_png(&[0xff0000, 0x00ff00, 0x0000ff, 0xffffff], 2, 2).unwrap();
        let names = chunks(&png);
        assert!(names.iter().all(|c| c == "IHDR" || c == "IDAT" || c == "IEND"), "{names:?}");
        let back = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(back.get_pixel(1, 0).0, [0, 255, 0]);
    }

    #[test]
    fn crops_the_box() {
        let shot: Vec<u32> = (0..16).collect();
        assert_eq!(crop(&shot, 4, Rect::new(1, 1, 2, 2)), vec![5, 6, 9, 10]);
        assert_eq!(span((5, 5), (2, 3)), Rect::new(2, 3, 4, 3));
    }

    #[test]
    fn where_to_save() {
        let home = Path::new("/home/you");
        let d = ferro_path::DriveTable::heavy();
        assert_eq!(resolve(r"C:\Users\you\Pictures\shot", &d, home).unwrap(), Path::new("/home/you/Pictures/shot.png"));
        assert_eq!(resolve("~/a.png", &d, home).unwrap(), Path::new("/home/you/a.png"));
        assert!(resolve("a.png", &d, home).is_err());
        assert!(resolve(r"C:\Users\you\", &d, home).is_err());
    }

    #[test]
    fn never_over_another_file() {
        let dir = std::env::temp_dir().join(format!("ferro-snip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("a.png");
        write_new(&to, b"one").unwrap();
        assert!(write_new(&to, b"two").is_err());
        assert_eq!(std::fs::read(&to).unwrap(), b"one");
        assert!(write_new(&dir.join("nowhere/b.png"), b"x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
