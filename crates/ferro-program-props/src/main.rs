//! ferro-program-props: a Start menu program's Properties (right-click it
//! in the Start menu). Shows what it starts and where it's installed, picks
//! the Start menu folder it's listed in, and has its Uninstall button.
//!
//!     ferro-program-props /usr/share/applications/NAME.desktop
//!     ferro-program-props --refresh    the Start menu and desktop re-read

// The Start menu's own reading of .desktop files, so the folder shown here
// is the one the Start menu uses.
#[path = "../../ferro-wm/src/apps.rs"]
#[allow(dead_code)]
mod apps;

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, FocusKey, Key, Keymap, Res};
use std::path::Path;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 380;
const H: i32 = 340;
const OK_BTN: Rect = Rect::new(W - 245, H - 34, 72, 24);
const CANCEL_BTN: Rect = Rect::new(W - 165, H - 34, 72, 24);
const APPLY_BTN: Rect = Rect::new(W - 85, H - 34, 72, 24);
const UNINSTALL_BTN: Rect = Rect::new(12, H - 34, 96, 24);
const FOLDERS_Y: i32 = 176;

/// The radio button for Start menu folder `i` (two columns of three).
fn folder_rect(i: usize) -> Rect {
    let (col, row) = ((i / 3) as i32, (i % 3) as i32);
    Rect::new(28 + col * 170, FOLDERS_Y + 24 + row * 24, 160, 20)
}

/// Tab order: the six folders, Uninstall, OK, Cancel, Apply.
fn controls() -> Vec<Rect> {
    let mut c: Vec<Rect> = (0..apps::CATEGORIES.len()).map(folder_rect).collect();
    c.extend([UNINSTALL_BTN, OK_BTN, CANCEL_BTN, APPLY_BTN]);
    c
}

struct State {
    app: apps::App,
    installed: String,
    folder: usize,
    saved: usize,
    focus: Option<usize>,
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let result = if arg == "--refresh" { refresh() } else { run(&arg) };
    if let Err(e) = result {
        eprintln!("ferro-program-props: {e}");
        std::process::exit(1);
    }
}

/// Asks the window manager to read the Start menu and the desktop again.
fn refresh() -> Res<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = conn.intern_atom(false, b"_FERRO_REFRESH")?.reply()?.atom;
    let msg = ClientMessageEvent::new(32, root, atom, [0u32; 5]);
    conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT, msg)?;
    conn.flush()?;
    Ok(())
}

/// Where a program came from, in words.
fn installed_where(file: &Path) -> String {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    if let Some(id) = text.lines().find_map(|l| l.strip_prefix("X-Ferro-Program=")) {
        let conf = std::fs::read_to_string(format!("/var/lib/ferro/programs/{}.conf", id.trim())).unwrap_or_default();
        if let Some(dir) = conf.lines().find_map(|l| l.strip_prefix("dir=")) {
            return format!("C:{}", dir.replace('/', "\\"));
        }
        if let Some(p) = conf.lines().find_map(|l| l.strip_prefix("package=")) {
            return format!("Debian package {p}");
        }
    }
    let out = std::process::Command::new("dpkg-query").arg("-S").arg(file).output();
    if let Some(pkg) = out.ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).split(':').next().map(str::to_owned)) {
        return format!("Debian package {pkg}");
    }
    if file.starts_with(std::env::var("HOME").unwrap_or_default()) {
        "Your own Start menu entry".into()
    } else {
        "Part of FerroOS".into()
    }
}

/// Saves the chosen folder for this program's entry.
fn save_folder(file: &str, folder: &str) -> std::io::Result<()> {
    let Some(path) = apps::categories_path() else { return Ok(()) };
    let base = Path::new(file).file_name().unwrap_or_default().to_string_lossy().into_owned();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| l.split_once('=').map(|(f, _)| f.trim()) != Some(base.as_str())).map(str::to_owned).collect();
    lines.push(format!("{base}={folder}"));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, lines.join("\n") + "\n")
}

