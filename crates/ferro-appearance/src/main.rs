//! ferro-appearance: FerroOS Heavy's Display Properties.
//!
//! Lets users customize the desktop background color, wallpaper image,
//! screensaver (Stars, Matrix, Blank, Custom scripts), logon screen wallpaper,
//! and dark mode theme, and which graphics card draws what (ferro-gpu). Can be
//! run as a native X11 dialog or via CLI flags.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{FocusKey, Keymap, Res};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use view::{GpuItem, GpuRule, Hit, ScreensaverItem, State, WallpaperItem, H, W};
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
                    if ext.eq_ignore_ascii_case("bmp")
                        || ext.eq_ignore_ascii_case("ppm")
                        || ext.eq_ignore_ascii_case("png")
                        || ext.eq_ignore_ascii_case("jpg")
                        || ext.eq_ignore_ascii_case("jpeg")
                        || ext.eq_ignore_ascii_case("webp")
                    {
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
                    if ext.eq_ignore_ascii_case("bmp")
                        || ext.eq_ignore_ascii_case("ppm")
                        || ext.eq_ignore_ascii_case("png")
                        || ext.eq_ignore_ascii_case("jpg")
                        || ext.eq_ignore_ascii_case("jpeg")
                        || ext.eq_ignore_ascii_case("webp")
                    {
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

const FERRO_GPU: &str = "/usr/local/bin/ferro-gpu";

fn gpu_config_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|home| PathBuf::from(home).join(".config").join("ferro").join("gpu.conf"))
}

/// The graphics cards, in ferro-gpu's order (gpu0 has the monitor).
fn scan_gpus() -> Vec<GpuItem> {
    let Ok(out) = Command::new("ferro-gpu").arg("list").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(parse_gpu_line).collect()
}

/// One line of `ferro-gpu list`: gpuN, PCI address, driver, monitor or -, name.
fn parse_gpu_line(line: &str) -> Option<GpuItem> {
    let mut f = line.split('\t');
    let _id = f.next()?;
    let pci = f.next()?.to_string();
    let driver = f.next()?.to_string();
    let monitor = f.next()? == "monitor";
    let name = f.next().unwrap_or("Graphics card").to_string();
    Some(GpuItem { pci, driver, monitor, name })
}

/// A card named in a setting (gpuN, or a PCI address) as its number.
fn gpu_index(gpus: &[GpuItem], value: &str) -> Option<usize> {
    if let Some(n) = value.strip_prefix("gpu").and_then(|n| n.parse::<usize>().ok()) {
        return (n < gpus.len()).then_some(n);
    }
    gpus.iter().position(|g| g.pci == value)
}

/// The cards' settings: the desktop's (the computer's, /etc/ferro/gpu.conf)
/// and the programs' (this person's, ~/.config/ferro/gpu.conf).
fn load_gpu_config(state: &mut State) {
    state.gpus = scan_gpus();
    let system = std::fs::read_to_string("/etc/ferro/gpu.conf").unwrap_or_default();
    let user = gpu_config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    parse_gpu_config(state, &system, &user);
}

fn parse_gpu_config(state: &mut State, system: &str, user: &str) {
    let pairs = |text: &str| -> Vec<(String, String)> {
        text.lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .collect()
    };
    state.gpu_desktop = pairs(system).into_iter().rfind(|(k, _)| k == "desktop").and_then(|(_, v)| gpu_index(&state.gpus, &v));
    state.gpu_desktop_saved = state.gpu_desktop;
    state.gpu_rules.clear();
    for (k, v) in pairs(user) {
        match k.as_str() {
            "apps" => state.gpu_apps = gpu_index(&state.gpus, &v).unwrap_or(0),
            "desktop" => {}
            _ => {
                if let Some(gpu) = gpu_index(&state.gpus, &v) {
                    state.gpu_rules.push(GpuRule { program: k, gpu });
                }
            }
        }
    }
}

