//! ferro-appearance: FerroOS Heavy's Display Properties.
//!
//! Lets users customize the desktop background color, wallpaper image,
//! screensaver (Stars, Matrix, Blank, Custom scripts), logon screen wallpaper,
//! and dark mode theme. Can be run as a native X11 dialog or via CLI flags.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use view::{Hit, ScreensaverItem, State, WallpaperItem, H, W};
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

pub fn scan_wallpapers() -> Vec<WallpaperItem> {
    let mut list = vec![WallpaperItem { name: "(None)".to_string(), path: None }];
    let dirs = ["/usr/share/backgrounds", "/usr/share/wallpapers", "/etc/ferro/wallpapers"];
    let mut seen = HashSet::new();
    for d in dirs {
        if let Ok(entries) = std::fs::read_dir(d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("bmp") || ext.eq_ignore_ascii_case("ppm") || ext.eq_ignore_ascii_case("png") {
                        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper").to_string();
                        if seen.insert(name.clone()) {
                            list.push(WallpaperItem { name, path: Some(p.to_string_lossy().into_owned()) });
                        }
                    }
                }
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local").join("share").join("backgrounds");
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("bmp") || ext.eq_ignore_ascii_case("ppm") || ext.eq_ignore_ascii_case("png") {
                        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper").to_string();
                        if seen.insert(name.clone()) {
                            list.push(WallpaperItem { name, path: Some(path.to_string_lossy().into_owned()) });
                        }
                    }
                }
            }
        }
    }
    list
}

pub fn scan_screensavers() -> Vec<ScreensaverItem> {
    let mut list = vec![
        ScreensaverItem { name: "(None)".into(), mode: "none".into(), exec: None },
        ScreensaverItem { name: "Flying Stars".into(), mode: "stars".into(), exec: None },
        ScreensaverItem { name: "Phosphor Matrix".into(), mode: "matrix".into(), exec: None },
        ScreensaverItem { name: "Blank Screen".into(), mode: "blank".into(), exec: None },
    ];

    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local").join("share").join("ferro").join("screensavers");
        let _ = std::fs::create_dir_all(&p);
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
                    list.push(ScreensaverItem { name, mode: "custom".into(), exec: Some(path.to_string_lossy().into_owned()) });
                }
            }
        }
    }

    list
}

fn load_config() -> State {
    let mut state = State { wallpapers: scan_wallpapers(), screensavers: scan_screensavers(), ..State::default() };

    let path = config_path();
    let content = std::fs::read_to_string(&path).or_else(|_| std::fs::read_to_string("/etc/ferro/appearance.conf")).unwrap_or_default();

    let mut configured_wp = None;
    let mut configured_logon_wp = None;
    let mut configured_ss = None;

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
                "wallpaper" if !val.is_empty() => {
                    configured_wp = Some(val.to_string());
                }
                "logon_wallpaper" if !val.is_empty() => {
                    configured_logon_wp = Some(val.to_string());
                }
                "screensaver" => {
                    configured_ss = Some(val.to_string());
                }
                "screensaver_timeout" => {
                    if let Ok(mins) = val.parse::<u32>() {
                        state.screensaver_timeout = mins.clamp(1, 60);
                    }
                }
                "screensaver_lock" => {
                    state.screensaver_lock = val == "1" || val.eq_ignore_ascii_case("true");
                }
                _ => {}
            }
        }
    }

    if let Some(wp) = configured_wp {
        if let Some(pos) = state.wallpapers.iter().position(|w| w.path.as_deref() == Some(&wp)) {
            state.selected_wallpaper = pos;
        } else {
            let name = Path::new(&wp).file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
            state.wallpapers.push(WallpaperItem { name, path: Some(wp) });
            state.selected_wallpaper = state.wallpapers.len() - 1;
        }
    }

    if let Some(lwp) = configured_logon_wp {
        let current_wp = state.wallpapers.get(state.selected_wallpaper).and_then(|w| w.path.as_deref());
        state.logon_wallpaper = current_wp == Some(&lwp);
    }

    if let Some(ss) = configured_ss {
        if let Some(pos) = state.screensavers.iter().position(|s| s.mode.eq_ignore_ascii_case(&ss) || s.name.eq_ignore_ascii_case(&ss)) {
            state.selected_screensaver = pos;
        }
    }

    state.reload_preview();
    state
}

