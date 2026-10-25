//! ferro-associations: Retro Windows 95 File Types & Associations Manager.

use ferro_associations::config::{default_associations, load_associations, save_associations};
use ferro_associations::view::{cycle_preset, draw, hit, Hit, State, CONTROLS, H, VISIBLE_ROWS, W};
use ferro_gfx::Surface;
use ferro_ui::{FocusKey, Key, Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

/// The middle of the OK button.
fn view_ok() -> (i32, i32) {
    let r = ferro_associations::view::OK_BTN;
    (r.x + r.w / 2, r.y + r.h / 2)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-associations: {e}");
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);

    let entries = load_associations();
    let mut state = State { entries, selected: 0, scroll: 0, applied: true, focus: None };
    let keymap = Keymap::load(&conn)?;

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

    let title = "File Types";
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

    // A key press handed on as a click on the focused button.
    let mut pending: Option<Event> = None;
    loop {
        let event = match pending.take() {
            Some(e) => e,
            None => conn.wait_for_event()?,
        };
        match event {
            Event::ClientMessage(_) => break,
            Event::Expose(_) => {
                repaint(&state)?;
            }
            // Up and Down choose a file type, Tab moves between the
            // buttons, Space or Enter presses one (Enter alone is OK).
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                match (key, ferro_ui::focus_key(&e, key, state.focus, &CONTROLS)) {
                    (Key::Escape, _) => break,
                    (_, FocusKey::Moved(f)) => state.focus = f,
                    (_, FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                    (Key::Enter, _) => {
                        let r = view_ok();
                        pending = Some(ferro_ui::click_event(win, r.0, r.1));
                    }
                    (Key::Up, _) if state.selected > 0 => state.selected -= 1,
                    (Key::Down, _) if state.selected + 1 < state.entries.len() => state.selected += 1,
                    _ => continue,
                }
                // Keep the chosen row in sight.
                if state.selected < state.scroll {
                    state.scroll = state.selected;
                } else if state.selected >= state.scroll + VISIBLE_ROWS {
                    state.scroll = state.selected + 1 - VISIBLE_ROWS;
                }
                repaint(&state)?;
            }
            Event::ButtonPress(e) => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                match hit(x, y, &state) {
                    Hit::Select(idx) => {
                        state.selected = idx;
                        repaint(&state)?;
                    }
                    Hit::CyclePreset => {
                        if let Some(entry) = state.entries.get_mut(state.selected) {
                            entry.command = cycle_preset(&entry.command);
                            state.applied = false;
                            repaint(&state)?;
                        }
                    }
                    Hit::RestoreDefaults => {
                        state.entries = default_associations();
                        state.selected = 0;
                        state.scroll = 0;
                        state.applied = false;
                        repaint(&state)?;
                    }
                    Hit::ScrollUp => {
                        if state.scroll > 0 {
                            state.scroll -= 1;
                            repaint(&state)?;
                        }
                    }
                    Hit::ScrollDown => {
                        if state.scroll + VISIBLE_ROWS < state.entries.len() {
                            state.scroll += 1;
                            repaint(&state)?;
                        }
                    }
                    Hit::Apply => {
                        save_associations(&state.entries);
                        state.applied = true;
                        repaint(&state)?;
                    }
                    Hit::Ok => {
                        if !state.applied {
                            save_associations(&state.entries);
                        }
                        break;
                    }
                    Hit::Cancel => break,
                    Hit::Nothing => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
