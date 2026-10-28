//! The Security page (ferro-settings --security): each thing that keeps the
//! computer safe, whether it's as it should be, and a button to put right
//! what can be, as Windows Security has it.

use ferro_gfx::{color, ellipsize, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 620;
const ROW_H: i32 = 46;

pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub said: String,
    /// The button that puts it right, and what it runs.
    pub fix: Option<(&'static str, &'static str)>,
}

/// What's to check, from what the system says about itself: `mounts` is
/// /proc/mounts, `switches` the Network & Privacy switches, `firewall`
/// whether nftables runs, `updated_days` since the package lists were
/// fetched, `lockdown` and `apparmor` the kernel's own files.
pub fn checks(mounts: &str, switches: &str, firewall: bool, updated_days: Option<u64>, lockdown: &str, apparmor: &str) -> Vec<Check> {
    let on = |key: &str| switches.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).is_some_and(|(_, v)| v.trim() == "true");
    let root = mounts.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|f| f.get(1) == Some(&"/"));
    let encrypted = root.is_some_and(|f| f[0].starts_with("/dev/mapper/") || f[0].starts_with("/dev/dm-"));
    let (mic, cam, vpn) = (on("microphone"), on("camera"), on("vpn"));
    let privacy = Some(("Change...", "ferro-network"));
    vec![
        Check { name: "Disk encryption", ok: encrypted, said: if encrypted { "Encrypted".into() } else { "Not encrypted".into() }, fix: None },
        Check {
            name: "Firewall",
            ok: firewall,
            said: if firewall { "On".into() } else { "Off".into() },
            fix: (!firewall).then_some(("Turn on", "ferro-terminal -e sudo systemctl start nftables")),
        },
        Check {
            name: "Updates",
            ok: updated_days.is_some_and(|d| d < 7),
            said: match updated_days {
                Some(0) => "Checked today".into(),
                Some(1) => "Checked yesterday".into(),
                Some(d) => format!("Checked {d} days ago"),
                None => "Never checked".into(),
            },
            fix: Some(("Update now", "ferro-terminal -e ferro-heavy update")),
        },
        Check { name: "Microphone", ok: !mic, said: if mic { "On".into() } else { "Off".into() }, fix: privacy },
        Check { name: "Camera", ok: !cam, said: if cam { "On".into() } else { "Off".into() }, fix: privacy },
        Check { name: "VPN", ok: vpn, said: if vpn { "On".into() } else { "Off".into() }, fix: privacy },
        Check {
            name: "Kernel lockdown",
            ok: lockdown.contains("[integrity]") || lockdown.contains("[confidentiality]"),
            said: if lockdown.contains("[none]") || lockdown.is_empty() { "Off".into() } else { "On".into() },
            fix: None,
        },
        Check { name: "AppArmor", ok: apparmor.trim() == "Y", said: if apparmor.trim() == "Y" { "On".into() } else { "Off".into() }, fix: None },
    ]
}

fn read_checks() -> Vec<Check> {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let switches = std::fs::read_to_string(home.join(".config/ferro/switches.conf")).unwrap_or_default();
    let firewall = std::process::Command::new("systemctl").args(["is-active", "--quiet", "nftables"]).status().is_ok_and(|s| s.success());
    let updated = std::fs::metadata("/var/lib/apt/lists").and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs() / 86400);
    checks(
        &read("/proc/mounts"),
        &switches,
        firewall,
        updated,
        &read("/sys/kernel/security/lockdown"),
        &read("/sys/module/apparmor/parameters/enabled"),
    )
}

fn row(i: usize) -> Rect {
    Rect::new(10, 40 + i as i32 * ROW_H, W - 20, ROW_H - 4)
}

fn fix_button(i: usize) -> Rect {
    let r = row(i);
    Rect::new(r.right() - 110, r.y + (r.h - 26) / 2, 100, 26)
}

fn draw(s: &mut Surface, checks: &[Check], h: i32) {
    s.fill(Rect::new(0, 0, W, h), color::FACE);
    let bad = checks.iter().filter(|c| !c.ok).count();
    let head = match bad {
        0 => "All good".to_string(),
        1 => "1 needs attention".to_string(),
        n => format!("{n} need attention"),
    };
    s.text_bold(14, 14, &head, if bad == 0 { 0x008000 } else { color::RED });
    for (i, c) in checks.iter().enumerate() {
        let r = row(i);
        s.fill(r, color::WHITE);
        s.bevel(r, Bevel::Sunken);
        let mark = if c.ok { 0x008000 } else { color::RED };
        s.fill(Rect::new(r.x + 10, r.y + (r.h - 12) / 2, 12, 12), mark);
        s.text_bold(r.x + 32, r.y + 8, c.name, color::BLACK);
        let room = ((r.w - 32 - 124) / GLYPH_W) as usize;
        s.text(r.x + 32, r.y + 8 + GLYPH_H + 6, &ellipsize(&c.said, room), color::BLACK);
        if let Some((label, _)) = c.fix {
            ferro_ui::button(s, fix_button(i), label, true);
        }
    }
}

pub fn run() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let mut list = read_checks();
    let h = row(list.len()).y + 6;
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::FOCUS_CHANGE;
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
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "Security";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-settings\0ferro-settings\0")?;
    ferro_ui::fixed_size(&conn, win, W, h)?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;
    let paint = |list: &[Check]| -> Res<()> {
        let mut s = Surface::new(W as usize, h as usize);
        draw(&mut s, list, h);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, h)
    };
    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&list)?,
            // Back from putting something right: how it is now.
            Event::FocusIn(_) => {
                list = read_checks();
                paint(&list)?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) if b.detail == 1 => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if let Some((_, command)) = (0..list.len()).find(|&i| fix_button(i).contains(x, y)).and_then(|i| list[i].fix) {
                    super::launch(command);
                }
            }
            Event::KeyPress(k) if keymap.typed(&k) == Key::Escape => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_needs_a_look() {
        let mounts = "/dev/mapper/ferro-system / ext4 rw 0 0\n";
        let c = checks(mounts, "microphone = false\ncamera = true\nvpn = true\n", true, Some(2), "none [integrity] confidentiality", "Y\n");
        let bad: Vec<&str> = c.iter().filter(|c| !c.ok).map(|c| c.name).collect();
        assert_eq!(bad, ["Camera"]);
        let c = checks("/dev/sda1 / ext4 rw 0 0\n", "", false, None, "[none] integrity", "N");
        assert!(c.iter().all(|c| !c.ok || c.name == "Microphone" || c.name == "Camera"));
        assert_eq!(c[2].said, "Never checked");
        assert!(c[1].fix.is_some());
        for i in 0..c.len() {
            assert!(fix_button(i).x > 32 + 30 * GLYPH_W);
        }
    }
}
