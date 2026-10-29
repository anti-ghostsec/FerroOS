//! ferro-volume: Retro Windows 95 Volume Control multi-channel mixer.
//! Allows master control and selective per-app audio volume/muting (Browser, Video Player, etc.),
//! and picks the speakers the sound comes out of ("Play sound on", through ferro-audio).

mod mpris;
mod view;

use ferro_gfx::Surface;
use ferro_ui::Res;
use std::process::Command;
use view::{draw, hit, window_width, ChannelKind, ChannelState, Hit, Output, State, H};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

const FERRO_AUDIO: &str = "/usr/local/bin/ferro-audio";

/// The ways the sound can come out, and the one it does.
fn query_outputs() -> (Vec<Output>, Option<usize>) {
    let Ok(out) = Command::new(FERRO_AUDIO).arg("list").output() else {
        return (Vec::new(), None);
    };
    parse_outputs(&String::from_utf8_lossy(&out.stdout))
}

/// `ferro-audio list`: ID, plugged in, in use, name, a tab apart.
fn parse_outputs(text: &str) -> (Vec<Output>, Option<usize>) {
    let mut outputs = Vec::new();
    let mut current = None;
    for line in text.lines() {
        let f: Vec<&str> = line.splitn(4, '\t').collect();
        if f.len() < 4 || !f[0].starts_with("pci-") {
            continue;
        }
        if f[2] == "1" {
            current = Some(outputs.len());
        }
        outputs.push(Output { id: f[0].to_string(), plugged: f[1] == "1", label: f[3].to_string() });
    }
    (outputs, current)
}

/// Sound from now on comes out of `id`: what programs open next, the
/// volume control included. (Those playing already keep on where they are
/// until they next start a sound, most of them.)
fn choose_output(id: &str) {
    let _ = Command::new(FERRO_AUDIO).args(["set", id]).status();
}

fn refresh_outputs(state: &mut State) {
    let (outputs, current) = query_outputs();
    state.outputs = outputs;
    state.output = current;
}

fn query_master() -> (u8, bool) {
    if let Ok(out) = Command::new("amixer").arg("sget").arg("Master").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let mut vol = 75u8;
            let mut muted = false;
            // A volume without a switch of its own (HDMI's, made by
            // ferro-audio) is muted by turning it right down.
            let has_switch = s.contains("[on]") || s.contains("[off]");
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
            return (vol, muted || (!has_switch && vol == 0));
        }
    }
    (75, false)
}

fn apply_master(vol: u8, muted: bool) {
    // 1. amixer
    let _ = Command::new("amixer").args(["-q", "sset", "Master", &format!("{vol}%")]).status();
    let mute_arg = if muted { "mute" } else { "unmute" };
    let switched = Command::new("amixer").args(["-q", "sset", "Master", mute_arg]).status().is_ok_and(|s| s.success());
    if muted && !switched {
        let _ = Command::new("amixer").args(["-q", "sset", "Master", "0%"]).status();
    }

    // 2. pactl fallback if installed
    let _ = Command::new("pactl").args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{vol}%")]).status();
    let _ = Command::new("pactl").args(["set-sink-mute", "@DEFAULT_SINK@", if muted { "1" } else { "0" }]).status();
}

fn apply_channel(ch: &ChannelState) {
    match &ch.kind {
        ChannelKind::Master => {
            apply_master(ch.volume, ch.muted);
        }
        ChannelKind::Mpris { dest } => {
            if ch.muted {
                mpris::set_player_volume(dest, 0);
            } else {
                mpris::set_player_volume(dest, ch.volume);
            }
        }
    }
}

