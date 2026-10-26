//! ferro-properties: FerroOS Heavy's Windows 95 File & Folder Property Sheets.
//!
//! Provides an authentic Windows 95 Properties dialog for desktop items and
//! files/folders in File Explorer. Displays item metadata, size on disk,
//! modification times, folder item counts, editable filename, working Hidden
//! and Read-only attributes, file associations, and full highlight/copy support.

use ferro_gfx::{color, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, text_field_with_cursor, Key, Keymap, Res};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 380;
const H: i32 = 460;

const OK_BTN: Rect = Rect::new(W - 245, H - 32, 72, 24);
const CANCEL_BTN: Rect = Rect::new(W - 165, H - 32, 72, 24);
const APPLY_BTN: Rect = Rect::new(W - 85, H - 32, 72, 24);
const COPY_BTN: Rect = Rect::new(12, H - 32, 78, 24);

const TAB_RECT: Rect = Rect::new(10, 8, 80, 22);
const FRAME_RECT: Rect = Rect::new(10, 29, W - 20, H - 70);

const NAME_BOX: Rect = Rect::new(70, 46, W - 95, 22);
const CHANGE_BTN: Rect = Rect::new(W - 90, 110, 68, 22);

const CHECK_RO: Rect = Rect::new(95, 345, 13, 13);
const CHECK_RO_LABEL: Rect = Rect::new(95, 340, 95, 22);
const CHECK_HID: Rect = Rect::new(205, 345, 13, 13);
const CHECK_HID_LABEL: Rect = Rect::new(205, 340, 85, 22);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowId {
    Name,
    Type,
    OpensWith,
    Location,
    Size,
    SizeOnDisk,
    Contains,
    Created,
    Modified,
    Attributes,
}

fn row_rect(row: RowId) -> Rect {
    match row {
        RowId::Name => NAME_BOX,
        RowId::Type => Rect::new(24, 90, W - 48, 18),
        RowId::OpensWith => Rect::new(24, 110, W - 120, 20),
        RowId::Location => Rect::new(24, 144, W - 48, 18),
        RowId::Size => Rect::new(24, 166, W - 48, 18),
        RowId::SizeOnDisk => Rect::new(24, 188, W - 48, 18),
        RowId::Contains => Rect::new(24, 210, W - 48, 18),
        RowId::Created => Rect::new(24, 246, W - 48, 18),
        RowId::Modified => Rect::new(24, 268, W - 48, 18),
        RowId::Attributes => Rect::new(24, 310, W - 48, 18),
    }
}

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

