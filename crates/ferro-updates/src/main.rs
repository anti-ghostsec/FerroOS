//! ferro-updates: the box that says an update is ready, so it's never
//! forgotten.
//!
//!   ferro-updates --watch          the desktop starts this at logon: a
//!                                  minute in, then every six hours, it asks
//!                                  GitHub for the newest release and, if
//!                                  it's newer than this PC's, offers it
//!   ferro-updates --check          the same, once, now
//!   ferro-updates --settings       Settings > Updates: Update now, and
//!                                  which parts of FerroOS updates keep up
//!                                  to date (settings.rs)
//!   ferro-updates --ask LINE1 LINE2 COMMAND [ARGS]
//!                                  the same box for anything else (a new
//!                                  build made on this PC): Install runs
//!                                  COMMAND in a terminal
//!
//! Install opens a terminal running `ferro-heavy update` (Debian's packages
//! and FerroOS's own programs, as Settings' Updates does), which asks for
//! the password there. Later closes the box; that release isn't offered
//! again until the next logon. With the network switched off, or GitHub
//! out of reach, nothing is shown.
//!
//! "Look for updates on their own" in Settings > Updates (`automatic =
//! false` in ~/.config/ferro/updates.conf) stops --watch asking: it looks
//! at the setting each round, so unticking it counts at once.

mod settings;

use ferro_gfx::{color, ellipsize, icons, Rect, Surface, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, Keymap, Res};
use std::process::{Command, Stdio};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const LATEST: &str = "https://github.com/anti-ghostsec/FerroOS/releases/latest";
/// The release this PC's FerroOS programs came from (written by the image
/// build, brought along by each update).
const RELEASE_FILE: &str = "/usr/local/share/ferro/release";

/// Where "Look for updates on their own" is kept, for this user.
pub(crate) fn conf_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config/ferro/updates.conf"))
}

/// Whether updates.conf leaves --watch to look (yes, unless it says
/// `automatic = false`).
pub(crate) fn automatic_in(conf: &str) -> bool {
    !conf.lines().any(|l| {
        let mut kv = l.splitn(2, '=');
        let (k, v) = (kv.next().unwrap_or("").trim(), kv.next().unwrap_or("").trim());
        k == "automatic" && (v == "0" || v.eq_ignore_ascii_case("false"))
    })
}

pub(crate) fn automatic() -> bool {
    conf_path().and_then(|p| std::fs::read_to_string(p).ok()).is_none_or(|c| automatic_in(&c))
}

pub(crate) fn set_automatic(on: bool) {
    let Some(path) = conf_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("# Settings > Updates\nautomatic = {on}\n"));
}

const W: i32 = 400;
const H: i32 = 130;
const INSTALL_BTN: Rect = Rect::new(W - 175, 90, 75, 24);
const LATER_BTN: Rect = Rect::new(W - 90, 90, 75, 24);

/// "v1.0.31" as numbers, to put releases in order; None if it isn't one.
fn version(tag: &str) -> Option<Vec<u32>> {
    let nums: Option<Vec<u32>> = tag.trim().trim_start_matches(['v', 'V']).split('.').map(|n| n.parse().ok()).collect();
    nums.filter(|n| !n.is_empty())
}

/// Whether `latest` is worth offering to a PC that has `have` (or doesn't
/// know what it has: then any release is).
fn newer(latest: &str, have: Option<&str>) -> bool {
    match (version(latest), have.and_then(version)) {
        (Some(l), Some(h)) => l > h,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The tag GitHub's "latest" page sends us on to (…/releases/tag/v1.0.32).
fn tag_from_redirect(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/tag/")?.1;
    (!tag.is_empty()).then(|| tag.to_owned())
}

fn latest_tag() -> Option<String> {
    let out = Command::new("curl")
        .args(["--proto", "=https", "--tlsv1.2", "-fsS", "--max-time", "20", "-o", "/dev/null", "-w", "%{redirect_url}", LATEST])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| tag_from_redirect(&String::from_utf8_lossy(&out.stdout))).flatten()
}

fn installed_tag() -> Option<String> {
    std::fs::read_to_string(RELEASE_FILE).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Shows the box; true if Install was pressed.
fn ask(title: &str, lines: [&str; 2]) -> Res<bool> {
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
        ((sh - H) / 2) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let (wtype, dialog) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?);
    let (state, above) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?);
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[dialog])?;
    conn.change_property32(PropMode::REPLACE, win, state, AtomEnum::ATOM, &[above])?;
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
    conn.map_window(win)?;
    conn.flush()?;
    ferro_ui::sound("ding");

    let keymap = Keymap::load(&conn)?;
    let mut s = Surface::new(W as usize, H as usize);
    let room = ((W - 70) / GLYPH_W).max(1) as usize;
    let mut repaint = || -> Res<()> {
        s.fill(Rect::new(0, 0, W, H), color::FACE);
        s.sprite(16, 18, icons::FERRO, 2);
        s.text(60, 22, &ellipsize(lines[0], room), color::BLACK);
        s.text(60, 44, &ellipsize(lines[1], room), color::BLACK);
        default_button(&mut s, INSTALL_BTN, "Install", true);
        button(&mut s, LATER_BTN, "Later", true);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    repaint()?;
    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => repaint()?,
            Event::MapNotify(_) => {
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == delete => return Ok(false),
            Event::ButtonPress(bp) => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if INSTALL_BTN.contains(x, y) {
                    return Ok(true);
                } else if LATER_BTN.contains(x, y) {
                    return Ok(false);
                }
            }
            Event::KeyPress(kp) => {
                if let Some(answer) = dialog_answer(keymap.typed(&kp)) {
                    return Ok(answer);
                }
            }
            _ => {}
        }
    }
}

