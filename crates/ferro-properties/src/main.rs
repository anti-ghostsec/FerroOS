//! ferro-properties: FerroOS Heavy's Windows 95 File & Folder Property Sheets.
//!
//! Provides an authentic Windows 95 Properties dialog for desktop items and
//! files/folders in File Explorer. Displays item metadata, size on disk,
//! modification times, folder item counts, and allows toggling file attributes.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_W};
use ferro_ui::{button, Res};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 360;
const H: i32 = 450;

const OK_BTN: Rect = Rect::new(W - 245, H - 32, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 165, H - 32, 75, 24);
const APPLY_BTN: Rect = Rect::new(W - 85, H - 32, 75, 24);

const TAB_RECT: Rect = Rect::new(10, 8, 80, 22);
const FRAME_RECT: Rect = Rect::new(10, 29, W - 20, H - 70);

const CHECK_RO: Rect = Rect::new(95, 345, 13, 13);
const CHECK_HID: Rect = Rect::new(200, 345, 13, 13);

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB ({bytes} bytes)", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB ({bytes} bytes)", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB ({bytes} bytes)", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_time(sec: i64) -> String {
    #[cfg(unix)]
    {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&sec, &mut tm) };
        let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let m = months.get(tm.tm_mon as usize).copied().unwrap_or("");
        let h12 = if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 };
        let ampm = if tm.tm_hour < 12 { "AM" } else { "PM" };
        format!("{}, {} {}, {:02}:{:02}:{:02} {}", m, tm.tm_mday, 1900 + tm.tm_year, h12, tm.tm_min, tm.tm_sec, ampm)
    }
    #[cfg(not(unix))]
    {
        let _ = sec;
        "Unknown".into()
    }
}

struct ItemProps {
    path: PathBuf,
    name: String,
    is_dir: bool,
    size_bytes: u64,
    size_blocks: u64,
    file_count: usize,
    folder_count: usize,
    modified_str: String,
    created_str: String,
    type_str: String,
    opens_with: String,
    read_only: bool,
    hidden: bool,
    applied: bool,
}

impl ItemProps {
    fn load(path: &Path) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let meta = fs::metadata(path).ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let mut size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mut size_blocks = 0;
        let mut file_count = 0;
        let mut folder_count = 0;

        #[cfg(unix)]
        if let Some(ref m) = meta {
            use std::os::unix::fs::MetadataExt;
            size_blocks = m.blocks() * 512;
        }

        if is_dir {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_dir() {
                            folder_count += 1;
                        } else {
                            file_count += 1;
                            if let Ok(m) = entry.metadata() {
                                size_bytes += m.len();
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::MetadataExt;
                                    size_blocks += m.blocks() * 512;
                                }
                            }
                        }
                    }
                }
            }
        }

        let modified_str = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| format_time(d.as_secs() as i64))
            .unwrap_or_else(|| "Unknown".into());

        let created_str = meta
            .as_ref()
            .and_then(|m| m.created().or_else(|_| m.modified()).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| format_time(d.as_secs() as i64))
            .unwrap_or_else(|| modified_str.clone());

        let read_only = meta.as_ref().map(|m| m.permissions().readonly()).unwrap_or(false);
        let hidden = name.starts_with('.');

        let (type_str, opens_with) = if is_dir {
            ("File Folder".to_string(), "File Explorer".to_string())
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "txt" | "md" | "log" | "ini" | "conf" => ("Text Document".to_string(), "Notepad / Text Editor".to_string()),
                "png" | "jpg" | "jpeg" | "bmp" | "gif" => ("Image File".to_string(), "Image Viewer".to_string()),
                "sh" | "bash" => ("Shell Script".to_string(), "Terminal".to_string()),
                "desktop" => ("Desktop Shortcut".to_string(), "Application Launcher".to_string()),
                "zip" | "tar" | "gz" | "xz" => ("Compressed Archive".to_string(), "File Explorer".to_string()),
                "pdf" => ("PDF Document".to_string(), "Document Viewer".to_string()),
                _ => ("File".to_string(), "Associated Program".to_string()),
            }
        } else {
            let is_exec = meta.as_ref().map(|m| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                false
            }).unwrap_or(false);
            if is_exec {
                ("Application / Executable".to_string(), "Direct Execution".to_string())
            } else {
                ("File".to_string(), "Unknown Application".to_string())
            }
        };

        Self {
            path: path.to_path_buf(),
            name,
            is_dir,
            size_bytes,
            size_blocks,
            file_count,
            folder_count,
            modified_str,
            created_str,
            type_str,
            opens_with,
            read_only,
            hidden,
            applied: true,
        }
    }

    fn apply(&mut self) {
        if let Ok(meta) = fs::metadata(&self.path) {
            let mut perms = meta.permissions();
            perms.set_readonly(self.read_only);
            let _ = fs::set_permissions(&self.path, perms);
        }
        self.applied = true;
    }
}

fn draw_separator(s: &mut Surface, y: i32) {
    s.hline(24, y, W - 48, color::SHADOW);
    s.hline(24, y + 1, W - 48, color::WHITE);
}

