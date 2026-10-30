//! Settings > Updates: Update now, and which parts of FerroOS updates keep
//! up to date. A part that's unticked is held back (ferro-update-hold):
//! updates leave it as it is, and one that was uninstalled stays away.

use ferro_gfx::{color, ellipsize, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, Keymap, Res};
use std::process::{Command, Stdio};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const HOLD: &str = "/usr/local/sbin/ferro-update-hold";

const W: i32 = 500;
const ROWS_Y: i32 = 84;
const ROW_H: i32 = 22;

/// A part of FerroOS that updates bring, as `ferro-update-hold list` tells.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Part {
    pub name: String,
    pub label: String,
    pub held: bool,
    pub installed: bool,
}

/// `name|label|held|installed`, a line each.
pub(crate) fn parse(list: &str) -> Vec<Part> {
    list.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('|').collect();
            (f.len() == 4 && !f[0].is_empty()).then(|| Part {
                name: f[0].to_owned(),
                label: f[1].to_owned(),
                held: f[2] == "yes",
                installed: f[3] == "yes",
            })
        })
        .collect()
}

fn parts() -> Vec<Part> {
    // FERRO_UPDATE_HOLD: another ferro-update-hold to list from (for trying this window out).
    let hold = std::env::var("FERRO_UPDATE_HOLD").unwrap_or_else(|_| HOLD.to_owned());
    Command::new(hold).arg("list").stderr(Stdio::null()).output().map(|o| parse(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// What a part's row says about it, after its name.
pub(crate) fn note(part: &Part, ticked: bool) -> &'static str {
    match (part.installed, ticked) {
        (true, true) => "",
        (true, false) => " (left as it is)",
        (false, true) => " (the next update adds it)",
        (false, false) => " (removed)",
    }
}

fn height(rows: usize) -> i32 {
    ROWS_Y + rows.max(1) as i32 * ROW_H + 70
}

fn row_rect(i: usize) -> Rect {
    Rect::new(24, ROWS_Y + i as i32 * ROW_H, W - 48, ROW_H)
}

/// Each change, as `ferro-update-hold`: without a password where the sudo
/// rule allows it, otherwise in a terminal that asks for it.
fn apply(parts: &[Part], ticked: &[bool]) {
    let mut asked = Vec::new();
    for (part, &on) in parts.iter().zip(ticked) {
        if on == !part.held {
            continue;
        }
        let verb = if on { "release" } else { "hold" };
        let quiet = Command::new("sudo").args(["-n", HOLD, verb, &part.name]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if !quiet.map(|s| s.success()).unwrap_or(false) {
            asked.push(format!("sudo {HOLD} {verb} {}", part.name));
        }
    }
    if !asked.is_empty() {
        let script = asked.join(" && ");
        let _ = Command::new("ferro-terminal").args(["--hold", "-e", "sh", "-c", &script]).stdin(Stdio::null()).spawn();
    }
}

pub(crate) fn run() -> Res<()> {
    let parts = parts();
    let mut ticked: Vec<bool> = parts.iter().map(|p| !p.held).collect();
    let h = height(parts.len());
    let update_btn = Rect::new(16, h - 38, 100, 24);
    let ok_btn = Rect::new(W - 175, h - 38, 75, 24);
    let cancel_btn = Rect::new(W - 90, h - 38, 75, 24);

    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values = CreateWindowAux::new()
        .background_pixel(color::FACE)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY);
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - h) / 2) as i16,
        W as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "Updates";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
    ferro_ui::fixed_size(&conn, win, W, h)?;
    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut s = Surface::new(W as usize, h as usize);
    let room = ((W - 60) / GLYPH_W).max(1) as usize;
    let paint = |s: &mut Surface, ticked: &[bool]| -> Res<()> {
        s.fill(Rect::new(0, 0, W, h), color::FACE);
        s.sprite(16, 14, icons::FERRO, 2);
        s.text(56, 16, "Update now brings Debian's packages and FerroOS's", color::BLACK);
        s.text(56, 30, "own programs up to date.", color::BLACK);
        s.text(16, 60, "Updates also keep these up to date (untick to leave one be):", color::BLACK);
        if parts.is_empty() {
            s.text(24, ROWS_Y + 6, "(nothing here can be left out)", color::SHADOW);
        }
        for (i, part) in parts.iter().enumerate() {
            let r = row_rect(i);
            let check = Rect::new(r.x, r.y + 3, 14, 14);
            s.fill(check, color::WHITE);
            s.bevel(check, Bevel::Sunken);
            if ticked[i] {
                s.text_bold(check.x + 3, check.y + 3, "x", color::BLACK);
            }
            let text = format!("{}{}", part.label, note(part, ticked[i]));
            s.text(r.x + 22, r.y + (r.h - GLYPH_H) / 2, &ellipsize(&text, room), color::BLACK);
        }
        button(s, update_btn, "Update now", true);
        default_button(s, ok_btn, "OK", true);
        button(s, cancel_btn, "Cancel", true);
        ferro_ui::show(&conn, win, gc, screen.root_depth, s, W, h)
    };
    paint(&mut s, &ticked)?;
    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => paint(&mut s, &ticked)?,
            Event::MapNotify(_) => {
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == delete => return Ok(()),
            Event::ButtonPress(bp) => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if let Some(i) = (0..parts.len()).find(|&i| row_rect(i).contains(x, y)) {
                    ticked[i] = !ticked[i];
                    paint(&mut s, &ticked)?;
                } else if ok_btn.contains(x, y) {
                    apply(&parts, &ticked);
                    return Ok(());
                } else if cancel_btn.contains(x, y) {
                    return Ok(());
                } else if update_btn.contains(x, y) {
                    apply(&parts, &ticked);
                    crate::run_in_terminal(&["ferro-heavy".into(), "update".into()]);
                    return Ok(());
                }
            }
            Event::KeyPress(kp) => match dialog_answer(keymap.typed(&kp)) {
                Some(true) => {
                    apply(&parts, &ticked);
                    return Ok(());
                }
                Some(false) => return Ok(()),
                None => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list() {
        let parts = parse("wisp|Wisp, the web browser|no|yes\nbad line\nx|X|yes|no\n");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], Part { name: "wisp".into(), label: "Wisp, the web browser".into(), held: false, installed: true });
        assert!(parts[1].held && !parts[1].installed);
        assert!(parse("").is_empty());
    }

    #[test]
    fn says_what_happens() {
        let p = |installed| Part { name: "w".into(), label: "W".into(), held: false, installed };
        assert_eq!(note(&p(true), true), "");
        assert_eq!(note(&p(true), false), " (left as it is)");
        assert_eq!(note(&p(false), true), " (the next update adds it)");
        assert_eq!(note(&p(false), false), " (removed)");
    }

    #[test]
    fn fits() {
        let line = "Updates also keep these up to date (untick to leave one be):";
        assert!(16 + line.len() as i32 * GLYPH_W <= W);
        assert!(56 + "Update now brings Debian's packages and FerroOS's".len() as i32 * GLYPH_W <= W);
    }
}
