//! ferro-flyout: what opens over the tray when its icons are clicked, as on
//! Windows. A click anywhere else, or Escape, closes it.
//!
//!     ferro-flyout volume X    the volume slider and Mute
//!     ferro-flyout network X   the connection, and the Network, VPN, Tor,
//!                              Microphone and Camera switches
//!
//! X is where the icon is, across the screen.

use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// The taskbar's height (ferro-wm's bar::HEIGHT).
const BAR_H: i32 = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Volume,
    Network,
}

fn size(kind: Kind) -> (i32, i32) {
    match kind {
        Kind::Volume => (96, 230),
        Kind::Network => (300, 250),
    }
}

// ---- Volume ------------------------------------------------------------

fn volume() -> (u8, bool) {
    let out = Command::new("amixer").args(["sget", "Master"]).stderr(Stdio::null()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let vol = text.split_whitespace().find_map(|t| t.strip_prefix('[')?.strip_suffix("%]")?.parse().ok()).unwrap_or(75);
    (vol, text.contains("[off]"))
}

fn set_volume(vol: u8, muted: bool) {
    let quiet = |cmd: &mut Command| {
        let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    quiet(Command::new("amixer").args(["-q", "sset", "Master", &format!("{vol}%"), if muted { "mute" } else { "unmute" }]));
    quiet(Command::new("pactl").args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{vol}%")]));
    quiet(Command::new("pactl").args(["set-sink-mute", "@DEFAULT_SINK@", if muted { "1" } else { "0" }]));
}

const TRACK: Rect = Rect::new(40, 42, 16, 122);

fn vol_at(y: i32) -> u8 {
    (100 - ((y - TRACK.y) * 100 / TRACK.h).clamp(0, 100)) as u8
}

fn mute_box() -> Rect {
    Rect::new(16, 176, 64, 16)
}

fn mixer_button() -> Rect {
    Rect::new(10, 198, 76, 24)
}

fn draw_volume(s: &mut Surface, vol: u8, muted: bool) {
    let (w, _) = size(Kind::Volume);
    s.text((w - text_width("Volume")) / 2, 10, "Volume", color::BLACK);
    s.text((w - text_width(&format!("{vol}%"))) / 2, 22, &format!("{vol}%"), color::SHADOW);
    s.bevel(Rect::new(TRACK.x + 6, TRACK.y, 4, TRACK.h), Bevel::Sunken);
    let ty = TRACK.y + (100 - i32::from(vol)) * (TRACK.h - 10) / 100;
    let thumb = Rect::new(TRACK.x - 6, ty, TRACK.w + 12, 10);
    s.fill(thumb, color::FACE);
    s.bevel(thumb, Bevel::Raised);
    let b = mute_box();
    s.fill(Rect::new(b.x, b.y + 2, 12, 12), color::WHITE);
    s.bevel(Rect::new(b.x, b.y + 2, 12, 12), Bevel::Sunken);
    if muted {
        s.text(b.x + 2, b.y + 4, "x", color::BLACK);
    }
    s.text(b.x + 18, b.y + 4, "Mute", color::BLACK);
    ferro_ui::button(s, mixer_button(), "Mixer...", true);
}

// ---- Network -----------------------------------------------------------

/// The switches as the Network & Privacy window keeps them.
fn switches_path() -> std::path::PathBuf {
    std::env::var_os("HOME").map_or_else(|| "/etc/ferro".into(), |h| std::path::PathBuf::from(h).join(".config/ferro")).join("switches.conf")
}

fn switch_on(text: &str, key: &str) -> bool {
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).is_some_and(|(_, v)| v.trim() == "true")
}

/// `key` set to `on` in the switches file's `text` (added if it isn't there).
fn with_switch(text: &str, key: &str, on: bool) -> String {
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| match l.split_once('=') {
            Some((k, _)) if k.trim() == key => {
                found = true;
                format!("{key} = {on}")
            }
            _ => l.to_owned(),
        })
        .collect();
    if !found {
        out.push(format!("{key} = {on}"));
    }
    out.join("\n") + "\n"
}

/// The five switches, in the order the tiles show them.
const SWITCHES: [(&str, &str); 5] = [("network", "Network"), ("vpn", "VPN"), ("tor", "Tor"), ("microphone", "Microphone"), ("camera", "Camera")];

fn flip(key: &str) {
    let path = switches_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let on = !switch_on(&text, key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, with_switch(&text, key, on));
    let mut store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    match key {
        "network" => store.switches.network = on,
        "vpn" => store.switches.vpn = on,
        "tor" => store.switches.tor = on,
        "microphone" => store.switches.microphone = on,
        _ => store.switches.camera = on,
    }
    let _ = store.save();
    let state = if on { "on" } else { "off" };
    let (tool, what): (&str, Vec<&str>) = match key {
        "vpn" => ("/usr/local/sbin/ferro-vpn", vec![state]),
        "tor" => ("/usr/local/sbin/ferro-tor", vec![state]),
        "network" => ("/usr/local/sbin/ferro-switch-hardware", vec!["net", state]),
        "microphone" => ("/usr/local/sbin/ferro-switch-hardware", vec!["mic", state]),
        _ => ("/usr/local/sbin/ferro-switch-hardware", vec!["cam", state]),
    };
    // VPN and Tor take a few seconds: on their own, while this stays open.
    let _ = Command::new("sudo").arg("-n").arg(tool).args(what).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// What the network panel says: the connection, the VPN, Tor.
fn status_lines() -> [String; 3] {
    let route = Command::new("ip").args(["-4", "route", "get", "1.1.1.1"]).stderr(Stdio::null()).output();
    let route = route.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let word_after = |w: &str| route.split_whitespace().skip_while(|t| *t != w).nth(1).map(str::to_owned);
    let net = match (word_after("dev"), word_after("src")) {
        (Some(dev), Some(ip)) => format!("Connected: {dev}, {ip}"),
        _ => "Not connected".into(),
    };
    let field = |file: &str, k: &str| {
        std::fs::read_to_string(file).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_owned()))).unwrap_or_default()
    };
    let vpn = match field("/run/ferro/vpn-status", "state=").as_str() {
        "connected" => format!("VPN: {}", field("/run/ferro/vpn-status", "detail=")),
        "" | "disconnected" => "VPN: off".into(),
        other => format!("VPN: {other}"),
    };
    let tor = match field("/run/ferro/tor-status", "state=").as_str() {
        "" | "off" => "Tor: off".into(),
        other => format!("Tor: {other}"),
    };
    [net, vpn, tor]
}