fn gpu_config_text(state: &State) -> String {
    let mut text = String::from(
        "# Which graphics card programs use (ferro-gpu; Display Properties, Graphics).\n\
         # gpu0 is the card the monitor is plugged into. A program's own line wins\n\
         # over apps; wine* matches wine64 and the rest.\n",
    );
    text.push_str(&format!("apps = gpu{}\n", state.gpu_apps));
    for r in &state.gpu_rules {
        text.push_str(&format!("{} = gpu{}\n", r.program, r.gpu));
    }
    text
}

/// A program's name as a setting may hold it: one word, letters, digits and
/// - _ . + and the * of a pattern, nothing that could break the file.
fn valid_program(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+*".contains(c))
}

fn save_gpu_config(state: &State) {
    if state.gpus.len() < 2 {
        return;
    }
    if let Some(path) = gpu_config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, gpu_config_text(state));
    }
    if state.gpu_desktop != state.gpu_desktop_saved {
        let to = state.gpu_desktop.map_or_else(|| "auto".to_string(), |i| format!("gpu{i}"));
        let _ = Command::new("sudo").args(["-n", FERRO_GPU, "desktop", &to]).status();
    }
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
                "compositor" => {
                    state.compositor = val == "1" || val.eq_ignore_ascii_case("true");
                }
                "sounds" => {
                    state.sounds = !(val == "0" || val.eq_ignore_ascii_case("false"));
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

    load_gpu_config(&mut state);
    state.reload_preview();
    state
}

/// Smooth drawing (picom) on or off now; the session starts it, or not, by
/// the same setting (see xinitrc).
fn compositor(on: bool) {
    let running = std::process::Command::new("pgrep").args(["-x", "picom"]).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if on && !running {
        let _ = std::process::Command::new("picom").args(["-b", "--config", "/etc/xdg/picom.conf"]).spawn();
    } else if !on && running {
        let _ = std::process::Command::new("pkill").args(["-x", "picom"]).status();
    }
}

fn save_config(state: &State) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let wp = state.wallpapers.get(state.selected_wallpaper).and_then(|w| w.path.as_deref()).unwrap_or("");
    let ss_mode = state.screensavers.get(state.selected_screensaver).map_or("none", |s| &s.mode);

    let mut logon_wp_path = wp.to_string();
    if state.logon_wallpaper && !wp.is_empty() {
        let ext = Path::new(wp).extension().and_then(|e| e.to_str()).unwrap_or("png");
        let dest = Path::new("/etc/ferro/wallpapers").join(format!("logon-wallpaper.{ext}"));
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(wp, &dest).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
            }
            logon_wp_path = dest.to_string_lossy().into_owned();
        }
    }

    let mut content = format!(
        "# FerroOS Appearance Configuration\n\
         desktop_color = 0x{:06X}\n\
         theme = {}\n\
         wallpaper = {}\n\
         screensaver = {}\n\
         screensaver_timeout = {}\n\
         screensaver_lock = {}\n\
         compositor = {}\n\
         sounds = {}\n",
        state.selected_color,
        if state.dark_mode { "dark" } else { "classic" },
        wp,
        ss_mode,
        state.screensaver_timeout,
        state.screensaver_lock,
        state.compositor,
        state.sounds
    );
    compositor(state.compositor);
    if state.logon_wallpaper && !wp.is_empty() {
        content.push_str(&format!("logon_wallpaper = {}\n", logon_wp_path));
    }
    let res = std::fs::write(&path, &content);
    save_gpu_config(state);

    // If logon wallpaper was checked, write or update /etc/ferro/appearance.conf if writable
    if state.logon_wallpaper {
        let etc_path = Path::new("/etc/ferro/appearance.conf");
        if let Some(parent) = etc_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(etc_path, &content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(etc_path, std::fs::Permissions::from_mode(0o644));
        }
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
                    if state.gpus.len() > 1 {
                        println!("Desktop card: {}", state.gpu_desktop.map_or_else(|| "automatic (gpu0)".to_string(), |i| format!("gpu{i}")));
                        println!("Programs' card: gpu{}", state.gpu_apps);
                        for r in &state.gpu_rules {
                            println!("  {} on gpu{}", r.program, r.gpu);
                        }
                    }
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

fn choose_file_dialog(title: &str, filter: &str) -> Option<String> {
    if let Ok(output) = Command::new("zenity").args(["--file-selection", &format!("--title={title}"), &format!("--file-filter={filter}")]).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && Path::new(&path).exists() {
                return Some(path);
            }
        }
    }
    None
}

