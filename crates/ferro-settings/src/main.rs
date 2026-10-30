//! ferro-settings: the Control Panel. Every part of FerroOS that has settings,
//! in one window, each a click away; and across the top, how private the
//! computer is right now (the switches, the VPN, Tor, the disk's encryption).

mod security;

use ferro_gfx::{color, icons, text_width, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::{Key, Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 804;
const H: i32 = 436;
const COLS: i32 = 3;
const TILE_W: i32 = 256;
const TILE_H: i32 = 76;
const GAP: i32 = 8;
const STATUS: Rect = Rect::new(10, 14, W - 20, 64);
const GRID_Y: i32 = 98;
const GREEN: u32 = 0x008000;

struct Tile {
    name: &'static str,
    about: [&'static str; 2],
    icon: icons::Icon,
    command: &'static str,
}

const TILES: [Tile; 12] = [
    Tile { name: "Network & Privacy", about: ["Wi-Fi, VPN, Tor", "Camera, Microphone"], icon: icons::NETWORK, command: "ferro-network" },
    Tile { name: "Appearance & Display", about: ["Wallpaper, Colours", "Screen"], icon: icons::COMPUTER, command: "ferro-appearance" },
    Tile { name: "Sound", about: ["Volume", "Speakers, Headphones"], icon: icons::SPEAKER, command: "ferro-volume" },
    Tile { name: "Date & Time", about: ["Clock", "Time zone"], icon: icons::CLOCK, command: "ferro-datetime" },
    Tile { name: "Default Programs", about: ["Open with", "File types"], icon: icons::FILE, command: "ferro-associations" },
    Tile { name: "Add Programs", about: ["Drivers, Steam", "Debian programs"], icon: icons::FOLDER, command: "ferro-terminal -e ferro-heavy menu" },
    Tile { name: "Updates", about: ["Update now", "What stays up to date"], icon: icons::FERRO, command: "ferro-updates --settings" },
    Tile { name: "Drives", about: ["Disks, USB drives", "Memory cards"], icon: icons::DRIVE, command: "ferro-files \"My Computer\"" },
    Tile { name: "Task Manager", about: ["Running programs", "Memory, CPU"], icon: icons::TASKMGR, command: "ferro-taskmgr" },
    Tile { name: "Security", about: ["Status", "Fixes"], icon: icons::FERRO, command: "ferro-settings --security" },
    Tile { name: "Backup", about: ["Back up to USB", "Automatic backup"], icon: icons::DRIVE, command: "ferro-backup" },
    Tile { name: "System Restore", about: ["Restore points", "Undo an update"], icon: icons::HELP, command: "ferro-terminal -e ferro-restore" },
];

fn tile_rect(i: usize) -> Rect {
    let (col, row) = (i as i32 % COLS, i as i32 / COLS);
    Rect::new(10 + col * (TILE_W + GAP), GRID_Y + row * (TILE_H + GAP), TILE_W, TILE_H)
}

fn tile_at(x: i32, y: i32) -> Option<usize> {
    (0..TILES.len()).find(|&i| tile_rect(i).contains(x, y))
}

/// One thing the status row tells: its name, what it is now, and whether
/// that's the private way round (None: neither, as Tor off with a VPN).
struct State {
    name: &'static str,
    now: &'static str,
    private: Option<bool>,
}

/// How private things are, from the switches as Network & Privacy last set
/// them (`switches.conf`) and how the system's disk is mounted (`mounts`).
fn states(switches: &str, mounts: &str) -> Vec<State> {
    let on = |key: &str, default: bool| {
        switches.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map_or(default, |(_, v)| v.trim() == "true")
    };
    let root = mounts.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|f| f.get(1) == Some(&"/"));
    let encrypted = root.is_some_and(|f| f[0].starts_with("/dev/mapper/") || f[0].starts_with("/dev/dm-"));
    let (mic, cam, vpn, tor) = (on("microphone", true), on("camera", true), on("vpn", false), on("tor", false));
    vec![
        State { name: "Microphone", now: if mic { "On" } else { "Off" }, private: Some(!mic) },
        State { name: "Camera", now: if cam { "On" } else { "Off" }, private: Some(!cam) },
        State { name: "VPN", now: if vpn { "On" } else { "Off" }, private: Some(vpn || tor) },
        State { name: "Tor", now: if tor { "On" } else { "Off" }, private: tor.then_some(true) },
        State { name: "Disk", now: if encrypted { "Encrypted" } else { "Not encrypted" }, private: Some(encrypted) },
    ]
}

fn read_states() -> Vec<State> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let switches = std::fs::read_to_string(home.join(".config/ferro/switches.conf")).unwrap_or_default();
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    states(&switches, &mounts)
}

pub(crate) fn launch(command: &str) {
    let _ = std::process::Command::new("/bin/sh").args(["-c", command]).stdin(std::process::Stdio::null()).spawn();
}

fn draw(s: &mut Surface, states: &[State], focus: usize, held: Option<usize>) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.group_box(STATUS, "Security and privacy");
    let mut x = STATUS.x + 12;
    for st in states {
        let y = STATUS.y + 16;
        s.text(x, y, st.name, color::BLACK);
        let c = match st.private {
            Some(true) => GREEN,
            Some(false) => color::RED,
            None => color::BLACK,
        };
        s.fill(Rect::new(x, y + GLYPH_H + 8, 8, 8), c);
        s.text_bold(x + 12, y + GLYPH_H + 8, st.now, c);
        x += text_width(st.name).max(12 + text_width(st.now) + 2) + 26;
    }
    s.text(STATUS.x + 12, STATUS.bottom() - GLYPH_H - 6, "Change...", color::NAVY);

    for (i, t) in TILES.iter().enumerate() {
        let r = tile_rect(i);
        s.bevel(r, if held == Some(i) { Bevel::Pressed } else { Bevel::Raised });
        s.sprite(r.x + 10, r.y + (r.h - 32) / 2, t.icon, 2);
        let tx = r.x + 52;
        if i == focus {
            let nw = text_width(t.name);
            s.fill(Rect::new(tx - 2, r.y + 13, nw + 4, GLYPH_H + 4), color::NAVY);
            s.text_bold(tx, r.y + 15, t.name, color::WHITE);
        } else {
            s.text_bold(tx, r.y + 15, t.name, color::BLACK);
        }
        for (n, line) in t.about.iter().enumerate() {
            s.text(tx, r.y + 34 + n as i32 * (GLYPH_H + 5), line, color::BLACK);
        }
    }
}

