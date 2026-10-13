//! ferro-datetime: Retro Windows 95 Date & Time Properties.

mod view;

use ferro_gfx::Surface;
use ferro_ui::Res;
use std::time::SystemTime;
use view::{days_in_month, draw, hit, Hit, State, H, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

fn get_current_state() -> State {
    // Read local or UTC time
    let mut state = State::default();
    if let Ok(duration) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        let secs = duration.as_secs();
        let s = (secs % 60) as u32;
        let m = ((secs / 60) % 60) as u32;
        let h = ((secs / 3600) % 24) as u32;
        state.hour = h;
        state.min = m;
        state.sec = s;

        // Approximate days since epoch
        let days = (secs / 86400) as i32;
        let mut y = 1970;
        let mut rem_days = days;
        loop {
            let diy = if view::is_leap_year(y) { 366 } else { 365 };
            if rem_days < diy {
                break;
            }
            rem_days -= diy;
            y += 1;
        }
        state.year = y;
        let mut mon = 1;
        loop {
            let dim = days_in_month(y, mon) as i32;
            if rem_days < dim {
                break;
            }
            rem_days -= dim;
            mon += 1;
        }
        state.month = mon;
        state.day = (rem_days + 1) as u32;
    }

    if let Ok(tz) = std::env::var("TZ") {
        state.timezone = tz;
    } else if let Ok(tz) = std::fs::read_to_string("/etc/timezone") {
        state.timezone = tz.trim().to_string();
    }

    state
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
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    let repaint = |state: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, W, H, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    repaint(&state)?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(_) => {
                repaint(&state)?;
            }
            Event::KeyPress(e) => {
                if e.detail == 9 {
                    // Escape
                    break;
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
                    Hit::Close => break,
                    Hit::Nothing => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