fn draw(s: &mut Surface, st: &State) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let frame = Rect::new(8, 8, W - 16, H - 52);
    s.bevel(frame, Bevel::Sunken);
    s.sprite(20, 20, icons::FERRO, 2);
    s.text_bold(64, 30, &ellipsize(&st.app.name, 36), color::BLACK);
    s.hline(18, 64, W - 36, color::SHADOW);
    s.hline(18, 65, W - 36, color::WHITE);
    let room = ((W - 120) / GLYPH_W) as usize;
    let rows = [("Type:", "Program".to_string()), ("Starts:", st.app.exec.clone()), ("Installed:", st.installed.clone())];
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = 78 + i as i32 * 22;
        s.text(20, y, label, color::BLACK);
        s.text(110, y, &ellipsize(value, room), color::BLACK);
    }
    s.hline(18, FOLDERS_Y - 8, W - 36, color::SHADOW);
    s.hline(18, FOLDERS_Y - 7, W - 36, color::WHITE);
    s.text(20, FOLDERS_Y, "Start menu folder (Programs):", color::BLACK);
    for (i, name) in apps::CATEGORIES.iter().enumerate() {
        let r = folder_rect(i);
        let dot = Rect::new(r.x, r.y + 3, 13, 13);
        s.fill(dot, color::WHITE);
        s.bevel(dot, Bevel::Sunken);
        if st.folder == i {
            s.fill(Rect::new(dot.x + 4, dot.y + 4, 5, 5), color::BLACK);
        }
        s.sprite(r.x + 20, r.y + 2, apps::category_icon(name), 1);
        s.text(r.x + 40, r.y + 5, name, color::BLACK);
    }
    button(s, UNINSTALL_BTN, "Uninstall...", true);
    button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", st.folder != st.saved);
    if let Some(r) = st.focus.and_then(|i| controls().get(i).copied()) {
        let r =
            if r.w == 160 { Rect::new(r.x + 38, r.y + 1, text_width(apps::CATEGORIES[st.focus.unwrap_or(0)]) + 6, GLYPH_H + 6) } else { r.inset(3) };
        ferro_ui::focus_rect(s, r);
    }
}

fn run(file: &str) -> Res<()> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let mut app = apps::parse_desktop(&text).ok_or("not a program's entry")?;
    app.file = file.to_owned();
    if let Some(c) = apps::load_categories().get(Path::new(file).file_name().unwrap_or_default().to_string_lossy().as_ref()) {
        app.category = c.clone();
    }
    let folder = apps::CATEGORIES.iter().position(|c| *c == app.category).unwrap_or(0);
    let mut st = State { installed: installed_where(Path::new(file)), app, folder, saved: folder, focus: None };

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
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
        &CreateWindowAux::new().background_pixel(screen.white_pixel).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    let title = format!("{} Properties", st.app.name);
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    let keymap = Keymap::load(&conn)?;

    let paint = |st: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, st);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    let apply = |st: &mut State| -> Res<()> {
        if st.folder != st.saved {
            save_folder(&st.app.file, apps::CATEGORIES[st.folder])?;
            st.saved = st.folder;
            refresh()?;
        }
        Ok(())
    };
    paint(&st)?;
    let mut pending: Option<Event> = None;
    loop {
        let event = match pending.take() {
            Some(e) => e,
            None => conn.wait_for_event()?,
        };
        match event {
            Event::Expose(e) if e.count == 0 => paint(&st)?,
            Event::ClientMessage(e) if e.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                // Enter is OK, unless a button has the keyboard (then it's
                // that button), as in Windows.
                let on_button = st.focus.is_some_and(|f| f >= apps::CATEGORIES.len());
                if key == Key::Enter && !on_button {
                    apply(&mut st)?;
                    return Ok(());
                }
                match (key, ferro_ui::focus_key(&e, key, st.focus, &controls())) {
                    (Key::Escape, _) => return Ok(()),
                    (_, FocusKey::Moved(f)) => {
                        st.focus = f;
                        paint(&st)?;
                    }
                    (_, FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                    (Key::Up | Key::Down, _) => {
                        let n = apps::CATEGORIES.len();
                        st.folder = if key == Key::Up { (st.folder + n - 1) % n } else { (st.folder + 1) % n };
                        paint(&st)?;
                    }
                    _ => {}
                }
            }
            Event::ButtonPress(e) if e.detail == 1 => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                if let Some(i) = (0..apps::CATEGORIES.len()).find(|&i| folder_rect(i).contains(x, y)) {
                    st.folder = i;
                    paint(&st)?;
                } else if OK_BTN.contains(x, y) {
                    apply(&mut st)?;
                    return Ok(());
                } else if CANCEL_BTN.contains(x, y) {
                    return Ok(());
                } else if APPLY_BTN.contains(x, y) {
                    apply(&mut st)?;
                    paint(&st)?;
                } else if UNINSTALL_BTN.contains(x, y) {
                    let q = ferro_associations::shell_quote(&st.app.file);
                    let _ =
                        std::process::Command::new("sh").arg("-c").arg(format!("ferro-terminal -e /usr/local/bin/ferro-uninstall {q} &")).status();
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_do_not_overlap_and_fit() {
        let all: Vec<Rect> = (0..apps::CATEGORIES.len()).map(folder_rect).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(a.right() < W - 8 && a.bottom() < UNINSTALL_BTN.y, "folder {i} fits");
            for b in &all[i + 1..] {
                assert!(a.intersect(b).w <= 0 || a.intersect(b).h <= 0);
            }
        }
    }
}