fn refresh_channels(state: &mut State) {
    // 1. Update Master (channel 0)
    let (m_vol, m_mute) = query_master();
    if let Some(ch0) = state.channels.get_mut(0) {
        ch0.volume = m_vol;
        ch0.muted = m_mute;
    }

    // 2. Query MPRIS players
    let players = mpris::list_players();

    // Retain master (col 0) and any still-living MPRIS streams
    state.channels.retain(|ch| match &ch.kind {
        ChannelKind::Master => true,
        ChannelKind::Mpris { dest } => players.iter().any(|p| &p.dest == dest),
    });

    // Update existing or append new players
    for p in players {
        if let Some(existing) = state.channels.iter_mut().find(|c| match &c.kind {
            ChannelKind::Mpris { dest } => dest == &p.dest,
            _ => false,
        }) {
            if state.dragging.is_none() {
                existing.volume = p.volume;
                existing.muted = p.muted;
            }
        } else {
            state.channels.push(ChannelState {
                kind: ChannelKind::Mpris { dest: p.dest },
                label: p.label,
                sublabel: p.sublabel,
                volume: p.volume,
                muted: p.muted,
                saved_volume: p.saved_volume,
            });
        }
    }

    if state.selected_col >= state.channels.len() {
        state.selected_col = state.channels.len().saturating_sub(1);
    }
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

    let mut state = State::default();
    refresh_channels(&mut state);
    refresh_outputs(&mut state);
    let mut ticks = 0u32;

    let mut cur_w = window_width(state.channels.len());
    let wx = (sw - cur_w - 10).max(10);
    let wy = (sh - H - 36).max(10);

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;

    let values = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::KEY_PRESS);

    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        wx as i16,
        wy as i16,
        cur_w as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;

    // Window title and protocol
    let title = "Volume Control";
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    let tick_atom = conn.intern_atom(false, b"_FERRO_VOLUME_TICK")?.reply()?.atom;

    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    // Tick thread: sends tick every 1000ms to poll for new media players or volume changes
    let tick_win = win;
    std::thread::spawn(move || {
        let Ok((tick_conn, _)) = x11rb::connect(None) else { return };
        let event = ClientMessageEvent::new(32, tick_win, tick_atom, [0; 5]);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            if tick_conn.send_event(false, tick_win, EventMask::NO_EVENT, event).is_err() || tick_conn.flush().is_err() {
                return;
            }
        }
    });

    let repaint = |state: &State, w: i32| -> Res<()> {
        let mut s = Surface::new(w as usize, H as usize);
        draw(&mut s, w, H, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, w, H)
    };

    repaint(&state, cur_w)?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::ClientMessage(e) => {
                let data = e.data.as_data32();
                if e.type_ == wm_protocols && data[0] == wm_delete {
                    break;
                }
                if e.type_ == tick_atom && state.dragging.is_none() {
                    // Speakers or a monitor plugged in or out, now and then.
                    ticks += 1;
                    if ticks.is_multiple_of(4) {
                        refresh_outputs(&mut state);
                    }
                    let prev_len = state.channels.len();
                    refresh_channels(&mut state);
                    let next_len = state.channels.len();
                    if prev_len != next_len {
                        cur_w = window_width(next_len);
                        let new_wx = (sw - cur_w - 10).max(10);
                        let _ = conn.configure_window(win, &ConfigureWindowAux::new().width(cur_w as u32).x(new_wx));
                    }
                    repaint(&state, cur_w)?;
                }
            }
            Event::Expose(_) => {
                repaint(&state, cur_w)?;
            }
            Event::KeyPress(e) => {
                match e.detail {
                    // Escape, Return, KP_Enter
                    9 | 36 | 104 => break,
                    // Left arrow
                    113 => {
                        state.selected_col = state.selected_col.saturating_sub(1);
                        repaint(&state, cur_w)?;
                    }
                    // Right arrow
                    114 => {
                        state.selected_col = (state.selected_col + 1).min(state.channels.len().saturating_sub(1));
                        repaint(&state, cur_w)?;
                    }
                    // Up arrow (+5%), Down arrow (-5%), PgUp (+20%), PgDown (-20%)
                    111 | 116 | 112 | 117 => {
                        let step: i16 = match e.detail {
                            111 => 5,
                            116 => -5,
                            112 => 20,
                            117 => -20,
                            _ => 0,
                        };
                        if let Some(ch) = state.channels.get_mut(state.selected_col) {
                            let new_vol = (i16::from(ch.volume) + step).clamp(0, 100) as u8;
                            if new_vol != ch.volume {
                                ch.volume = new_vol;
                                if ch.muted && new_vol > 0 {
                                    ch.muted = false;
                                }
                                apply_channel(ch);
                                repaint(&state, cur_w)?;
                            }
                        }
                    }
                    // Space or 'm' / 'M'
                    65 | 58 => {
                        if let Some(ch) = state.channels.get_mut(state.selected_col) {
                            ch.muted = !ch.muted;
                            if ch.muted {
                                ch.saved_volume = if ch.volume > 0 { ch.volume } else { 80 };
                            } else if ch.volume == 0 {
                                ch.volume = if ch.saved_volume > 0 { ch.saved_volume } else { 80 };
                            }
                            apply_channel(ch);
                            repaint(&state, cur_w)?;
                        }
                    }
                    _ => {}
                }
            }
            Event::ButtonPress(e) => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                match hit(x, y, cur_w, &state) {
                    Hit::Slider(col, vol) => {
                        state.selected_col = col;
                        state.dragging = Some(col);
                        if let Some(ch) = state.channels.get_mut(col) {
                            ch.volume = vol;
                            if ch.muted && vol > 0 {
                                ch.muted = false;
                            }
                            apply_channel(ch);
                        }
                        repaint(&state, cur_w)?;
                    }
                    Hit::ToggleMute(col) => {
                        state.selected_col = col;
                        if let Some(ch) = state.channels.get_mut(col) {
                            ch.muted = !ch.muted;
                            if ch.muted {
                                ch.saved_volume = if ch.volume > 0 { ch.volume } else { 80 };
                            } else if ch.volume == 0 {
                                ch.volume = if ch.saved_volume > 0 { ch.saved_volume } else { 80 };
                            }
                            apply_channel(ch);
                        }
                        repaint(&state, cur_w)?;
                    }
                    Hit::SelectCol(col) => {
                        state.selected_col = col;
                        repaint(&state, cur_w)?;
                    }
                    h @ (Hit::OutputPrev | Hit::OutputNext) => {
                        if let Some(i) = view::step_output(&state, h == Hit::OutputNext) {
                            choose_output(&state.outputs[i].id);
                            refresh_outputs(&mut state);
                            refresh_channels(&mut state);
                        }
                        repaint(&state, cur_w)?;
                    }
                    Hit::OutputTest => {
                        let _ = Command::new(FERRO_AUDIO).arg("test").spawn().map(|mut c| std::thread::spawn(move || c.wait()));
                    }
                    Hit::Close => break,
                    Hit::Nothing => {}
                }
            }
            Event::ButtonRelease(_) => {
                state.dragging = None;
            }
            Event::MotionNotify(e) => {
                if let Some(col) = state.dragging {
                    let y = i32::from(e.event_y);
                    let vol = view::volume_from_y(y);
                    if let Some(ch) = state.channels.get_mut(col) {
                        if vol != ch.volume {
                            ch.volume = vol;
                            if ch.muted && vol > 0 {
                                ch.muted = false;
                            }
                            apply_channel(ch);
                            repaint(&state, cur_w)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_from_ferro_audio() {
        let text = "pci-0000:03:00.1:3\t0\t0\tHDMI 0 (NVIDIA graphics)\n\
                    pci-0000:0a:00.1:3\t1\t1\tFireTV (HDMI, AMD graphics)\n\
                    ferro-audio: a warning\n\
                    pci-0000:0a:00.6:0\t1\t0\tSpeakers / headphones (Realtek ALC897)\n";
        let (outputs, current) = parse_outputs(text);
        assert_eq!(outputs.len(), 3);
        assert_eq!(current, Some(1));
        assert_eq!(outputs[1].label, "FireTV (HDMI, AMD graphics)");
        assert!(!outputs[0].plugged && outputs[2].plugged);
        assert_eq!(parse_outputs(""), (Vec::new(), None));
    }
}
