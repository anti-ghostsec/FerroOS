//! ferro-volume: Retro Windows 95 Volume Control.

mod view;

use ferro_gfx::Surface;
use ferro_ui::Res;
use std::process::Command;
use view::{draw, hit, Hit, State, H, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

fn query_volume() -> (u8, bool) {
    if let Ok(out) = Command::new("amixer").arg("sget").arg("Master").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let mut vol = 75u8;
            let mut muted = false;
            for token in s.split_whitespace() {
                if token.starts_with('[') && token.ends_with("%]") {
                    let num = &token[1..token.len() - 2];
                    if let Ok(v) = num.parse::<u8>() {
                        vol = v;
                    }
                } else if token == "[off]" {
                    muted = true;
                }
            }
            return (vol, muted);
        }
    }
    (75, false)
}

fn apply_volume(vol: u8, muted: bool) {
    // 1. amixer
    let _ = Command::new("amixer").args(["-q", "sset", "Master", &format!("{vol}%")]).status();
    let mute_arg = if muted { "mute" } else { "unmute" };
    let _ = Command::new("amixer").args(["-q", "sset", "Master", mute_arg]).status();

    // 2. pactl fallback
    let _ = Command::new("pactl").args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{vol}%")]).status();
    let _ = Command::new("pactl").args(["set-sink-mute", "@DEFAULT_SINK@", if muted { "1" } else { "0" }]).status();
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-volume: {e}");
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);

    let (cur_vol, cur_mute) = query_volume();
    let mut state = State { volume: cur_vol, muted: cur_mute, dragging: false };

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;

    let (wx, wy) = (sw - W - 10, sh - H - 36);
    let values = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::KEY_PRESS);

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

    // WM_NAME
    let title = "Volume";
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

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::ClientMessage(_) => break,
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
                    Hit::Slider(vol) => {
                        state.volume = vol;
                        state.dragging = true;
                        apply_volume(state.volume, state.muted);
                        repaint(&state)?;
                    }
                    Hit::ToggleMute => {
                        state.muted = !state.muted;
                        apply_volume(state.volume, state.muted);
                        repaint(&state)?;
                    }
                    Hit::Close => break,
                    Hit::Nothing => {}
                }
            }
            Event::ButtonRelease(_) => {
                state.dragging = false;
            }
            Event::MotionNotify(e) if state.dragging => {
                let y = i32::from(e.event_y);
                let vol = view::volume_from_y(y);
                if vol != state.volume {
                    state.volume = vol;
                    apply_volume(state.volume, state.muted);
                    repaint(&state)?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}