fn draw(s: &mut Surface, props: &ItemProps) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);

    // Tab Header
    s.bevel(TAB_RECT, Bevel::Raised);
    s.text(TAB_RECT.x + 16, TAB_RECT.y + 4, "General", color::BLACK);

    // Frame
    s.bevel(FRAME_RECT, Bevel::Raised);

    // Top icon and name
    let icon = if props.is_dir {
        icons::FOLDER
    } else if props.type_str.contains("Application") {
        icons::COMPUTER
    } else {
        icons::FILE
    };
    s.sprite(26, 42, icon, 2);

    let name_box = Rect::new(70, 46, W - 100, 22);
    s.bevel(name_box, Bevel::Sunken);
    s.fill(name_box.inset(1), color::WHITE);
    let label = ferro_gfx::ellipsize(&props.name, ((name_box.w - 8) / GLYPH_W) as usize);
    s.text(name_box.x + 4, name_box.y + 3, &label, color::BLACK);

    draw_separator(s, 82);

    // Type of file
    s.text(26, 94, "Type:", color::SHADOW);
    s.text(105, 94, &props.type_str, color::BLACK);

    // Opens with
    s.text(26, 114, "Opens with:", color::SHADOW);
    s.text(105, 114, &props.opens_with, color::BLACK);

    draw_separator(s, 136);

    // Location
    s.text(26, 148, "Location:", color::SHADOW);
    let parent = props.path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into());
    let loc = ferro_gfx::ellipsize(&parent, ((W - 140) / GLYPH_W) as usize);
    s.text(105, 148, &loc, color::BLACK);

    // Size
    s.text(26, 170, "Size:", color::SHADOW);
    s.text(105, 170, &human_size(props.size_bytes), color::BLACK);

    // Size on disk
    s.text(26, 192, "Size on disk:", color::SHADOW);
    s.text(105, 192, &human_size(props.size_blocks.max(props.size_bytes)), color::BLACK);

    // If folder, Contains
    if props.is_dir {
        s.text(26, 214, "Contains:", color::SHADOW);
        let contains = format!("{} Files, {} Folders", props.file_count, props.folder_count);
        s.text(105, 214, &contains, color::BLACK);
    }

    draw_separator(s, 238);

    // Created
    s.text(26, 250, "Created:", color::SHADOW);
    s.text(105, 250, &props.created_str, color::BLACK);

    // Modified
    s.text(26, 272, "Modified:", color::SHADOW);
    s.text(105, 272, &props.modified_str, color::BLACK);

    draw_separator(s, 298);

    // Attributes
    s.text(26, 314, "Attributes:", color::SHADOW);

    // Read-only checkbox
    s.bevel(CHECK_RO, Bevel::Sunken);
    s.fill(CHECK_RO.inset(1), color::WHITE);
    if props.read_only {
        s.text(CHECK_RO.x + 3, CHECK_RO.y + 1, "v", color::BLACK);
    }
    s.text(CHECK_RO.right() + 6, CHECK_RO.y + 1, "Read-only", color::BLACK);

    // Hidden checkbox
    s.bevel(CHECK_HID, Bevel::Sunken);
    s.fill(CHECK_HID.inset(1), color::WHITE);
    if props.hidden {
        s.text(CHECK_HID.x + 3, CHECK_HID.y + 1, "v", color::BLACK);
    }
    s.text(CHECK_HID.right() + 6, CHECK_HID.y + 1, "Hidden", color::BLACK);

    // Bottom buttons
    button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", !props.applied);
}

fn main() -> Res<()> {
    let arg = env::args().nth(1);
    let target_path = match arg {
        Some(a) => PathBuf::from(a),
        None => env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut props = ItemProps::load(&target_path);

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id()?;

    let events = EventMask::EXPOSURE
        | EventMask::BUTTON_PRESS
        | EventMask::KEY_PRESS
        | EventMask::STRUCTURE_NOTIFY;

    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        150,
        120,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(events),
    )?;

    let title = format!("{} Properties", props.name);
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
    let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);

    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;

    conn.map_window(win)?;
    conn.flush()?;

    let paint = |props: &ItemProps| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, props);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    paint(&props)?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(e) if e.count == 0 => paint(&props)?,
            Event::ButtonPress(e) => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                if e.detail == 1 {
                    if OK_BTN.contains(x, y) {
                        props.apply();
                        return Ok(());
                    }
                    if CANCEL_BTN.contains(x, y) {
                        return Ok(());
                    }
                    if APPLY_BTN.contains(x, y) {
                        props.apply();
                        paint(&props)?;
                    }
                    if CHECK_RO.inset(-4).contains(x, y) {
                        props.read_only = !props.read_only;
                        props.applied = false;
                        paint(&props)?;
                    }
                }
            }
            Event::KeyPress(e) => {
                // Enter = OK, Escape = Cancel
                if e.detail == 36 || e.detail == 104 {
                    props.apply();
                    return Ok(());
                }
                if e.detail == 9 {
                    return Ok(());
                }
            }
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => {
                return Ok(());
            }
            _ => {}
        }
    }
}