fn save_config(state: &State) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let wp = state.wallpapers.get(state.selected_wallpaper).and_then(|w| w.path.as_deref()).unwrap_or("");
    let ss_mode = state.screensavers.get(state.selected_screensaver).map_or("none", |s| &s.mode);

    let mut content = format!(
        "# FerroOS Appearance Configuration\n\
         desktop_color = 0x{:06X}\n\
         theme = {}\n\
         wallpaper = {}\n\
         screensaver = {}\n\
         screensaver_timeout = {}\n\
         screensaver_lock = {}\n",
        state.selected_color,
        if state.dark_mode { "dark" } else { "classic" },
        wp,
        ss_mode,
        state.screensaver_timeout,
        state.screensaver_lock
    );
    if state.logon_wallpaper && !wp.is_empty() {
        content.push_str(&format!("logon_wallpaper = {}\n", wp));
    }
    let res = std::fs::write(&path, &content);

    // If logon wallpaper was checked, write or update /etc/ferro/appearance.conf if writable
    if state.logon_wallpaper {
        let etc_path = Path::new("/etc/ferro/appearance.conf");
        if let Some(parent) = etc_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(etc_path, &content);
    }

    res
}

fn notify_wm(conn: &RustConnection, root: Window, state: &State) -> Res<()> {
    let atom = conn.intern_atom(false, b"_FERRO_APPEARANCE")?.reply()?.atom;
    let data = [state.selected_color, u32::from(state.dark_mode)];
    conn.change_property32(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &data)?;
    conn.flush()?;
    Ok(())
}