fn copy_to_clipboard(text: &str) {
    for sel in ["clipboard", "primary"] {
        if let Ok(mut child) = std::process::Command::new("xclip").args(["-selection", sel, "-in"]).stdin(std::process::Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

fn paste_from_clipboard() -> Option<String> {
    if let Ok(out) = std::process::Command::new("xclip").args(["-selection", "clipboard", "-out"]).output() {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let trimmed = s.trim_matches(|c| c == '\r' || c == '\n');
                return Some(trimmed.to_string());
            }
        }
    }
    None
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

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let (type_str, opens_with) = if is_dir {
            ("File Folder".to_string(), "File Explorer".to_string())
        } else if !ext.is_empty() {
            let loaded_entries = ferro_associations::load_associations();
            let app_cmd = loaded_entries
                .iter()
                .find(|e| e.ext.eq_ignore_ascii_case(&ext))
                .map(|e| e.command.replace("%f", "").trim().to_string())
                .unwrap_or_else(|| ferro_associations::resolve_command_for_path(path).replace("%f", "").trim().to_string());

            let desc = match ext.as_str() {
                "txt" | "md" | "log" | "ini" | "conf" => "Text Document".to_string(),
                "png" | "jpg" | "jpeg" | "bmp" | "gif" => "Image File".to_string(),
                "sh" | "bash" => "Shell Script".to_string(),
                "desktop" => "Desktop Shortcut".to_string(),
                "zip" | "tar" | "gz" | "xz" => "Compressed Archive".to_string(),
                "pdf" => "PDF Document".to_string(),
                "exe" => "Windows Executable".to_string(),
                _ => format!("{} File", ext.to_uppercase()),
            };
            (desc, app_cmd)
        } else {
            let is_exec = meta
                .as_ref()
                .map(|m| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    false
                })
                .unwrap_or(false);
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

    fn row_value(&self, row: RowId) -> String {
        match row {
            RowId::Name => self.name.clone(),
            RowId::Type => self.type_str.clone(),
            RowId::OpensWith => self.opens_with.clone(),
            RowId::Location => self.path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into()),
            RowId::Size => human_size(self.size_bytes),
            RowId::SizeOnDisk => human_size(self.size_blocks.max(self.size_bytes)),
            RowId::Contains => {
                if self.is_dir {
                    format!("{} Files, {} Folders", self.file_count, self.folder_count)
                } else {
                    String::new()
                }
            }
            RowId::Created => self.created_str.clone(),
            RowId::Modified => self.modified_str.clone(),
            RowId::Attributes => {
                format!("Read-only: {}, Hidden: {}", if self.read_only { "Yes" } else { "No" }, if self.hidden { "Yes" } else { "No" })
            }
        }
    }

    fn all_details(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("File: {}", self.name));
        lines.push(format!("Type: {}", self.type_str));
        lines.push(format!("Opens with: {}", self.opens_with));
        lines.push(format!("Location: {}", self.path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into())));
        lines.push(format!("Size: {}", human_size(self.size_bytes)));
        lines.push(format!("Size on disk: {}", human_size(self.size_blocks.max(self.size_bytes))));
        if self.is_dir {
            lines.push(format!("Contains: {} Files, {} Folders", self.file_count, self.folder_count));
        }
        lines.push(format!("Created: {}", self.created_str));
        lines.push(format!("Modified: {}", self.modified_str));
        lines.push(format!(
            "Attributes: Read-only: {}, Hidden: {}",
            if self.read_only { "Yes" } else { "No" },
            if self.hidden { "Yes" } else { "No" }
        ));
        lines.join("\n")
    }

    fn apply_changes(&mut self, new_name: &str, conn: &RustConnection, root: Window, ferro_refresh: Atom) {
        let clean_name = new_name.trim();
        let target_name = if clean_name.is_empty() { self.name.clone() } else { clean_name.to_string() };

        // Adjust dotfile prefix according to hidden checkbox
        let final_name = if self.hidden {
            if !target_name.starts_with('.') {
                format!(".{target_name}")
            } else {
                target_name
            }
        } else if target_name.starts_with('.') {
            target_name.trim_start_matches('.').to_string()
        } else {
            target_name
        };

        // Rename file if name or hidden status changed
        if !final_name.is_empty() && final_name != self.name {
            if let Some(parent) = self.path.parent() {
                let new_path = parent.join(&final_name);
                if fs::rename(&self.path, &new_path).is_ok() {
                    self.path = new_path;
                    self.name = final_name;
                }
            }
        }

        // Apply read-only permission change
        if let Ok(meta) = fs::metadata(&self.path) {
            let mut perms = meta.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut mode = perms.mode();
                if self.read_only {
                    mode &= !0o222; // strip write
                } else {
                    mode |= 0o200; // grant user write
                }
                perms.set_mode(mode);
            }
            #[cfg(not(unix))]
            {
                perms.set_readonly(self.read_only);
            }
            let _ = fs::set_permissions(&self.path, perms);
        }

        // Broadcast refresh event to WM
        let msg = ClientMessageEvent::new(32, root, ferro_refresh, [0, 0, 0, 0, 0]);
        let _ = conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, msg);
        let _ = conn.flush();

        self.applied = true;
    }
}

struct OpenWithDialog {
    ext: String,
    cmd_text: String,
    cmd_cursor: usize,
}

const DLG_RECT: Rect = Rect::new(25, 75, W - 50, 240);
const DLG_CMD_INPUT: Rect = Rect::new(40, 130, W - 80, 22);
const DLG_OK: Rect = Rect::new(W - 190, 280, 68, 24);
const DLG_CANCEL: Rect = Rect::new(W - 110, 280, 68, 24);
const DLG_MORE: Rect = Rect::new(40, 280, 68, 24);

const PRESET_BTNS: [(Rect, &str, &str); 5] = [
    (Rect::new(40, 165, 60, 22), "Geany", "geany %f"),
    (Rect::new(105, 165, 55, 22), "Wine", "wine %f"),
    (Rect::new(165, 165, 55, 22), "MPV", "mpv %f"),
    (Rect::new(225, 165, 65, 22), "Viewnior", "viewnior %f"),
    (Rect::new(40, 195, 75, 22), "LibreWolf", "librewolf %f"),
];

fn draw_separator(s: &mut Surface, y: i32) {
    s.hline(24, y, W - 48, color::SHADOW);
    s.hline(24, y + 1, W - 48, color::WHITE);
}