/// The name of a program to give a card of its own, asked for in a box.
fn ask_program() -> Option<String> {
    let out = Command::new("zenity")
        .args([
            "--entry",
            "--title=Add Program",
            "--text=The program's name, as you'd type it to start it\n(wine, steam, ollama; wine* for wine64 and the rest):",
        ])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        return None;
    }
    if valid_program(&name) {
        Some(name)
    } else {
        let _ = Command::new("zenity")
            .args(["--error", "--title=Add Program", "--text=A program's name is one word: letters, digits, and - _ . + or *."])
            .status();
        None
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
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

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

    // A key press handed on as a click on the focused control.
    let mut pending: Option<Event> = None;
    loop {
        let event = match pending.take() {
            Some(e) => e,
            None => conn.wait_for_event()?,
        };
        match event {
            Event::Expose(e) if e.count == 0 => {
                paint(&state)?;
            }
            Event::ButtonPress(e) if e.detail == 1 => {
                let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
                let hit = view::hit(px, py, &state);
                match hit {
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
                    Hit::BrowseWallpaper => {
                        if let Some(path) = choose_file_dialog(
                            "Select Desktop Wallpaper Image",
                            "Image Files (*.bmp, *.png, *.ppm, *.jpg) | *.bmp *.png *.ppm *.jpg *.jpeg",
                        ) {
                            let name = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
                            state.wallpapers.push(WallpaperItem { name, path: Some(path) });
                            state.selected_wallpaper = state.wallpapers.len() - 1;
                            state.reload_preview();
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::BrowseLogonWallpaper => {
                        if let Some(path) = choose_file_dialog(
                            "Select Logon Screen Wallpaper Image",
                            "Image Files (*.bmp, *.png, *.ppm, *.jpg) | *.bmp *.png *.ppm *.jpg *.jpeg",
                        ) {
                            let name = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
                            state.wallpapers.push(WallpaperItem { name, path: Some(path) });
                            state.selected_wallpaper = state.wallpapers.len() - 1;
                            state.logon_wallpaper = true;
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
                    Hit::ToggleCompositor => {
                        state.compositor = !state.compositor;
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::ToggleSounds => {
                        state.sounds = !state.sounds;
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
                    Hit::BrowseScreensaver => {
                        if let Some(path) = choose_file_dialog("Select Screensaver Executable or Script", "All Files (*.*) | *") {
                            let name = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
                            state.screensavers.push(ScreensaverItem { name, mode: "custom".into(), exec: Some(path) });
                            state.selected_screensaver = state.screensavers.len() - 1;
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
                    Hit::PrevDesktopGpu | Hit::NextDesktopGpu => {
                        // Automatic, gpu0, gpu1... round and round.
                        let n = state.gpus.len() + 1;
                        let at = state.gpu_desktop.map_or(0, |i| i + 1);
                        let at = if matches!(hit, Hit::NextDesktopGpu) { (at + 1) % n } else { (at + n - 1) % n };
                        state.gpu_desktop = at.checked_sub(1);
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::PrevAppsGpu | Hit::NextAppsGpu => {
                        let n = state.gpus.len();
                        state.gpu_apps = if matches!(hit, Hit::NextAppsGpu) { (state.gpu_apps + 1) % n } else { (state.gpu_apps + n - 1) % n };
                        state.applied = false;
                        paint(&state)?;
                    }
                    Hit::SelectRule(i) => {
                        state.gpu_rule_selected = Some(i);
                        paint(&state)?;
                    }
                    Hit::AddRule => {
                        if let Some(program) = ask_program() {
                            // A new program goes on the second card: what it's usually for.
                            let gpu = 1.min(state.gpus.len() - 1);
                            match state.gpu_rules.iter().position(|r| r.program == program) {
                                Some(i) => state.gpu_rule_selected = Some(i),
                                None => {
                                    state.gpu_rules.push(GpuRule { program, gpu });
                                    state.gpu_rule_selected = Some(state.gpu_rules.len() - 1);
                                }
                            }
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::RemoveRule => {
                        if let Some(i) = state.gpu_rule_selected.take() {
                            if i < state.gpu_rules.len() {
                                state.gpu_rules.remove(i);
                            }
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::PrevRuleGpu | Hit::NextRuleGpu => {
                        let n = state.gpus.len();
                        if let Some(r) = state.gpu_rule_selected.and_then(|i| state.gpu_rules.get_mut(i)) {
                            r.gpu = if matches!(hit, Hit::NextRuleGpu) { (r.gpu + 1) % n } else { (r.gpu + n - 1) % n };
                            state.applied = false;
                            paint(&state)?;
                        }
                    }
                    Hit::Apply => {
                        let _ = save_config(&state);
                        state.gpu_desktop_saved = state.gpu_desktop;
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
            // Tab moves between the controls and Space or Enter presses
            // one; Enter with none chosen is OK.
            Event::KeyPress(e) => match (keymap.typed(&e), ferro_ui::focus_key(&e, keymap.typed(&e), state.focus, &view::controls(&state))) {
                (_, FocusKey::Moved(f)) => {
                    state.focus = f;
                    paint(&state)?;
                }
                (_, FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                (key, _) if ferro_ui::dialog_answer(key) == Some(true) => {
                    let _ = save_config(&state);
                    let _ = notify_wm(&conn, root, &state);
                    break;
                }
                (key, _) if ferro_ui::dialog_answer(key) == Some(false) => break,
                _ => {}
            },
            Event::ClientMessage(_) => break,
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards() -> Vec<GpuItem> {
        ["gpu0\t0000:0a:00.0\tamdgpu\tmonitor\tAMD Raphael", "gpu1\t0000:03:00.0\tnouveau\t-\tNVIDIA TU116"]
            .iter()
            .filter_map(|l| parse_gpu_line(l))
            .collect()
    }

    #[test]
    fn reads_ferro_gpu_list() {
        let g = cards();
        assert_eq!(g.len(), 2);
        assert!(g[0].monitor && !g[1].monitor);
        assert_eq!(g[1].driver, "nouveau");
        assert_eq!(gpu_index(&g, "gpu1"), Some(1));
        assert_eq!(gpu_index(&g, "0000:0a:00.0"), Some(0));
        assert_eq!(gpu_index(&g, "gpu2"), None);
    }

    #[test]
    fn settings_round_trip() {
        let mut state = State { gpus: cards(), ..State::default() };
        parse_gpu_config(&mut state, "desktop = 0000:03:00.0\n", "apps = gpu0\nwine* = gpu1 # games\nsteam = gpu1\nbogus = gpu7\n");
        assert_eq!(state.gpu_desktop, Some(1));
        assert_eq!(state.gpu_apps, 0);
        assert_eq!(state.gpu_rules, vec![GpuRule { program: "wine*".into(), gpu: 1 }, GpuRule { program: "steam".into(), gpu: 1 }]);
        let text = gpu_config_text(&state);
        let mut again = State { gpus: cards(), ..State::default() };
        parse_gpu_config(&mut again, "desktop = auto\n", &text);
        assert_eq!(again.gpu_rules, state.gpu_rules);
        assert_eq!(again.gpu_desktop, None);
    }

    #[test]
    fn program_names() {
        assert!(valid_program("wine*"));
        assert!(valid_program("steam"));
        assert!(!valid_program(""));
        assert!(!valid_program("wine = gpu0\napps"));
        assert!(!valid_program("rm -rf"));
    }
}