fn tile(i: usize) -> Rect {
    let (col, row) = (i as i32 % 3, i as i32 / 3);
    Rect::new(10 + col * 94, 92 + row * 52, 88, 46)
}

fn settings_button() -> Rect {
    Rect::new(10, 210, 280, 28)
}

fn draw_network(s: &mut Surface, lines: &[String; 3], switches: &str) {
    let room = ((300 - 24) / GLYPH_W) as usize;
    for (k, line) in lines.iter().enumerate() {
        let bold = k == 0;
        let y = 12 + k as i32 * (GLYPH_H + 8);
        if bold {
            s.text_bold(12, y, &ellipsize(line, room), color::BLACK);
        } else {
            s.text(12, y, &ellipsize(line, room), color::BLACK);
        }
    }
    s.etched_hline(8, 80, 284);
    for (i, (key, name)) in SWITCHES.iter().enumerate() {
        let r = tile(i);
        let on = switch_on(switches, key);
        s.fill(r, if on { color::NAVY } else { color::FACE });
        s.bevel(r, if on { Bevel::Pressed } else { Bevel::Raised });
        let fg = if on { color::WHITE } else { color::BLACK };
        s.text_bold(r.x + (r.w - text_width(name)) / 2, r.y + 10, name, fg);
        let state = if on { "On" } else { "Off" };
        s.text(r.x + (r.w - text_width(state)) / 2, r.y + 26, state, fg);
    }
    ferro_ui::button(s, settings_button(), "Network & Privacy settings...", true);
}