struct ViewState<'a> {
    props: &'a ItemProps,
    name_text: &'a str,
    name_cursor: usize,
    name_sel: Option<(usize, usize)>,
    selected_row: Option<RowId>,
    select_all_details: bool,
    copied_time: Option<Instant>,
    dialog: Option<&'a OpenWithDialog>,
}

fn draw(s: &mut Surface, v: &ViewState) {
    let props = v.props;
    let name_text = v.name_text;
    let name_cursor = v.name_cursor;
    let name_sel = v.name_sel;
    let selected_row = v.selected_row;
    let select_all_details = v.select_all_details;
    let copied_time = v.copied_time;
    let dialog = v.dialog;

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

    // Name text field
    let name_focused = selected_row == Some(RowId::Name);
    text_field_with_cursor(s, NAME_BOX, name_text, name_cursor, name_sel, name_focused);

    draw_separator(s, 82);

    let render_val = |s: &mut Surface, row: RowId, y: i32, val: &str| {
        let is_selected = selected_row == Some(row) || select_all_details;
        let tw = text_width(val);
        if is_selected && !val.is_empty() {
            s.fill(Rect::new(103, y - 2, tw + 4, GLYPH_H + 4), color::NAVY);
            s.text(105, y, val, color::WHITE);
        } else {
            s.text(105, y, val, color::BLACK);
        }
    };

    // Type of file
    s.text(26, 94, "Type:", color::SHADOW);
    render_val(s, RowId::Type, 94, &props.type_str);

    // Opens with
    s.text(26, 114, "Opens with:", color::SHADOW);
    let opens_display = ferro_gfx::ellipsize(&props.opens_with, ((W - 200) / GLYPH_W).max(1) as usize);
    render_val(s, RowId::OpensWith, 114, &opens_display);

    if !props.is_dir {
        button(s, CHANGE_BTN, "Change...", true);
    }

    draw_separator(s, 136);

    // Location
    s.text(26, 148, "Location:", color::SHADOW);
    let parent = props.path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into());
    let loc = ferro_gfx::ellipsize(&parent, ((W - 140) / GLYPH_W).max(1) as usize);
    render_val(s, RowId::Location, 148, &loc);

    // Size
    s.text(26, 170, "Size:", color::SHADOW);
    render_val(s, RowId::Size, 170, &human_size(props.size_bytes));

    // Size on disk
    s.text(26, 192, "Size on disk:", color::SHADOW);
    render_val(s, RowId::SizeOnDisk, 192, &human_size(props.size_blocks.max(props.size_bytes)));

    // If folder, Contains
    if props.is_dir {
        s.text(26, 214, "Contains:", color::SHADOW);
        let contains = format!("{} Files, {} Folders", props.file_count, props.folder_count);
        render_val(s, RowId::Contains, 214, &contains);
    }

    draw_separator(s, 238);

    // Created
    s.text(26, 250, "Created:", color::SHADOW);
    render_val(s, RowId::Created, 250, &props.created_str);

    // Modified
    s.text(26, 272, "Modified:", color::SHADOW);
    render_val(s, RowId::Modified, 272, &props.modified_str);

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
    let copy_label = if copied_time.is_some_and(|t| t.elapsed().as_millis() < 1500) { "Copied!" } else { "Copy" };
    button(s, COPY_BTN, copy_label, true);
    button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", !props.applied);

    // Modal "Open With" Dialog
    if let Some(dlg) = dialog {
        s.fill(DLG_RECT, color::FACE);
        s.bevel(DLG_RECT, Bevel::Raised);

        let title_bar = Rect::new(DLG_RECT.x + 3, DLG_RECT.y + 3, DLG_RECT.w - 6, 18);
        s.fill(title_bar, color::NAVY);
        s.text_bold(title_bar.x + 6, title_bar.y + 3, "Open With", color::WHITE);

        s.text(40, 100, &format!("Select application for .{} files:", dlg.ext), color::BLACK);
        text_field_with_cursor(s, DLG_CMD_INPUT, &dlg.cmd_text, dlg.cmd_cursor, None, true);

        s.text(40, 150, "Quick Presets:", color::SHADOW);
        for &(r, label, _) in &PRESET_BTNS {
            button(s, r, label, true);
        }

        button(s, DLG_MORE, "More...", true);
        button(s, DLG_OK, "OK", true);
        button(s, DLG_CANCEL, "Cancel", true);
    }
}