/// COMMAND in FerroOS's progress window (ferro-programs), which stays open
/// once it's done, so what it did can be read; `root` asks for the password
/// in the password box first.
fn run_in_window(title: &str, root: bool, command: &[String]) {
    let mut c = Command::new("ferro-programs");
    c.args(["--run", title]);
    if root {
        c.arg("--root");
    }
    let _ = c.arg("--").args(command).stdin(Stdio::null()).spawn();
}

/// Everything updated (ferro-heavy update), in the progress window.
fn update_now() {
    run_in_window("Updating FerroOS", true, &["/usr/local/bin/ferro-heavy".into(), "--gui".into(), "update".into()]);
}

/// A newer release than this PC's: offered, unless it already was. True if
/// it was offered (whatever the answer).
fn check(offered: &mut Option<String>) -> Res<bool> {
    let Some(latest) = latest_tag() else { return Ok(false) };
    let have = installed_tag();
    if !newer(&latest, have.as_deref()) || offered.as_deref() == Some(latest.as_str()) {
        return Ok(false);
    }
    *offered = Some(latest.clone());
    let first = format!("FerroOS {latest} is ready to install.");
    let second = match &have {
        Some(have) => format!("This PC has {have}. Install it now?"),
        None => "Install it on this PC now?".to_owned(),
    };
    if ask("FerroOS Update", [&first, &second])? {
        update_now();
    }
    Ok(true)
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--ask") if args.len() >= 4 => {
            if ask("FerroOS Update", [&args[1], &args[2]])? {
                run_in_window("FerroOS Update", false, &args[3..]);
                return Ok(());
            }
            std::process::exit(1);
        }
        Some("--settings") => settings::run()?,
        Some("--check") => {
            check(&mut None)?;
        }
        Some("--watch") => {
            let mut offered = None;
            std::thread::sleep(Duration::from_secs(60));
            loop {
                // A box that can't be shown (the desktop restarting) isn't
                // the end: the next round tries again.
                if automatic() {
                    let _ = check(&mut offered);
                }
                std::thread::sleep(Duration::from_secs(6 * 60 * 60));
            }
        }
        _ => {
            eprintln!("usage: ferro-updates --watch | --check | --settings | --ask LINE1 LINE2 COMMAND [ARGS]");
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_in_order() {
        assert!(newer("v1.0.32", Some("v1.0.31")));
        assert!(newer("v1.0.100", Some("v1.0.99")));
        assert!(!newer("v1.0.31", Some("v1.0.31")));
        assert!(!newer("v1.0.30", Some("v1.0.31")));
        assert!(newer("v1.0.32", None));
        assert!(newer("v1.0.32", Some("unknown")));
        assert!(!newer("nightly", Some("v1.0.31")));
    }

    #[test]
    fn automatic_unless_turned_off() {
        assert!(automatic_in(""));
        assert!(automatic_in("automatic = true\n"));
        assert!(!automatic_in("# Settings > Updates\nautomatic = false\n"));
        assert!(!automatic_in("automatic=0"));
        assert!(automatic_in("# automatic = false"));
    }

    #[test]
    fn tag_from_githubs_redirect() {
        let url = "https://github.com/anti-ghostsec/FerroOS/releases/tag/v1.0.31\n";
        assert_eq!(tag_from_redirect(url).as_deref(), Some("v1.0.31"));
        assert_eq!(tag_from_redirect(""), None);
        assert_eq!(tag_from_redirect("https://github.com/anti-ghostsec/FerroOS/releases/tag/"), None);
    }
}