fn preview_current_screensaver(state: &State) {
    if let Some(ss) = state.screensavers.get(state.selected_screensaver) {
        match ss.mode.as_str() {
            "none" => {}
            "stars" => {
                let _ = Command::new("ferro-screensaver").arg("--stars").spawn();
            }
            "matrix" => {
                let _ = Command::new("ferro-screensaver").arg("--matrix").spawn();
            }
            "blank" => {
                let _ = Command::new("ferro-screensaver").arg("--blank").spawn();
            }
            "custom" => {
                if let Some(cmd) = &ss.exec {
                    let _ = Command::new("sh").arg("-c").arg(cmd).spawn();
                }
            }
            _ => {}
        }
    }
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
                    println!("  --set-color <hex>         Set desktop background color (e.g. 0x008080)");
                    println!("  --set-wallpaper <path>    Set desktop wallpaper image (.bmp/.ppm)");
                    println!("  --set-logon-wallpaper <p> Set logon screen wallpaper image");
                    println!("  --set-screensaver <mode>  Set screensaver ('stars', 'matrix', 'blank', 'none')");
                    println!("  --set-theme <name>        Set theme ('classic' or 'dark')");
                    println!("  --status                  Show current appearance settings");
                    return;
                }
                "--set-color" if i + 1 < args.len() => {
                    let hex = args[i + 1].strip_prefix("0x").or_else(|| args[i + 1].strip_prefix('#')).unwrap_or(&args[i + 1]);
                    if let Ok(c) = u32::from_str_radix(hex, 16) {
                        state.selected_color = c & 0x00ffffff;
                        changed = true;
                    }
                    i += 1;
                }
                "--set-wallpaper" if i + 1 < args.len() => {
                    let p = &args[i + 1];
                    let name = Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper").to_string();
                    state.wallpapers.push(WallpaperItem { name, path: Some(p.to_string()) });
                    state.selected_wallpaper = state.wallpapers.len() - 1;
                    changed = true;
                    i += 1;
                }
                "--set-logon-wallpaper" if i + 1 < args.len() => {
                    let p = &args[i + 1];
                    let name = Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper").to_string();
                    state.wallpapers.push(WallpaperItem { name, path: Some(p.to_string()) });
                    state.selected_wallpaper = state.wallpapers.len() - 1;
                    state.logon_wallpaper = true;
                    changed = true;
                    i += 1;
                }
                "--set-screensaver" if i + 1 < args.len() => {
                    let mode = &args[i + 1];
                    if let Some(pos) = state.screensavers.iter().position(|s| s.mode.eq_ignore_ascii_case(mode)) {
                        state.selected_screensaver = pos;
                        changed = true;
                    }
                    i += 1;
                }
                "--set-theme" if i + 1 < args.len() => {
                    state.dark_mode = args[i + 1].eq_ignore_ascii_case("dark");
                    changed = true;
                    i += 1;
                }
                "--status" => {
                    let wp = state.wallpapers.get(state.selected_wallpaper).map_or("(None)", |w| &w.name);
                    let ss = state.screensavers.get(state.selected_screensaver).map_or("(None)", |s| &s.name);
                    println!("Color: 0x{:06X}", state.selected_color);
                    println!("Wallpaper: {}", wp);
                    println!("Screensaver: {} (timeout: {} min)", ss, state.screensaver_timeout);
                    println!("Theme: {}", if state.dark_mode { "dark" } else { "classic" });
                    return;
                }
                _ => {}
            }
            i += 1;
        }
        if changed {
            let _ = save_config(&state);
            if let Ok((conn, screen_num)) = x11rb::connect(None) {
                let screen = conn.setup().roots[screen_num].clone();
                let _ = notify_wm(&conn, screen.root, &state);
            }
            println!("Settings saved.");
        }
        return;
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
                match view::hit(px, py, &state) {
                    Hit::Tab(t) => {
                        state.tab = t;
                        paint(&state)?;
                    }
                    Hit::PrevWallpaper => {
                        if !state.wallpapers.is_empty() {
                            state.selected_wallpaper = (state.selected_wallpaper + state.wallpapers.len() - 1) % state.wallpapers.len();
                            state.reload_preview();
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::NextWallpaper => {
                        if !state.wallpapers.is_empty() {
                            state.selected_wallpaper = (state.selected_wallpaper + 1) % state.wallpapers.len();
                            state.reload_preview();
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::ToggleLogonWallpaper => {
                        state.logon_wallpaper = !state.logon_wallpaper;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::Color(c) => {
                        state.selected_color = c;
                        state.selected_wallpaper = 0;
                        state.reload_preview();
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::ToggleDark => {
                        state.dark_mode = !state.dark_mode;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::PrevScreensaver => {
                        if !state.screensavers.is_empty() {
                            state.selected_screensaver = (state.selected_screensaver + state.screensavers.len() - 1) % state.screensavers.len();
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::NextScreensaver => {
                        if !state.screensavers.is_empty() {
                            state.selected_screensaver = (state.selected_screensaver + 1) % state.screensavers.len();
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::DecTimeout => {
                        if state.screensaver_timeout > 1 {
                            state.screensaver_timeout -= 1;
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::IncTimeout => {
                        if state.screensaver_timeout < 60 {
                            state.screensaver_timeout += 1;
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::ToggleSsLock => {
                        state.screensaver_lock = !state.screensaver_lock;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::PreviewScreensaver => {
                        preview_current_screensaver(&state);
                    }
                    Hit::OpenScreensaversFolder => {
                        if let Ok(home) = std::env::var("HOME") {
                            let dir = format!("{home}/.local/share/ferro/screensavers");
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = Command::new("ferro-files").arg(&dir).spawn();
                        }
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