fn main() -> Res<()> {
    let arg = env::args().nth(1);
    let target_path = match arg {
        Some(a) => PathBuf::from(a),
        None => env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut props = ItemProps::load(&target_path);
    let mut name_text = props.name.clone();

    // Select the file stem initially like Windows 11
    let stem_len = match Path::new(&name_text).file_stem().and_then(|s| s.to_str()) {
        Some(s) if !props.is_dir => s.len(),
        _ => name_text.len(),
    };
    let mut name_cursor = stem_len;
    let mut name_sel: Option<(usize, usize)> = if stem_len > 0 { Some((0, stem_len)) } else { None };

    let mut selected_row: Option<RowId> = Some(RowId::Name);
    let mut select_all_details = false;
    let mut copied_time: Option<Instant> = None;
    let mut dialog: Option<OpenWithDialog> = None;

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id()?;

    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;

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
    let ferro_refresh = atom("_FERRO_REFRESH")?;

    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;

    let keymap = Keymap::load(&conn)?;

    conn.map_window(win)?;
    conn.flush()?;

    let repaint = |props: &ItemProps,
                   name_text: &str,
                   name_cursor: usize,
                   name_sel: Option<(usize, usize)>,
                   selected_row: Option<RowId>,
                   select_all_details: bool,
                   copied_time: Option<Instant>,
                   dialog: Option<&OpenWithDialog>|
     -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        let view = ViewState { props, name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog };
        draw(&mut s, &view);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog.as_ref())?;

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(e) if e.count == 0 => {
                repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog.as_ref())?;
            }
            Event::ButtonPress(e) => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                if e.detail == 1 {
                    // Modal Dialog Click Handling
                    if let Some(ref mut dlg) = dialog {
                        if DLG_CANCEL.contains(x, y) {
                            dialog = None;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                            continue;
                        }
                        if DLG_MORE.contains(x, y) {
                            let _ = std::process::Command::new("ferro-associations").arg(&dlg.ext).spawn();
                            dialog = None;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                            continue;
                        }
                        if DLG_OK.contains(x, y) {
                            let new_cmd = dlg.cmd_text.trim();
                            if !new_cmd.is_empty() {
                                let mut entries = ferro_associations::load_associations();
                                let cmd = if new_cmd.contains("%f") { new_cmd.to_string() } else { format!("{new_cmd} %f") };
                                let desc = format!("{} File", dlg.ext.to_uppercase());
                                if let Some(existing) = entries.iter_mut().find(|e| e.ext.eq_ignore_ascii_case(&dlg.ext)) {
                                    existing.command = cmd.clone();
                                } else {
                                    entries.push(ferro_associations::AssociationEntry { ext: dlg.ext.clone(), desc, command: cmd.clone() });
                                }
                                ferro_associations::save_associations(&entries);
                                props.opens_with = cmd.replace("%f", "").trim().to_string();
                                props.applied = false;
                            }
                            dialog = None;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                            continue;
                        }
                        for &(r, _, cmd) in &PRESET_BTNS {
                            if r.contains(x, y) {
                                dlg.cmd_text = cmd.to_string();
                                dlg.cmd_cursor = dlg.cmd_text.len();
                                break;
                            }
                        }
                        if DLG_CMD_INPUT.contains(x, y) {
                            dlg.cmd_cursor = dlg.cmd_text.len();
                        }
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, Some(dlg))?;
                        continue;
                    }

                    // Main Window Click Handling
                    if OK_BTN.contains(x, y) {
                        props.apply_changes(&name_text, &conn, screen.root, ferro_refresh);
                        return Ok(());
                    }
                    if CANCEL_BTN.contains(x, y) {
                        return Ok(());
                    }
                    if APPLY_BTN.contains(x, y) {
                        props.apply_changes(&name_text, &conn, screen.root, ferro_refresh);
                        let new_title = format!("{} Properties", props.name);
                        let _ = conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, new_title.as_bytes());
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if COPY_BTN.contains(x, y) {
                        let to_copy = if let Some(row) = selected_row {
                            if row == RowId::Name {
                                if let Some((s, e)) = name_sel {
                                    let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                    name_text[min_s..max_e].to_string()
                                } else {
                                    name_text.clone()
                                }
                            } else {
                                props.row_value(row)
                            }
                        } else {
                            props.all_details()
                        };
                        copy_to_clipboard(&to_copy);
                        copied_time = Some(Instant::now());
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if !props.is_dir && CHANGE_BTN.contains(x, y) {
                        let ext = props.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let cur_cmd = if props.opens_with.contains("%f") { props.opens_with.clone() } else { format!("{} %f", props.opens_with) };
                        dialog = Some(OpenWithDialog { ext, cmd_cursor: cur_cmd.len(), cmd_text: cur_cmd });
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog.as_ref())?;
                        continue;
                    }
                    if CHECK_RO_LABEL.contains(x, y) {
                        props.read_only = !props.read_only;
                        props.applied = false;
                        selected_row = Some(RowId::Attributes);
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if CHECK_HID_LABEL.contains(x, y) {
                        props.hidden = !props.hidden;
                        props.applied = false;
                        selected_row = Some(RowId::Attributes);
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }

                    // Name Box Focus / Click
                    if NAME_BOX.contains(x, y) {
                        selected_row = Some(RowId::Name);
                        select_all_details = false;
                        let rel_x = x - (NAME_BOX.x + 5);
                        let char_idx = (rel_x / GLYPH_W).max(0) as usize;
                        name_cursor = char_idx.min(name_text.len());
                        name_sel = None;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }

                    // Metadata Row Click -> Select & Highlight
                    let mut clicked_row = None;
                    for row in [
                        RowId::Type,
                        RowId::OpensWith,
                        RowId::Location,
                        RowId::Size,
                        RowId::SizeOnDisk,
                        RowId::Contains,
                        RowId::Created,
                        RowId::Modified,
                        RowId::Attributes,
                    ] {
                        if row_rect(row).contains(x, y) {
                            clicked_row = Some(row);
                            break;
                        }
                    }

                    if let Some(row) = clicked_row {
                        selected_row = Some(row);
                        select_all_details = false;
                        name_sel = None;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    } else if FRAME_RECT.contains(x, y) {
                        // Click on background of frame: deselect individual row
                        selected_row = None;
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    }
                }
            }
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                let ctrl = e.state.contains(KeyButMask::CONTROL);

                // Modal Dialog Key Handling
                if let Some(ref mut dlg) = dialog {
                    let mut closed = false;
                    match key {
                        Key::Escape => {
                            closed = true;
                        }
                        Key::Enter => {
                            let new_cmd = dlg.cmd_text.trim();
                            if !new_cmd.is_empty() {
                                let mut entries = ferro_associations::load_associations();
                                let cmd = if new_cmd.contains("%f") { new_cmd.to_string() } else { format!("{new_cmd} %f") };
                                let desc = format!("{} File", dlg.ext.to_uppercase());
                                if let Some(existing) = entries.iter_mut().find(|e| e.ext.eq_ignore_ascii_case(&dlg.ext)) {
                                    existing.command = cmd.clone();
                                } else {
                                    entries.push(ferro_associations::AssociationEntry { ext: dlg.ext.clone(), desc, command: cmd.clone() });
                                }
                                ferro_associations::save_associations(&entries);
                                props.opens_with = cmd.replace("%f", "").trim().to_string();
                                props.applied = false;
                            }
                            closed = true;
                        }
                        Key::Backspace => {
                            if dlg.cmd_cursor > 0 && dlg.cmd_cursor <= dlg.cmd_text.len() {
                                dlg.cmd_text.remove(dlg.cmd_cursor - 1);
                                dlg.cmd_cursor -= 1;
                            }
                        }
                        Key::Delete => {
                            if dlg.cmd_cursor < dlg.cmd_text.len() {
                                dlg.cmd_text.remove(dlg.cmd_cursor);
                            }
                        }
                        Key::Left => {
                            if dlg.cmd_cursor > 0 {
                                dlg.cmd_cursor -= 1;
                            }
                        }
                        Key::Right => {
                            if dlg.cmd_cursor < dlg.cmd_text.len() {
                                dlg.cmd_cursor += 1;
                            }
                        }
                        Key::Home => {
                            dlg.cmd_cursor = 0;
                        }
                        Key::End => {
                            dlg.cmd_cursor = dlg.cmd_text.len();
                        }
                        Key::Char(c) if !c.is_control() => {
                            if ctrl && (c == 'v' || c == 'V') {
                                if let Some(pasted) = paste_from_clipboard() {
                                    dlg.cmd_text.insert_str(dlg.cmd_cursor, &pasted);
                                    dlg.cmd_cursor += pasted.len();
                                }
                            } else if !ctrl {
                                dlg.cmd_text.insert(dlg.cmd_cursor, c);
                                dlg.cmd_cursor += 1;
                            }
                        }
                        _ => {}
                    }
                    if closed {
                        dialog = None;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    } else {
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, Some(dlg))?;
                    }
                    continue;
                }

                // Global Shortcuts
                if ctrl && (key == Key::Char('c') || key == Key::Char('C')) {
                    let to_copy = if let Some(row) = selected_row {
                        if row == RowId::Name {
                            if let Some((s, e)) = name_sel {
                                let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                name_text[min_s..max_e].to_string()
                            } else {
                                name_text.clone()
                            }
                        } else {
                            props.row_value(row)
                        }
                    } else {
                        props.all_details()
                    };
                    copy_to_clipboard(&to_copy);
                    copied_time = Some(Instant::now());
                    repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    continue;
                }

                if ctrl && (key == Key::Char('a') || key == Key::Char('A')) {
                    if selected_row == Some(RowId::Name) {
                        name_sel = Some((0, name_text.len()));
                        name_cursor = name_text.len();
                    } else {
                        select_all_details = true;
                        selected_row = None;
                    }
                    repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    continue;
                }

                if key == Key::Enter {
                    props.apply_changes(&name_text, &conn, screen.root, ferro_refresh);
                    return Ok(());
                }

                if key == Key::Escape {
                    return Ok(());
                }

                // Name Box Text Editing
                if selected_row == Some(RowId::Name) {
                    match key {
                        Key::Backspace => {
                            if let Some((s, e)) = name_sel.take() {
                                let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                name_text.drain(min_s..max_e);
                                name_cursor = min_s;
                                props.applied = false;
                            } else if name_cursor > 0 && name_cursor <= name_text.len() {
                                name_text.remove(name_cursor - 1);
                                name_cursor -= 1;
                                props.applied = false;
                            }
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::Delete => {
                            if let Some((s, e)) = name_sel.take() {
                                let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                name_text.drain(min_s..max_e);
                                name_cursor = min_s;
                                props.applied = false;
                            } else if name_cursor < name_text.len() {
                                name_text.remove(name_cursor);
                                props.applied = false;
                            }
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::Left => {
                            name_sel = None;
                            name_cursor = name_cursor.saturating_sub(1);
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::Right => {
                            name_sel = None;
                            if name_cursor < name_text.len() {
                                name_cursor += 1;
                            }
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::Home => {
                            name_sel = None;
                            name_cursor = 0;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::End => {
                            name_sel = None;
                            name_cursor = name_text.len();
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        Key::Char(c) if !c.is_control() => {
                            if ctrl && (c == 'v' || c == 'V') {
                                if let Some(pasted) = paste_from_clipboard() {
                                    if let Some((s, e)) = name_sel.take() {
                                        let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                        name_text.drain(min_s..max_e);
                                        name_cursor = min_s;
                                    }
                                    name_text.insert_str(name_cursor, &pasted);
                                    name_cursor += pasted.len();
                                    props.applied = false;
                                }
                            } else if !ctrl {
                                if let Some((s, e)) = name_sel.take() {
                                    let (min_s, max_e) = (s.min(e).min(name_text.len()), s.max(e).min(name_text.len()));
                                    name_text.drain(min_s..max_e);
                                    name_cursor = min_s;
                                }
                                name_text.insert(name_cursor, c);
                                name_cursor += 1;
                                props.applied = false;
                            }
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        }
                        _ => {}
                    }
                }
            }
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => {
                return Ok(());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(500), "500 bytes");
        assert_eq!(human_size(2048), "2.0 KB (2048 bytes)");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB (5242880 bytes)");
    }

    #[test]
    fn test_row_value_and_all_details() {
        let temp = std::env::temp_dir().join(format!("ferro_prop_test_{}.txt", std::process::id()));
        let _ = fs::write(&temp, "Hello World FerroOS");
        let props = ItemProps::load(&temp);

        assert_eq!(props.row_value(RowId::Type), "Text Document");
        assert!(props.row_value(RowId::Size).contains("19 bytes"));
        let details = props.all_details();
        assert!(details.contains("Type: Text Document"));
        assert!(details.contains("Size: 19 bytes"));
        assert!(details.contains("Location:"));

        let _ = fs::remove_file(&temp);
    }

    #[test]
    fn test_hidden_attribute_detection() {
        let temp = std::env::temp_dir().join(format!(".ferro_hidden_{}", std::process::id()));
        let _ = fs::write(&temp, "hidden");
        let props = ItemProps::load(&temp);
        assert!(props.hidden);
        let _ = fs::remove_file(&temp);
    }
}