fn main() -> Res<()> {
    if std::env::args().nth(1).as_deref() == Some("--security") {
        return security::run();
    }
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::KEY_PRESS | EventMask::FOCUS_CHANGE;
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
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "Control Panel";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-settings\0ferro-settings\0")?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;
    let (mut states, mut focus, mut held) = (read_states(), 0usize, None::<usize>);

    let paint = |states: &[State], focus: usize, held: Option<usize>| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, states, focus, held);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&states, focus, held)?,
            // Back from changing something: what it is now.
            Event::FocusIn(_) => {
                states = read_states();
                paint(&states, focus, held)?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) if b.detail == 1 => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if STATUS.contains(x, y) {
                    launch("ferro-network");
                } else if let Some(i) = tile_at(x, y) {
                    (focus, held) = (i, Some(i));
                    paint(&states, focus, held)?;
                }
            }
            Event::ButtonRelease(b) if b.detail == 1 => {
                if let Some(i) = held.take() {
                    if tile_at(i32::from(b.event_x), i32::from(b.event_y)) == Some(i) {
                        launch(TILES[i].command);
                    }
                    paint(&states, focus, held)?;
                }
            }
            Event::KeyPress(k) => {
                let n = TILES.len();
                let cols = COLS as usize;
                focus = match keymap.typed(&k) {
                    Key::Escape => return Ok(()),
                    Key::Enter | Key::Char(' ') => {
                        launch(TILES[focus].command);
                        focus
                    }
                    Key::Left => focus.saturating_sub(1),
                    Key::Right | Key::Next => (focus + 1) % n,
                    Key::Up => focus.checked_sub(cols).unwrap_or(focus),
                    Key::Down if focus + cols < n => focus + cols,
                    Key::Home => 0,
                    Key::End => n - 1,
                    _ => focus,
                };
                paint(&states, focus, held)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fit_the_window() {
        for (i, t) in TILES.iter().enumerate() {
            let r = tile_rect(i);
            assert!(r.right() <= W - 10 && r.bottom() <= H - 10, "tile {i} is outside");
            assert_eq!(tile_at(r.x + 1, r.y + 1), Some(i));
            assert!(52 + text_width(t.name) + 2 <= TILE_W - 4, "{:?} is too long", t.name);
            for line in t.about {
                assert!(52 + text_width(line) <= TILE_W - 4, "{line:?} is too long");
            }
        }
        assert_eq!(tile_at(5, GRID_Y + 2), None);
    }

    #[test]
    fn what_the_status_row_says() {
        let switches = "network = true\nmicrophone = false\ncamera = true\nvpn = true\ntor = false\n";
        let mounts = "/dev/mapper/ferro-system / ext4 rw 0 0\n";
        let s = states(switches, mounts);
        let said: Vec<(&str, &str, Option<bool>)> = s.iter().map(|s| (s.name, s.now, s.private)).collect();
        assert_eq!(
            said,
            [
                ("Microphone", "Off", Some(true)),
                ("Camera", "On", Some(false)),
                ("VPN", "On", Some(true)),
                ("Tor", "Off", None),
                ("Disk", "Encrypted", Some(true))
            ]
        );
        // Never set: as the switches start (on), and a plain disk.
        let s = states("", "/dev/sda2 / ext4 rw 0 0\n");
        assert_eq!((s[0].now, s[0].private), ("On", Some(false)));
        assert_eq!((s[2].now, s[2].private), ("Off", Some(false)));
        assert_eq!((s[4].now, s[4].private), ("Not encrypted", Some(false)));
    }
}
