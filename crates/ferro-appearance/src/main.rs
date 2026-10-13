//! ferro-appearance: FerroOS Heavy's Display Properties.
//!
//! Lets users customize the desktop background color (Classic Teal, Dark Slate,
//! Pitch Black, Navy, Plum, Forest Green) and dark mode theme. Can be run
//! as a native X11 dialog or via CLI flags (`--set-color`, `--set-theme`).

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::path::PathBuf;
use view::{Hit, State, H, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("appearance.conf")
    } else {
        PathBuf::from("/etc/ferro/appearance.conf")
    }
}

fn load_config() -> State {
    let mut state = State::default();
    let path = config_path();
    let content = std::fs::read_to_string(&path).or_else(|_| std::fs::read_to_string("/etc/ferro/appearance.conf")).unwrap_or_default();

    for line in content.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "desktop_color" | "color" => {
                    let hex = val.strip_prefix("0x").or_else(|| val.strip_prefix('#')).unwrap_or(val);
                    if let Ok(c) = u32::from_str_radix(hex, 16) {
                        state.selected_color = c & 0x00ffffff;
                    }
                }
                "theme" => {
                    state.dark_mode = val.eq_ignore_ascii_case("dark");
                }
                "dark_mode" => {
                    state.dark_mode = val == "1" || val.eq_ignore_ascii_case("true");
                }
                _ => {}
            }
        }
    }
    state
}

fn save_config(state: &State) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = format!(
        "# FerroOS Appearance Configuration\n\
         desktop_color = 0x{:06X}\n\
         theme = {}\n",
        state.selected_color,
        if state.dark_mode { "dark" } else { "classic" }
    );
    std::fs::write(path, content)
}

fn notify_wm(conn: &RustConnection, root: Window, state: &State) -> Res<()> {
    let atom = conn.intern_atom(false, b"_FERRO_APPEARANCE")?.reply()?.atom;
    let data = [state.selected_color, u32::from(state.dark_mode)];
    conn.change_property32(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &data)?;
    conn.flush()?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let mut state = load_config();
        let mut changed = false;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--help" | "-h" => {
                    println!("Usage: ferro-appearance [OPTIONS]");
                    println!("Options:");
                    println!("  --set-color <hex>    Set desktop background color (e.g. 0x008080, 0x182026)");
                    println!("  --set-theme <name>   Set theme ('classic' or 'dark')");
                    println!("  --status             Show current appearance settings");
                    return;
                }
                "--status" => {
                    println!("Desktop Color: 0x{:06X}", state.selected_color);
                    println!("Theme: {}", if state.dark_mode { "dark" } else { "classic" });
                    return;
                }
                "--set-color" if i + 1 < args.len() => {
                    i += 1;
                    let val = &args[i];
                    let hex = val.strip_prefix("0x").or_else(|| val.strip_prefix('#')).unwrap_or(val);
                    if let Ok(c) = u32::from_str_radix(hex, 16) {
                        state.selected_color = c & 0x00ffffff;
                        changed = true;
                    } else {
                        eprintln!("Invalid color: {}", val);
                        std::process::exit(1);
                    }
                }
                "--set-theme" if i + 1 < args.len() => {
                    i += 1;
                    state.dark_mode = args[i].eq_ignore_ascii_case("dark");
                    changed = true;
                }
                _ => {}
            }
            i += 1;
        }
        if changed {
            let _ = save_config(&state);
            if let Ok((conn, screen_num)) = x11rb::connect(None) {
                let root = conn.setup().roots[screen_num].root;
                let _ = notify_wm(&conn, root, &state);
            }
            println!("Updated appearance: color=0x{:06X}, theme={}", state.selected_color, if state.dark_mode { "dark" } else { "classic" });
            return;
        }
    }

    if let Err(e) = run_gui() {
        eprintln!("ferro-appearance: {e}");
        std::process::exit(1);
    }
}

fn run_gui() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let root = screen.root;
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));

    let win = conn.generate_id()?;
    let x = ((sw - W) / 2).max(0) as i16;
    let y = ((sh - H) / 2).max(0) as i16;

    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS;
    conn.create_window(
        screen.root_depth,
        win,
        root,
        x,
        y,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(ferro_gfx::color::FACE).event_mask(mask),
    )?;

    // Set window title
    let title = "Display Properties";
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;

    conn.map_window(win)?;

    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;

    let keymap = Keymap::load(&conn)?;
    let mut state = load_config();

    let paint = |state: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        view::draw(&mut s, W, H, state);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    paint(&state)?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(e) if e.count == 0 => {
                paint(&state)?;
            }
            Event::ButtonPress(e) if e.detail == 1 => {
                let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
                match view::hit(px, py) {
                    Hit::Color(c) => {
                        state.selected_color = c;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::ToggleDark => {
                        state.dark_mode = !state.dark_mode;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::Apply => {
                        let _ = save_config(&state);
                        let _ = notify_wm(&conn, root, &state);
                        state.applied = true;
                        paint(&state)?;
                    }
                    Hit::Ok => {
                        let _ = save_config(&state);
                        let _ = notify_wm(&conn, root, &state);
                        break;
                    }
                    Hit::Cancel => {
                        break;
                    }
                    Hit::Nothing => {}
                }
            }
            Event::KeyPress(e) => match keymap.typed(&e) {
                Key::Enter => {
                    let _ = save_config(&state);
                    let _ = notify_wm(&conn, root, &state);
                    break;
                }
                Key::Escape => break,
                _ => {}
            },
            Event::ClientMessage(_) => break,
            _ => {}
        }
    }

    Ok(())
}
