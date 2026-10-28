//! ferro-datetime: Retro Windows 95 Date & Time Properties.
//!
//! Shows the local date and time, kept right automatically (from internet
//! time servers, or from the Tor network while Tor is on), and sets the
//! time zone: chosen with < and >, applied on OK through systemd's
//! timedated, which harden.sh lets administrators use.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{FocusKey, Keymap, Res};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};
use view::{days_in_month, draw, hit, zone_index, Hit, State, CONTROLS, DEFAULT, H, W, ZONES};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

/// The local time now: (year, month, day, hour, minute, second).
fn local_now() -> (i32, u32, u32, u32, u32, u32) {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    #[cfg(unix)]
    {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r fills the zeroed struct it's given.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32);
        }
    }
    // UTC, from the seconds alone.
    let (s, m, h) = ((secs % 60) as u32, ((secs / 60) % 60) as u32, ((secs / 3600) % 24) as u32);
    let mut rem = (secs / 86400) as i32;
    let mut y = 1970;
    while rem >= if view::is_leap_year(y) { 366 } else { 365 } {
        rem -= if view::is_leap_year(y) { 366 } else { 365 };
        y += 1;
    }
    let mut mon = 1;
    while rem >= days_in_month(y, mon) as i32 {
        rem -= days_in_month(y, mon) as i32;
        mon += 1;
    }
    (y, mon, (rem + 1) as u32, h, m, s)
}

/// The system's time zone: what /etc/localtime points at.
fn current_zone() -> String {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let t = target.to_string_lossy();
        if let Some(i) = t.find("zoneinfo/") {
            return t[i + "zoneinfo/".len()..].to_owned();
        }
    }
    std::fs::read_to_string("/etc/timezone").map(|z| z.trim().to_owned()).ok().filter(|z| !z.is_empty()).unwrap_or_else(|| "Etc/UTC".into())
}

/// Whether the clock is set automatically, and whether it has been yet.
fn sync_state() -> (bool, bool) {
    let out = Command::new("timedatectl").args(["show", "-p", "NTP", "-p", "NTPSynchronized"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let tor = std::fs::read_to_string("/run/ferro/tor-status").is_ok_and(|t| t.contains("state=ready"));
    // With Tor on, ferro-tor sets the clock from the Tor network.
    (text.contains("NTP=yes") || tor, text.contains("NTPSynchronized=yes") || tor)
}

fn refresh_time(state: &mut State) {
    let (_, _, _, h, mi, s) = local_now();
    (state.hour, state.min, state.sec) = (h, mi, s);
}

fn get_current_state() -> State {
    let (year, month, day, hour, min, sec) = local_now();
    let (auto, synced) = sync_state();
    State { year, month, day, hour, min, sec, timezone: current_zone(), auto, synced, focus: None }
}

/// Applies what was chosen: the time zone, and automatic time (always on).
fn apply(original_zone: &str, state: &State) {
    if state.timezone != original_zone {
        match Command::new("timedatectl").args(["set-timezone", &state.timezone]).status() {
            Ok(s) if s.success() => {}
            _ => eprintln!("ferro-datetime: couldn't set the time zone to {}", state.timezone),
        }
    }
    if !state.auto {
        let _ = Command::new("timedatectl").args(["set-ntp", "true"]).status();
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-datetime: {e}");
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);

    let mut state = get_current_state();
    let original_zone = state.timezone.clone();

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;

    let (wx, wy) = ((sw - W) / 2, (sh - H) / 2);
    let values =
        CreateWindowAux::new().background_pixel(screen.white_pixel).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS);

    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        wx as i16,
        wy as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;

    let title = "Date/Time Properties";
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    let repaint = |state: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, W, H, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    repaint(&state)?;
    let mut last_tick = Instant::now();
    let keymap = Keymap::load(&conn)?;
    // A key press handed on as a click on a button.
    let mut pending: Option<Event> = None;

    loop {
        // The clock ticks: once a second, the time shown moves on.
        let Some(event) = (match pending.take() {
            Some(e) => Some(e),
            None => conn.poll_for_event()?,
        }) else {
            if last_tick.elapsed() >= Duration::from_secs(1) {
                last_tick = Instant::now();
                refresh_time(&mut state);
                repaint(&state)?;
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        match event {
            Event::ClientMessage(_) => break,
            Event::Expose(_) => {
                repaint(&state)?;
            }
            // Tab moves between the buttons, Space or Enter presses one
            // (Enter alone is OK), Left and Right change the time zone.
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                let press = |i: usize| {
                    let r = CONTROLS[i];
                    Some(ferro_ui::click_event(win, r.x + r.w / 2, r.y + r.h / 2))
                };
                match (key, ferro_ui::focus_key(&e, key, state.focus, &CONTROLS)) {
                    _ if ferro_ui::dialog_answer(key) == Some(false) => break,
                    (_, FocusKey::Moved(f)) => {
                        state.focus = f;
                        repaint(&state)?;
                    }
                    (_, FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                    _ if ferro_ui::dialog_answer(key) == Some(true) => pending = press(DEFAULT),
                    // Left and Right arrows: the zone's < and > buttons.
                    _ if e.detail == 113 => pending = press(2),
                    _ if e.detail == 114 => pending = press(3),
                    _ => {}
                }
            }
            Event::ButtonPress(e) => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                match hit(x, y, &state) {
                    Hit::PrevMonth => {
                        if state.month == 1 {
                            state.month = 12;
                            state.year -= 1;
                        } else {
                            state.month -= 1;
                        }
                        let max_d = days_in_month(state.year, state.month);
                        if state.day > max_d {
                            state.day = max_d;
                        }
                        repaint(&state)?;
                    }
                    Hit::NextMonth => {
                        if state.month == 12 {
                            state.month = 1;
                            state.year += 1;
                        } else {
                            state.month += 1;
                        }
                        let max_d = days_in_month(state.year, state.month);
                        if state.day > max_d {
                            state.day = max_d;
                        }
                        repaint(&state)?;
                    }
                    Hit::SelectDay(d) => {
                        state.day = d;
                        repaint(&state)?;
                    }
                    Hit::PrevZone | Hit::NextZone => {
                        let i = zone_index(&state.timezone);
                        let n = ZONES.len();
                        let next = if matches!(hit(x, y, &state), Hit::NextZone) { (i + 1) % n } else { (i + n - 1) % n };
                        state.timezone = ZONES[next].0.to_owned();
                        repaint(&state)?;
                    }
                    Hit::Ok => {
                        apply(&original_zone, &state);
                        break;
                    }
                    Hit::Close => break,
                    Hit::Nothing => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
