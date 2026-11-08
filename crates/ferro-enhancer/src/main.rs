//! ferro-enhancer: the Sound Enhancer. A volume boost past 100% (with a
//! limiter, so it doesn't clip), a bass boost and a ten-band equalizer with
//! presets, for every program's sound, switched on and off in one place.
//!
//! The sound itself is changed by ferro-ladspa, a small plugin alsa-lib
//! runs inside each program that plays (`~/.asoundrc`, from ferro-audio).
//! This window only writes `~/.config/ferro/enhancer.conf`; the plugins
//! notice within a twentieth of a second, so a change is heard as soon as
//! the sound already handed to the card has played out.

mod view;

use ferro_dsp::settings::{Settings, PRESETS};
use ferro_gfx::Surface;
use ferro_ui::Res;
use std::path::PathBuf;
use std::process::Command;
use view::{draw, hit, Drag, Hit, State, H, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

const FERRO_AUDIO: &str = "/usr/local/bin/ferro-audio";
/// Where the plugin is (heavy/ferro-audio has the same).
const PLUGIN: &str = "/usr/local/lib/ferro-heavy/ladspa/ferro-enhancer.so";
const MARK: &str = "# Written by ferro-audio";

fn asoundrc() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".asoundrc"))
}

/// Whether programs' sound goes through the enhancer, and if not, why
/// (false). When ferro-audio wrote `~/.asoundrc` before the enhancer was
/// there, it's written again with it.
fn chain_state() -> (String, bool) {
    let read = || asoundrc().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let text = read();
    if text.contains("ferro_enhancer") {
        return ("Changes are heard at once.".into(), true);
    }
    if !std::path::Path::new(PLUGIN).exists() {
        return ("Not in use: its plugin isn't installed".into(), false);
    }
    if !text.is_empty() && !text.starts_with(MARK) {
        return ("Not in use: ~/.asoundrc is your own".into(), false);
    }
    let _ = Command::new(FERRO_AUDIO).arg("apply").status();
    if read().contains("ferro_enhancer") {
        ("Programs started from now on use it.".into(), true)
    } else {
        ("Not in use: no sound card was found".into(), false)
    }
}

fn save(s: &Settings) {
    if let Err(e) = s.save() {
        eprintln!("ferro-enhancer: can't save the settings: {e}");
    }
}

/// Moves a slider; saved (and heard) only when its value changes.
fn slide(s: &mut Settings, d: Drag, v: f32) -> bool {
    let old = s.clone();
    match d {
        Drag::Boost => s.boost = v,
        Drag::Loudness => s.loudness = v,
        Drag::Bass => s.bass = v,
        Drag::Band(i) if s.eq.get(i) != Some(&v) => s.set_band(i, v),
        Drag::Band(_) => {}
    }
    *s != old
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-enhancer: {e}");
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));

    let mut state = State { settings: Settings::load(), dragging: None, chain: chain_state() };

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION | EventMask::KEY_PRESS);
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2).max(0) as i16,
        ((sh - H) / 3).max(0) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Sound Enhancer")?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-enhancer\0FerroEnhancer\0")?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    let repaint = |state: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    repaint(&state)?;

    loop {
        let changed = match conn.wait_for_event()? {
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => break,
            Event::Expose(e) if e.count == 0 => {
                repaint(&state)?;
                false
            }
            Event::KeyPress(e) => match e.detail {
                // Esc, Enter, keypad Enter
                9 | 36 | 104 => break,
                // Space: on or off
                65 => {
                    state.settings.on = !state.settings.on;
                    true
                }
                _ => false,
            },
            Event::ButtonPress(e) if e.detail == 1 => match hit(i32::from(e.event_x), i32::from(e.event_y)) {
                Hit::On => !std::mem::replace(&mut state.settings.on, true),
                Hit::Off => std::mem::replace(&mut state.settings.on, false),
                Hit::Slider(d, v) => {
                    state.dragging = Some(d);
                    slide(&mut state.settings, d, v) || {
                        repaint(&state)?;
                        false
                    }
                }
                Hit::SmallSpeakers => {
                    state.settings.small_speakers = !state.settings.small_speakers;
                    true
                }
                Hit::Preset(i) => {
                    let name = PRESETS.get(i).map_or(ferro_dsp::settings::OWN, |(n, _)| n);
                    let old = state.settings.clone();
                    state.settings.choose(name);
                    state.settings != old
                }
                Hit::Reset => {
                    let s = &mut state.settings;
                    let old = s.clone();
                    (s.boost, s.loudness, s.bass) = (0.0, 0.0, 0.0);
                    s.choose("Flat");
                    *s != old
                }
                Hit::Close => break,
                Hit::Nothing => false,
            },
            Event::MotionNotify(e) => match state.dragging {
                Some(d) => slide(&mut state.settings, d, view::drag_value(d, i32::from(e.event_x), i32::from(e.event_y))),
                None => false,
            },
            Event::ButtonRelease(_) => {
                state.dragging = None;
                false
            }
            _ => false,
        };
        if changed {
            save(&state.settings);
            repaint(&state)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_change_only_when_their_value_does() {
        let mut s = Settings::default();
        assert!(slide(&mut s, Drag::Boost, 4.0));
        assert!(!slide(&mut s, Drag::Boost, 4.0));
        assert!(slide(&mut s, Drag::Band(2), -3.0));
        assert_eq!(s.preset, ferro_dsp::settings::OWN);
        assert!(!slide(&mut s, Drag::Band(2), -3.0));
        assert!(slide(&mut s, Drag::Bass, 12.0));
        assert!(slide(&mut s, Drag::Loudness, 6.0));
        assert_eq!(s.loudness, 6.0);
        assert_eq!((s.boost, s.bass, s.eq[2]), (4.0, 12.0, -3.0));
    }
}
