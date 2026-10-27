//! ferro-info: the Info window. What a picture or video carries, to see,
//! change or strip (see the library). Opened from the Image Viewer's and the
//! Video Player's Info; Properties has the same in its Info tab.
//!
//!     ferro-info FILE                 the window
//!     ferro-info --via-desktop FILE   asks the desktop to open it (from
//!                                     inside a sandbox that may start nothing)

use ferro_gfx::{color, Rect, Surface};
use ferro_info::{Action, Panel};
use ferro_ui::{Key, Keymap, Res};
use std::path::{Path, PathBuf};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 640;
const H: i32 = 460;
const AREA: Rect = Rect::new(10, 10, W - 20, H - 20);

/// Shows `text` in the text editor, from a file kept in memory (the
/// session's runtime folder), never on the disk.
fn open_text(text: &str) {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let file = dir.join(format!("ferro-info-{}.txt", std::process::id()));
    if std::fs::write(&file, text).is_ok() {
        let _ = std::process::Command::new("geany").arg("-i").arg(&file).spawn();
    }
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (via_desktop, file) = match args.as_slice() {
        [flag, f] if flag == "--via-desktop" => (true, f),
        [f] => (false, f),
        _ => {
            eprintln!("usage: ferro-info [--via-desktop] FILE");
            std::process::exit(2);
        }
    };
    let path = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    if via_desktop {
        return ferro_info::ask_desktop(&conn, screen.root, &path);
    }
    run(&conn, &screen, &path)
}

fn run(conn: &RustConnection, screen: &Screen, path: &Path) -> Res<()> {
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - H) / 2) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let name = path.file_name().map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy());
    let title = format!("Info - {name}");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-info\0ferro-info\0")?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(conn)?;
    let mut panel = Panel::new(path);
    let mut last_click = 0u32;

    let paint = |panel: &Panel| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        s.fill(Rect::new(0, 0, W, H), color::FACE);
        panel.draw(&mut s, AREA);
        ferro_ui::show(conn, win, gc, screen.root_depth, &s, W, H)
    };
    let act = |a: Action, panel: &Panel| -> Res<()> {
        match a {
            Action::None => Ok(()),
            Action::Redraw => paint(panel),
            Action::OpenText(t) => {
                open_text(&t);
                Ok(())
            }
        }
    };

    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&panel)?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) => {
                let a = match b.detail {
                    4 | 5 => panel.wheel(AREA, b.detail == 4),
                    1 => {
                        let double = b.time.wrapping_sub(last_click) < 400;
                        last_click = b.time;
                        panel.click(AREA, i32::from(b.event_x), i32::from(b.event_y), double)
                    }
                    _ => Action::None,
                };
                act(a, &panel)?;
            }
            Event::KeyPress(k) => {
                let key = keymap.typed(&k);
                let (taken, a) = panel.key(AREA, key);
                if !taken && key == Key::Escape {
                    return Ok(());
                }
                act(a, &panel)?;
            }
            _ => {}
        }
    }
}