// ---- The window -------------------------------------------------------

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kind = match args.first().map(String::as_str) {
        Some("volume") => Kind::Volume,
        Some("network") => Kind::Network,
        _ => {
            eprintln!("usage: ferro-flyout volume|network X");
            std::process::exit(2);
        }
    };
    let anchor: i32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(i32::MAX);
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let (w, h) = size(kind);
    let x = (anchor - w / 2).clamp(0, (sw - w).max(0));
    let y = (sh - BAR_H - h - 2).max(0);
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION | EventMask::KEY_PRESS;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        x as i16,
        y as i16,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(mask),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
    conn.flush()?;
    // Clicks anywhere come here: one outside closes it.
    let grab_mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION;
    for _ in 0..20 {
        let ok = conn.grab_pointer(true, win, grab_mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)?.reply()?;
        if ok.status == GrabStatus::SUCCESS {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = conn.grab_keyboard(true, win, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply();
    let keymap = Keymap::load(&conn)?;

    let (mut vol, mut muted) = if kind == Kind::Volume { volume() } else { (0, false) };
    let mut lines = if kind == Kind::Network { status_lines() } else { Default::default() };
    let mut switches = std::fs::read_to_string(switches_path()).unwrap_or_default();
    let mut dragging = false;
    let mut last_look = Instant::now();

    let paint = |vol: u8, muted: bool, lines: &[String; 3], switches: &str| -> Res<()> {
        let mut s = Surface::new(w as usize, h as usize);
        s.fill(Rect::new(0, 0, w, h), color::FACE);
        s.bevel(Rect::new(0, 0, w, h), Bevel::Window);
        match kind {
            Kind::Volume => draw_volume(&mut s, vol, muted),
            Kind::Network => draw_network(&mut s, lines, switches),
        }
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, w, h)
    };
    let close = |command: Option<&str>| -> Res<()> {
        conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
        conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        conn.destroy_window(win)?;
        conn.flush()?;
        if let Some(c) = command {
            let _ = Command::new("/bin/sh").args(["-c", c]).stdin(Stdio::null()).spawn();
        }
        Ok(())
    };
    paint(vol, muted, &lines, &switches)?;

    loop {
        let Some(event) = conn.poll_for_event()? else {
            // The network's state, every second while it's open.
            if kind == Kind::Network && last_look.elapsed() > Duration::from_secs(1) {
                last_look = Instant::now();
                let now = (status_lines(), std::fs::read_to_string(switches_path()).unwrap_or_default());
                if now.0 != lines || now.1 != switches {
                    (lines, switches) = now;
                    paint(vol, muted, &lines, &switches)?;
                }
            }
            std::thread::sleep(Duration::from_millis(30));
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => paint(vol, muted, &lines, &switches)?,
            Event::ButtonPress(b) => {
                let (px, py) = (i32::from(b.event_x), i32::from(b.event_y));
                if !(0..w).contains(&px) || !(0..h).contains(&py) {
                    return close(None);
                }
                match (kind, b.detail) {
                    (Kind::Volume, 4 | 5) => {
                        vol = if b.detail == 4 { vol.saturating_add(5).min(100) } else { vol.saturating_sub(5) };
                        set_volume(vol, muted);
                    }
                    (Kind::Volume, 1) if Rect::new(TRACK.x - 10, TRACK.y - 6, TRACK.w + 20, TRACK.h + 12).contains(px, py) => {
                        vol = vol_at(py);
                        dragging = true;
                        set_volume(vol, muted);
                    }
                    (Kind::Volume, 1) if mute_box().contains(px, py) => {
                        muted = !muted;
                        set_volume(vol, muted);
                    }
                    (Kind::Volume, 1) if mixer_button().contains(px, py) => return close(Some("ferro-volume")),
                    (Kind::Network, 1) if settings_button().contains(px, py) => return close(Some("ferro-network")),
                    (Kind::Network, 1) => {
                        if let Some(i) = (0..SWITCHES.len()).find(|&i| tile(i).contains(px, py)) {
                            flip(SWITCHES[i].0);
                            switches = std::fs::read_to_string(switches_path()).unwrap_or_default();
                        }
                    }
                    _ => continue,
                }
                paint(vol, muted, &lines, &switches)?;
            }
            Event::MotionNotify(m) if dragging => {
                let v = vol_at(i32::from(m.event_y));
                if v != vol {
                    vol = v;
                    set_volume(vol, muted);
                    paint(vol, muted, &lines, &switches)?;
                }
            }
            Event::ButtonRelease(_) => dragging = false,
            Event::KeyPress(k) => match keymap.typed(&k) {
                Key::Escape | Key::Enter => return close(None),
                Key::Up | Key::Right if kind == Kind::Volume => {
                    vol = vol.saturating_add(2).min(100);
                    set_volume(vol, muted);
                    paint(vol, muted, &lines, &switches)?;
                }
                Key::Down | Key::Left if kind == Kind::Volume => {
                    vol = vol.saturating_sub(2);
                    set_volume(vol, muted);
                    paint(vol, muted, &lines, &switches)?;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_file_kept_as_it_was() {
        let text = "# FerroOS Hardware & Privacy Switches\nnetwork = true\nmicrophone = false\n";
        assert!(switch_on(text, "network") && !switch_on(text, "microphone") && !switch_on(text, "vpn"));
        let t = with_switch(text, "microphone", true);
        assert_eq!(t, "# FerroOS Hardware & Privacy Switches\nnetwork = true\nmicrophone = true\n");
        let t = with_switch(&t, "vpn", true);
        assert!(t.ends_with("vpn = true\n") && switch_on(&t, "vpn"));
    }

    #[test]
    fn the_slider_reads_top_to_bottom() {
        assert_eq!(vol_at(TRACK.y), 100);
        assert_eq!(vol_at(TRACK.bottom()), 0);
        assert_eq!(vol_at(TRACK.y - 50), 100);
        for i in 0..SWITCHES.len() {
            assert!(tile(i).right() <= 300 - 4 && tile(i).bottom() < settings_button().y);
        }
    }
}
