//! ferro-properties: FerroOS Heavy's Windows 95 File & Folder Property Sheets.
//!
//! Provides an authentic Windows 95 Properties dialog for desktop items and
//! files/folders in File Explorer. Displays item metadata, size on disk,
//! modification times, folder item counts, editable filename, working Hidden
//! and Read-only attributes, file associations, and full highlight/copy support.
//! A folder can be made Restricted: given to root, so nothing running as you
//! opens it without your password (see ferro-restrict).
//!
//! Given several files and folders (`ferro-properties A B C`, what's
//! selected), the same sheet shows them together and its changes (Read-only,
//! Hidden, Restricted, Opens with) go to every one of them.

use ferro_gfx::{color, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, text_field_with_cursor, Key, Keymap, Res};
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
/// The Info tab: what the file carries (ferro-info's panel), files only.
const INFO_TAB: Rect = Rect::new(90, 10, 56, 20);
const INFO_AREA: Rect = Rect::new(22, 42, W - 44, H - 92);

/// The two tabs, `info` the one in front.
fn draw_tabs(s: &mut Surface, info: bool, with_info: bool) {
    let (front, back) =
        if info { (Rect::new(INFO_TAB.x, 8, INFO_TAB.w, 22), Rect::new(TAB_RECT.x, 10, TAB_RECT.w, 20)) } else { (TAB_RECT, INFO_TAB) };
    if with_info {
        s.bevel(back, Bevel::Raised);
    }
    s.bevel(front, Bevel::Raised);
    s.text(TAB_RECT.x + 16, TAB_RECT.y + 4, "General", color::BLACK);
    if with_info {
        s.text(INFO_TAB.x + 14, INFO_TAB.y + 3, "Info", color::BLACK);
    }
}

/// The Info tab in front: the panel, and OK, Cancel and Apply as ever.
fn draw_info(s: &mut Surface, panel: &ferro_info::Panel, applied: bool) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.bevel(FRAME_RECT, Bevel::Raised);
    draw_tabs(s, true, true);
    panel.draw(s, INFO_AREA);
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", !applied);
}
const FRAME_RECT: Rect = Rect::new(10, 29, W - 20, H - 70);

const NAME_BOX: Rect = Rect::new(70, 46, W - 95, 22);
const CHANGE_BTN: Rect = Rect::new(W - 90, 110, 68, 22);

const CHECK_RO: Rect = Rect::new(95, 345, 13, 13);
const CHECK_RO_LABEL: Rect = Rect::new(95, 340, 95, 22);
const CHECK_HID: Rect = Rect::new(205, 345, 13, 13);
const CHECK_HID_LABEL: Rect = Rect::new(205, 340, 85, 22);
/// Folders only: Restricted, and under it what became of it.
const CHECK_RES: Rect = Rect::new(95, 368, 13, 13);
const CHECK_RES_LABEL: Rect = Rect::new(95, 363, 110, 22);
const RES_NOTE_Y: i32 = 388;

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

fn comma_number(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut parts = Vec::new();
    while n > 0 {
        let rem = n % 1000;
        n /= 1000;
        if n > 0 {
            parts.push(format!("{rem:03}"));
        } else {
            parts.push(format!("{rem}"));
        }
    }
    parts.reverse();
    parts.join(",")
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct DriveProps {
    path: PathBuf,
    label: String,
    letter: char,
    fs_type: String,
    used_bytes: u64,
    free_bytes: u64,
    total_bytes: u64,
}

fn is_drive(path: &Path) -> bool {
    let p_str = path.to_string_lossy();
    if path == Path::new("/") || p_str == "C:" || p_str == "C:\\" || p_str.starts_with("Local Disk") {
        return true;
    }
    if let Ok(content) = fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && Path::new(parts[1]) == path {
                return true;
            }
        }
    }
    false
}

impl DriveProps {
    fn load(path: &Path) -> Self {
        let mut total = 0u64;
        let mut free = 0u64;
        #[cfg(unix)]
        unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default();
            if libc::statvfs(c_path.as_ptr(), &mut st) == 0 {
                let bsize = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize } as u64;
                total = st.f_blocks as u64 * bsize;
                free = st.f_bavail as u64 * bsize;
            }
        }
        let used = total.saturating_sub(free);

        let mut fs_type = "ext4".to_string();
        if let Ok(content) = fs::read_to_string("/proc/mounts") {
            for line in content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 && Path::new(parts[1]) == path {
                    fs_type = parts[2].to_string();
                    break;
                }
            }
        }

        let letter = if path == Path::new("/") { 'C' } else { 'D' };
        let label = if path == Path::new("/") { "Local Disk".to_string() } else { "Removable Disk".to_string() };

        Self { path: path.to_path_buf(), label, letter, fs_type, used_bytes: used, free_bytes: free, total_bytes: total }
    }
}

fn calc_dir_stats(path: &Path) -> (u64, u64, usize, usize) {
    let mut total_bytes = 0;
    let mut total_blocks = 0;
    let mut file_count = 0;
    let mut folder_count = 0;
    let mut stack = vec![path.to_path_buf()];

    while let Some(current) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Ok(ft) = entry.file_type() {
                    if ft.is_symlink() {
                        continue;
                    }
                    if ft.is_dir() {
                        folder_count += 1;
                        stack.push(p);
                    } else {
                        file_count += 1;
                        if let Ok(m) = entry.metadata() {
                            total_bytes += m.len();
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::MetadataExt;
                                total_blocks += m.blocks() * 512;
                            }
                        }
                    }
                }
            }
        }
    }
    (total_bytes, total_blocks, file_count, folder_count)
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
    /// Restricted as ticked; `was_restricted` as the folder is now.
    restricted: bool,
    was_restricted: bool,
    /// A folder you can't open (restricted, or not yours to open): padlocked.
    locked: bool,
    /// Why restricting or freeing it didn't happen, said under the box.
    restrict_note: Option<String>,
    applied: bool,
    /// How many items it is about: 1, or the selection's (see `group`).
    count: usize,
    /// All in the one folder (that of `path`).
    same_folder: bool,
    /// A folder among them: no Opens with to change.
    any_dir: bool,
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
            let (b, blk, files, folders) = calc_dir_stats(path);
            size_bytes = b;
            size_blocks = blk;
            file_count = files;
            folder_count = folders;
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
        let restricted = is_dir && ferro_restrict::is_restricted(path);

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let own = if is_dir { None } else { ferro_associations::file_override(path) };
        let (type_str, opens_with) = if is_dir {
            ("File Folder".to_string(), "File Explorer".to_string())
        } else if !ext.is_empty() {
            let loaded_entries = ferro_associations::load_associations();
            let app_cmd = loaded_entries
                .iter()
                .find(|e| e.ext.eq_ignore_ascii_case(&ext))
                .map(|e| shown_command(&e.command))
                .unwrap_or_else(|| shown_command(&ferro_associations::resolve_command_for_path(path)));

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
        // A program chosen for this file alone is what opens it.
        let opens_with = own.map_or(opens_with, |c| shown_command(&c));

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
            restricted,
            was_restricted: restricted,
            locked: is_dir && ferro_restrict::is_locked(path),
            restrict_note: None,
            applied: true,
            count: 1,
            same_folder: true,
            any_dir: is_dir,
        }
    }

    /// The sheet for several items at once: their sizes added up, what they
    /// have in common shown, "Various" where they differ, and a box ticked
    /// only if it is so for every one of them.
    fn group(items: &[ItemProps]) -> Self {
        let first = &items[0];
        let same = |f: fn(&ItemProps) -> &str| if items.iter().all(|i| f(i) == f(first)) { f(first).to_string() } else { "Various".to_string() };
        let folders = items.iter().filter(|i| i.is_dir).count();
        let files = items.len() - folders;
        let all_dirs = folders == items.len();
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let name = match (files, folders) {
            (_, 0) => plural(files, "File", "Files"),
            (0, _) => plural(folders, "Folder", "Folders"),
            _ => format!("{}, {}", plural(files, "File", "Files"), plural(folders, "Folder", "Folders")),
        };
        let type_str = same(|i| &i.type_str);
        Self {
            path: first.path.clone(),
            name,
            is_dir: all_dirs,
            size_bytes: items.iter().map(|i| i.size_bytes).sum(),
            size_blocks: items.iter().map(|i| i.size_blocks).sum(),
            file_count: files + items.iter().map(|i| i.file_count).sum::<usize>(),
            folder_count: folders + items.iter().map(|i| i.folder_count).sum::<usize>(),
            modified_str: same(|i| &i.modified_str),
            created_str: same(|i| &i.created_str),
            type_str: if type_str == "Various" { "Various types".to_string() } else { type_str },
            opens_with: if folders > 0 && !all_dirs { "Various".to_string() } else { same(|i| &i.opens_with) },
            read_only: items.iter().all(|i| i.read_only),
            hidden: items.iter().all(|i| i.hidden),
            restricted: all_dirs && items.iter().all(|i| i.restricted),
            was_restricted: all_dirs && items.iter().all(|i| i.was_restricted),
            locked: false,
            restrict_note: items.iter().find_map(|i| i.restrict_note.clone()),
            applied: true,
            count: items.len(),
            same_folder: items.iter().all(|i| i.path.parent() == first.path.parent()),
            any_dir: folders > 0,
        }
    }

    /// Where it is, as shown.
    fn location(&self) -> String {
        if !self.same_folder {
            return "Various folders".to_string();
        }
        self.path.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into())
    }

    fn row_value(&self, row: RowId) -> String {
        match row {
            RowId::Name => self.name.clone(),
            RowId::Type => self.type_str.clone(),
            RowId::OpensWith => self.opens_with.clone(),
            RowId::Location => self.location(),
            RowId::Size => human_size(self.size_bytes),
            RowId::SizeOnDisk => human_size(self.size_blocks.max(self.size_bytes)),
            RowId::Contains => {
                if self.was_restricted {
                    "Restricted: not looked inside".to_string()
                } else if self.is_dir || self.count > 1 {
                    format!("{} Files, {} Folders", self.file_count, self.folder_count)
                } else {
                    String::new()
                }
            }
            RowId::Created => self.created_str.clone(),
            RowId::Modified => self.modified_str.clone(),
            RowId::Attributes => self.attributes(),
        }
    }

    fn all_details(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("File: {}", self.name));
        lines.push(format!("Type: {}", self.type_str));
        lines.push(format!("Opens with: {}", self.opens_with));
        lines.push(format!("Location: {}", self.location()));
        lines.push(format!("Size: {}", human_size(self.size_bytes)));
        lines.push(format!("Size on disk: {}", human_size(self.size_blocks.max(self.size_bytes))));
        if self.is_dir || self.count > 1 {
            lines.push(format!("Contains: {}", self.row_value(RowId::Contains)));
        }
        lines.push(format!("Created: {}", self.created_str));
        lines.push(format!("Modified: {}", self.modified_str));
        lines.push(format!("Attributes: {}", self.attributes()));
        lines.join("\n")
    }

    fn attributes(&self) -> String {
        let yes = |b: bool| if b { "Yes" } else { "No" };
        let mut said = format!("Read-only: {}, Hidden: {}", yes(self.read_only), yes(self.hidden));
        if self.is_dir {
            said.push_str(&format!(", Restricted: {}", yes(self.was_restricted)));
        }
        said
    }

    /// Restricts or frees the folder as ticked (asking for the password),
    /// then shows it as it really is. False if it didn't happen.
    fn change_restriction(&mut self) -> bool {
        self.restrict_note = ferro_restrict::set(&self.path, self.restricted).err();
        self.was_restricted = ferro_restrict::is_restricted(&self.path);
        self.restricted = self.was_restricted;
        self.locked = ferro_restrict::is_locked(&self.path);
        self.restrict_note.is_none()
    }

    /// The changes made on disk. False if restricting or freeing the folder
    /// didn't happen (the window then stays, saying why).
    fn apply_to_disk(&mut self, new_name: &str) -> bool {
        let mut done = true;
        // Freed first: Read-only below needs the folder to be yours again.
        if self.is_dir && self.was_restricted && !self.restricted {
            done &= self.change_restriction();
        }
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

        // Apply read-only permission change (root's folder, while restricted: not ours to change)
        if let (false, Ok(meta)) = (self.was_restricted, fs::metadata(&self.path)) {
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

        // Restricted last: what it was, read-only and all, is what unticking gives back.
        if self.is_dir && !self.was_restricted && self.restricted {
            done &= self.change_restriction();
        }

        self.applied = true;
        done
    }
}

/// The desktop and File Explorer shown again, for what changed.
fn tell_desktop(conn: &RustConnection, root: Window, ferro_refresh: Atom) {
    let msg = ClientMessageEvent::new(32, root, ferro_refresh, [0, 0, 0, 0, 0]);
    let _ = conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, msg);
    let _ = conn.flush();
}

/// Which boxes were clicked on a sheet for several items: only those are
/// changed on each, so one left as it was (ticked for some of them, not
/// all) leaves every item as it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Touched {
    read_only: bool,
    hidden: bool,
    restricted: bool,
}

/// The several items' sheet `shown` applied to each of them: the boxes in
/// `touched` set as ticked there, each keeping its own name. The sheet is
/// then made again from them as they now are. False if restricting or
/// freeing any folder didn't happen.
fn apply_group(items: &mut [ItemProps], shown: &mut ItemProps, touched: Touched) -> bool {
    let mut done = true;
    for item in items.iter_mut() {
        if touched.read_only {
            item.read_only = shown.read_only;
        }
        if touched.hidden {
            item.hidden = shown.hidden;
        }
        if touched.restricted && item.is_dir {
            item.restricted = shown.restricted;
        }
        let name = item.name.clone();
        done &= item.apply_to_disk(&name);
    }
    *shown = ItemProps::group(items);
    done
}

struct OpenWithDialog {
    ext: String,
    cmd_text: String,
    cmd_cursor: usize,
    /// For this one file, not every file of its type.
    only_this: bool,
    /// The programs installed that say they open it: name and command.
    programs: Vec<(String, String)>,
}

/// What the Open With box chose: for this file alone, or for its type (and
/// this file then back to its type's). The program, as Properties shows it.
fn apply_open_with(path: &std::path::Path, dlg: &OpenWithDialog) -> Option<String> {
    let new_cmd = dlg.cmd_text.trim();
    if new_cmd.is_empty() {
        return None;
    }
    let cmd = if ferro_associations::has_file_code(new_cmd) { new_cmd.to_string() } else { format!("{new_cmd} %f") };
    if dlg.only_this || dlg.ext.is_empty() {
        ferro_associations::set_file_override(path, Some(&cmd));
    } else {
        let mut entries = ferro_associations::load_associations();
        if let Some(existing) = entries.iter_mut().find(|e| e.ext.eq_ignore_ascii_case(&dlg.ext)) {
            existing.command = cmd.clone();
        } else {
            let desc = format!("{} File", dlg.ext.to_uppercase());
            entries.push(ferro_associations::AssociationEntry { ext: dlg.ext.clone(), desc, command: cmd.clone() });
        }
        ferro_associations::save_associations(&entries);
        ferro_associations::set_file_override(path, None);
    }
    Some(shown_command(&cmd))
}

/// Open With's choice for the one file, or for each of several (their
/// type's once, if they're all of the one type and it's for the type).
fn open_with_all(path: &Path, group: &mut [ItemProps], dlg: &OpenWithDialog) -> Option<String> {
    if group.is_empty() {
        return apply_open_with(path, dlg);
    }
    let mut shown = None;
    for item in group.iter_mut() {
        shown = apply_open_with(&item.path, dlg);
        if let Some(s) = &shown {
            item.opens_with = s.clone();
        }
    }
    shown
}

/// A chosen command as Properties shows it: without the `%f` most end
/// with, but one that says where the file goes (`tool %f -y`) as it is, so
/// Change... gives it back as it was typed.
fn shown_command(cmd: &str) -> String {
    let cmd = cmd.trim();
    let bare = cmd.strip_suffix("%f").map(str::trim_end).filter(|b| !ferro_associations::has_file_code(b));
    bare.unwrap_or(cmd).to_string()
}

const DLG_RECT: Rect = Rect::new(25, 75, W - 50, 240);
const DLG_CMD_INPUT: Rect = Rect::new(40, 130, W - 80, 22);
const DLG_OK: Rect = Rect::new(W - 190, 280, 68, 24);
const DLG_CANCEL: Rect = Rect::new(W - 110, 280, 68, 24);
const DLG_MORE: Rect = Rect::new(40, 280, 68, 24);
const DLG_ONLY: Rect = Rect::new(40, 240, 13, 13);
/// The box and its words: either is a click on it.
const DLG_ONLY_HIT: Rect = Rect::new(36, 236, W - 90, 21);

/// A button for each program that opens the file, in two rows; what
/// doesn't fit is in More... (File Associations).
fn program_buttons(programs: &[(String, String)]) -> Vec<(Rect, String)> {
    let (left, right) = (40, W - 40);
    let (mut x, mut y) = (left, 165);
    let mut out = Vec::new();
    for (name, _) in programs {
        let label = ferro_gfx::ellipsize(name, 16);
        let w = (text_width(&label) + 16).max(55);
        if x + w > right {
            if y == 195 {
                break;
            }
            (x, y) = (left, 195);
        }
        out.push((Rect::new(x, y, w, 22), label));
        x += w + 5;
    }
    out
}

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
    s.bevel(FRAME_RECT, Bevel::Raised);
    draw_tabs(s, false, !props.is_dir && props.count == 1);

    // Top icon and name
    let icon = if props.was_restricted || props.locked {
        icons::FOLDER_LOCKED
    } else if props.is_dir {
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

    if !props.any_dir {
        button(s, CHANGE_BTN, "Change...", true);
    }

    draw_separator(s, 136);

    // Location
    s.text(26, 148, "Location:", color::SHADOW);
    let parent = props.location();
    let loc = ferro_gfx::ellipsize(&parent, ((W - 140) / GLYPH_W).max(1) as usize);
    render_val(s, RowId::Location, 148, &loc);

    // Size
    s.text(26, 170, "Size:", color::SHADOW);
    render_val(s, RowId::Size, 170, &human_size(props.size_bytes));

    // Size on disk
    s.text(26, 192, "Size on disk:", color::SHADOW);
    render_val(s, RowId::SizeOnDisk, 192, &human_size(props.size_blocks.max(props.size_bytes)));

    // If folder, Contains
    if props.is_dir || props.count > 1 {
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

    // Restricted checkbox (folders): nothing running as you opens it without your password
    if props.is_dir {
        s.bevel(CHECK_RES, Bevel::Sunken);
        s.fill(CHECK_RES.inset(1), color::WHITE);
        if props.restricted {
            s.text(CHECK_RES.x + 3, CHECK_RES.y + 1, "v", color::BLACK);
        }
        s.text(CHECK_RES.right() + 6, CHECK_RES.y + 1, "Restricted", color::BLACK);
        let note = match &props.restrict_note {
            Some(why) => why.clone(),
            None if props.restricted != props.was_restricted => "Apply asks for your password.".to_string(),
            None if props.was_restricted => "Only root can open it: untick to open it again.".to_string(),
            None => String::new(),
        };
        let fg = if props.restrict_note.is_some() { color::BLACK } else { color::SHADOW };
        for (k, line) in ferro_gfx::wrap(&note, ((W - 48) / GLYPH_W) as usize).iter().take(2).enumerate() {
            s.text(26, RES_NOTE_Y + k as i32 * (GLYPH_H + 2), line, fg);
        }
    }

    // Bottom buttons
    let copy_label = if copied_time.is_some_and(|t| t.elapsed().as_millis() < 1500) { "Copied!" } else { "Copy" };
    button(s, COPY_BTN, copy_label, true);
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", !props.applied);

    // Modal "Open With" Dialog
    if let Some(dlg) = dialog {
        s.fill(DLG_RECT, color::FACE);
        s.bevel(DLG_RECT, Bevel::Raised);

        let title_bar = Rect::new(DLG_RECT.x + 3, DLG_RECT.y + 3, DLG_RECT.w - 6, 18);
        s.fill(title_bar, color::NAVY);
        s.text(title_bar.x + 6, title_bar.y + 3, "Open With", color::WHITE);

        let what = if dlg.only_this || dlg.ext.is_empty() { "this file only".to_string() } else { format!("all .{} files", dlg.ext) };
        s.text(40, 100, &format!("Select application for {what}:"), color::BLACK);
        text_field_with_cursor(s, DLG_CMD_INPUT, &dlg.cmd_text, dlg.cmd_cursor, None, true);

        if dlg.programs.is_empty() {
            s.text(40, 150, "No program here says it opens this:", color::SHADOW);
            s.text(40, 165, "type the one to use above.", color::SHADOW);
        } else {
            s.text(40, 150, "Programs that open it:", color::SHADOW);
        }
        for (r, label) in program_buttons(&dlg.programs) {
            button(s, r, &label, true);
        }

        s.bevel(DLG_ONLY, Bevel::Sunken);
        s.fill(DLG_ONLY.inset(1), color::WHITE);
        if dlg.only_this || dlg.ext.is_empty() {
            s.text(DLG_ONLY.x + 3, DLG_ONLY.y + 1, "v", color::BLACK);
        }
        let only_label = if dlg.ext.is_empty() { "Only this file".to_string() } else { format!("Only this file, not every .{} file", dlg.ext) };
        s.text(DLG_ONLY.right() + 6, DLG_ONLY.y + 2, &only_label, color::BLACK);

        button(s, DLG_MORE, "More...", true);
        default_button(s, DLG_OK, "OK", true);
        button(s, DLG_CANCEL, "Cancel", true);
    }
}

fn draw_drive_chart(s: &mut Surface, drive: &DriveProps) {
    let (cx, cy) = (W / 2, 275);
    let rx = 72;
    let ry = 38;
    let depth = 14;

    let total = drive.total_bytes;
    let used = drive.used_bytes;
    let frac = if total > 0 { (used as f64 / total as f64).clamp(0.0, 1.0) } else { 0.0 };
    let split_angle = frac * 2.0 * std::f64::consts::PI;

    // Draw 3D side extrusion from y = cy to cy + depth
    for d in (1..=depth).rev() {
        let side_y = cy + d;
        for dx in -rx..=rx {
            let x = cx + dx;
            let max_dy = ((ry * ry) as f64 * (1.0 - (dx * dx) as f64 / (rx * rx) as f64)).max(0.0).sqrt() as i32;
            let dy = max_dy; // bottom rim
            let y = side_y + dy;
            let angle = {
                let mut a = (dy as f64).atan2(dx as f64);
                if a < 0.0 {
                    a += 2.0 * std::f64::consts::PI;
                }
                a
            };
            let c = if angle <= split_angle { 0x000045 } else { 0x00800080 };
            s.put(x, y, c);
            s.put(x, y - 1, c);
        }
    }

    // Draw top ellipse
    for dy in -ry..=ry {
        let y = cy + dy;
        let max_dx = ((rx * rx) as f64 * (1.0 - (dy * dy) as f64 / (ry * ry) as f64)).max(0.0).sqrt() as i32;
        for dx in -max_dx..=max_dx {
            let x = cx + dx;
            let mut angle = (dy as f64).atan2(dx as f64);
            if angle < 0.0 {
                angle += 2.0 * std::f64::consts::PI;
            }
            let is_edge = dx.abs() >= max_dx - 1 || dy.abs() >= ry - 1;
            let c = if is_edge {
                0x00404040
            } else if angle <= split_angle {
                0x000080
            } else {
                0x00C000C0
            };
            s.put(x, y, c);
        }
    }

    let drive_label = format!("Drive {}:", drive.letter);
    let tw = text_width(&drive_label);
    s.text((W - tw) / 2, cy + depth + ry + 12, &drive_label, color::BLACK);
}

fn draw_drive(s: &mut Surface, drive: &DriveProps, label: &str, cursor: usize, sel: Option<(usize, usize)>, focused: bool) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);

    // Tab Header
    s.bevel(TAB_RECT, Bevel::Raised);
    s.text(TAB_RECT.x + 16, TAB_RECT.y + 4, "General", color::BLACK);

    // Frame
    s.bevel(FRAME_RECT, Bevel::Raised);

    // Icon
    s.sprite(26, 42, icons::COMPUTER, 2);

    // Label field
    text_field_with_cursor(s, NAME_BOX, label, cursor, sel, focused);

    draw_separator(s, 82);

    // Type
    s.text(26, 94, "Type:", color::SHADOW);
    s.text(105, 94, "Local Disk", color::BLACK);

    // File System
    s.text(26, 114, "File system:", color::SHADOW);
    s.text(105, 114, &drive.fs_type, color::BLACK);

    draw_separator(s, 136);

    // Used Space
    s.fill(Rect::new(26, 148, 12, 12), 0x000080);
    s.bevel(Rect::new(26, 148, 12, 12), Bevel::Sunken);
    s.text(44, 150, "Used space:", color::BLACK);
    let used_bytes_str = format!("{} bytes", comma_number(drive.used_bytes));
    let used_human = human_size(drive.used_bytes);
    s.text(140, 150, &used_bytes_str, color::BLACK);
    s.text(W - 85, 150, &used_human, color::BLACK);

    // Free Space
    s.fill(Rect::new(26, 172, 12, 12), 0x00C000C0);
    s.bevel(Rect::new(26, 172, 12, 12), Bevel::Sunken);
    s.text(44, 174, "Free space:", color::BLACK);
    let free_bytes_str = format!("{} bytes", comma_number(drive.free_bytes));
    let free_human = human_size(drive.free_bytes);
    s.text(140, 174, &free_bytes_str, color::BLACK);
    s.text(W - 85, 174, &free_human, color::BLACK);

    draw_separator(s, 196);

    // Capacity
    s.text(44, 204, "Capacity:", color::BLACK);
    let cap_bytes_str = format!("{} bytes", comma_number(drive.total_bytes));
    let cap_human = human_size(drive.total_bytes);
    s.text(140, 204, &cap_bytes_str, color::BLACK);
    s.text(W - 85, 204, &cap_human, color::BLACK);

    // Pie chart
    draw_drive_chart(s, drive);

    // Buttons
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", false);
}

fn main() -> Res<()> {
    let result = run();
    // The Info tab's Open as Text file, if the editor hasn't had it yet:
    // not left behind.
    ferro_info::text::finish(ferro_info::text::EXIT_WAIT);
    result
}

fn run() -> Res<()> {
    let mut paths: Vec<PathBuf> = env::args_os().skip(1).filter(|a| a != "--").map(PathBuf::from).collect();
    paths.dedup();
    if paths.is_empty() {
        paths.push(env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    }
    let target_path = paths[0].clone();

    if paths.len() == 1 && is_drive(&target_path) {
        let drive = DriveProps::load(&target_path);
        let mut label_text = drive.label.clone();
        let mut label_cursor = label_text.len();
        let mut label_sel = Some((0, label_text.len()));
        let mut label_focused = true;

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

        let title = format!("{} ({}:) Properties", drive.label, drive.letter);
        let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
        let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);

        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
        conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;

        let keymap = Keymap::load(&conn)?;
        ferro_ui::fixed_size(&conn, win, W, H)?;
        conn.map_window(win)?;
        conn.flush()?;

        let repaint_drive = |label: &str, cursor: usize, sel: Option<(usize, usize)>, focused: bool| -> Res<()> {
            let mut s = Surface::new(W as usize, H as usize);
            draw_drive(&mut s, &drive, label, cursor, sel, focused);
            ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
        };

        repaint_drive(&label_text, label_cursor, label_sel, label_focused)?;

        loop {
            let event = conn.wait_for_event()?;
            match event {
                Event::Expose(e) if e.count == 0 => {
                    repaint_drive(&label_text, label_cursor, label_sel, label_focused)?;
                }
                Event::ButtonPress(e) => {
                    let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                    if OK_BTN.contains(x, y) || CANCEL_BTN.contains(x, y) {
                        return Ok(());
                    }
                    if NAME_BOX.contains(x, y) {
                        label_focused = true;
                        label_cursor = label_text.len();
                        label_sel = None;
                        repaint_drive(&label_text, label_cursor, label_sel, label_focused)?;
                    }
                }
                Event::KeyPress(e) => {
                    let key = keymap.typed(&e);
                    match key {
                        _ if dialog_answer(key).is_some() => return Ok(()),
                        Key::Backspace => {
                            if let Some((s, e)) = label_sel.take() {
                                let (min_s, max_e) = (s.min(e), s.max(e));
                                label_text.drain(min_s..max_e.min(label_text.len()));
                                label_cursor = min_s;
                            } else if label_cursor > 0 {
                                label_cursor = label_cursor.saturating_sub(1);
                                if label_cursor < label_text.len() {
                                    label_text.remove(label_cursor);
                                }
                            }
                            repaint_drive(&label_text, label_cursor, label_sel, label_focused)?;
                        }
                        Key::Char(c) if !c.is_control() => {
                            if let Some((s, e)) = label_sel.take() {
                                let (min_s, max_e) = (s.min(e), s.max(e));
                                label_text.drain(min_s..max_e.min(label_text.len()));
                                label_cursor = min_s;
                            }
                            if label_text.len() < 32 {
                                label_text.insert(label_cursor, c);
                                label_cursor += 1;
                            }
                            repaint_drive(&label_text, label_cursor, label_sel, label_focused)?;
                        }
                        _ => {}
                    }
                }
                Event::ClientMessage(e) if e.data.as_data32()[0] == wm_delete => {
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    // Several: each item as it is, and the sheet for them together, whose
    // name box only says what they are.
    let mut group: Vec<ItemProps> = if paths.len() > 1 { paths.iter().map(|p| ItemProps::load(p)).collect() } else { Vec::new() };
    let mut touched = Touched::default();
    let mut props = if group.is_empty() { ItemProps::load(&target_path) } else { ItemProps::group(&group) };
    let mut name_text = props.name.clone();

    // Select the file stem initially like Windows 11
    let stem_len = match Path::new(&name_text).file_stem().and_then(|s| s.to_str()) {
        Some(s) if !props.is_dir => s.len(),
        _ => name_text.len(),
    };
    let mut name_cursor = stem_len;
    let mut name_sel: Option<(usize, usize)> = if stem_len > 0 { Some((0, stem_len)) } else { None };

    let mut selected_row: Option<RowId> = if group.is_empty() { Some(RowId::Name) } else { None };
    if !group.is_empty() {
        name_sel = None;
    }
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

    ferro_ui::fixed_size(&conn, win, W, H)?;
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

    // The Info tab: made the first time it's opened (reading takes a moment).
    let mut info: Option<ferro_info::Panel> = None;
    let mut info_front = false;
    let mut last_click = 0u32;
    let repaint_info = |panel: &ferro_info::Panel, applied: bool| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw_info(&mut s, panel, applied);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };

    // OK, Enter and Apply: one item's changes, or the several items' to each.
    let apply = |props: &mut ItemProps, group: &mut Vec<ItemProps>, touched: &mut Touched, name: &str| -> bool {
        let done = if group.is_empty() {
            props.apply_to_disk(name)
        } else {
            let done = apply_group(group, props, *touched);
            *touched = Touched::default();
            done
        };
        tell_desktop(&conn, screen.root, ferro_refresh);
        done
    };

    loop {
        let event = conn.wait_for_event()?;
        // The Info tab in front: its events are the panel's, but for OK,
        // Cancel and Apply, which work as on General.
        if let (true, Some(panel)) = (info_front, info.as_mut()) {
            let act = |a: ferro_info::Action, panel: &ferro_info::Panel| -> Res<()> {
                match a {
                    ferro_info::Action::None => Ok(()),
                    ferro_info::Action::Redraw => repaint_info(panel, props.applied),
                }
            };
            match &event {
                Event::Expose(e) if e.count == 0 => {
                    repaint_info(panel, props.applied)?;
                    continue;
                }
                Event::ButtonPress(e) => {
                    let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                    if e.detail == 1 && TAB_RECT.contains(x, y) {
                        info_front = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog.as_ref())?;
                        continue;
                    }
                    if FRAME_RECT.contains(x, y) {
                        let a = match e.detail {
                            4 | 5 => panel.wheel(INFO_AREA, e.detail == 4),
                            1 => {
                                let double = e.time.wrapping_sub(last_click) < 400;
                                last_click = e.time;
                                panel.click(INFO_AREA, x, y, double)
                            }
                            _ => ferro_info::Action::None,
                        };
                        act(a, panel)?;
                        continue;
                    }
                    // OK, Cancel, Apply, Copy: as on General, which comes back to the front.
                    info_front = false;
                }
                Event::KeyPress(k) => {
                    let key = keymap.typed(k);
                    let (taken, a) = panel.key(INFO_AREA, key);
                    if taken {
                        act(a, panel)?;
                        continue;
                    }
                    // Enter (OK) and Esc (Cancel) as on General; other keys
                    // are General's (the name box), not for here.
                    if dialog_answer(key).is_none() {
                        continue;
                    }
                    info_front = false;
                }
                _ => {}
            }
        }
        // On General, a click on the Info tab brings it to the front.
        if let Event::ButtonPress(e) = &event {
            if e.detail == 1 && dialog.is_none() && !props.is_dir && group.is_empty() && INFO_TAB.contains(i32::from(e.event_x), i32::from(e.event_y))
            {
                let panel = info.get_or_insert_with(|| ferro_info::Panel::new(&props.path));
                info_front = true;
                repaint_info(panel, props.applied)?;
                continue;
            }
        }
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
                        if DLG_ONLY_HIT.contains(x, y) && !dlg.ext.is_empty() {
                            dlg.only_this = !dlg.only_this;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, Some(dlg))?;
                            continue;
                        }
                        if DLG_OK.contains(x, y) {
                            if let Some(shown) = open_with_all(&props.path, &mut group, dlg) {
                                props.opens_with = shown;
                                props.applied = false;
                            }
                            dialog = None;
                            repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                            continue;
                        }
                        let picked = program_buttons(&dlg.programs).iter().position(|(r, _)| r.contains(x, y));
                        if let Some((_, cmd)) = picked.and_then(|i| dlg.programs.get(i)) {
                            dlg.cmd_text = cmd.clone();
                            dlg.cmd_cursor = dlg.cmd_text.len();
                        }
                        if DLG_CMD_INPUT.contains(x, y) {
                            dlg.cmd_cursor = dlg.cmd_text.len();
                        }
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, Some(dlg))?;
                        continue;
                    }

                    // Main Window Click Handling
                    if OK_BTN.contains(x, y) {
                        if apply(&mut props, &mut group, &mut touched, &name_text) {
                            return Ok(());
                        }
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if CANCEL_BTN.contains(x, y) {
                        return Ok(());
                    }
                    if APPLY_BTN.contains(x, y) {
                        let _ = apply(&mut props, &mut group, &mut touched, &name_text);
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
                    if !props.any_dir && CHANGE_BTN.contains(x, y) {
                        let ext_of = |p: &Path| p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let mut ext = ext_of(&props.path);
                        // Files of several types: each file is given the program, not a type.
                        if group.iter().any(|i| ext_of(&i.path) != ext) {
                            ext.clear();
                        }
                        let cur_cmd = if ferro_associations::has_file_code(&props.opens_with) {
                            props.opens_with.clone()
                        } else {
                            format!("{} %f", props.opens_with)
                        };
                        let only_this = ext.is_empty() || ferro_associations::file_override(&props.path).is_some();
                        let programs = ferro_associations::programs_for_path(&props.path);
                        dialog = Some(OpenWithDialog { ext, cmd_cursor: cur_cmd.len(), cmd_text: cur_cmd, only_this, programs });
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, dialog.as_ref())?;
                        continue;
                    }
                    if CHECK_RO_LABEL.contains(x, y) {
                        props.read_only = !props.read_only;
                        touched.read_only = true;
                        props.applied = false;
                        selected_row = Some(RowId::Attributes);
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if props.is_dir && CHECK_RES_LABEL.contains(x, y) {
                        props.restricted = !props.restricted;
                        touched.restricted = true;
                        props.restrict_note = None;
                        props.applied = false;
                        selected_row = Some(RowId::Attributes);
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }
                    if CHECK_HID_LABEL.contains(x, y) {
                        props.hidden = !props.hidden;
                        touched.hidden = true;
                        props.applied = false;
                        selected_row = Some(RowId::Attributes);
                        select_all_details = false;
                        repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                        continue;
                    }

                    // Name Box Focus / Click (several items: nothing to rename)
                    if NAME_BOX.contains(x, y) && group.is_empty() {
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
                        _ if dialog_answer(key).is_some() => {
                            if dialog_answer(key) == Some(true) {
                                if let Some(shown) = open_with_all(&props.path, &mut group, dlg) {
                                    props.opens_with = shown;
                                    props.applied = false;
                                }
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

                if dialog_answer(key) == Some(true) {
                    if apply(&mut props, &mut group, &mut touched, &name_text) {
                        return Ok(());
                    }
                    repaint(&props, &name_text, name_cursor, name_sel, selected_row, select_all_details, copied_time, None)?;
                    continue;
                }

                if dialog_answer(key) == Some(false) {
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

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferro_prop_group_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn writable(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).unwrap().permissions().mode() & 0o200 != 0
    }

    #[test]
    fn several_items_are_shown_together() {
        let dir = scratch("shown");
        fs::write(dir.join("a.txt"), "12345").unwrap();
        fs::write(dir.join("b.txt"), "123").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/c.png"), "1").unwrap();
        let items: Vec<ItemProps> = ["a.txt", "b.txt", "sub"].iter().map(|n| ItemProps::load(&dir.join(n))).collect();
        let g = ItemProps::group(&items);
        assert_eq!(g.name, "2 Files, 1 Folder");
        assert_eq!(g.count, 3);
        assert_eq!(g.type_str, "Various types");
        assert_eq!(g.size_bytes, 9);
        assert_eq!(g.row_value(RowId::Contains), "3 Files, 1 Folders");
        assert_eq!(g.location(), dir.display().to_string());
        assert!(g.any_dir && !g.is_dir, "no Change..., no Restricted");
        let txt = ItemProps::group(&items[..2]);
        assert_eq!((txt.name.as_str(), txt.type_str.as_str()), ("2 Files", "Text Document"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_change_goes_to_every_item() {
        let dir = scratch("apply");
        for n in ["a.txt", "b.txt", "c.txt"] {
            fs::write(dir.join(n), n).unwrap();
        }
        let mut items: Vec<ItemProps> = ["a.txt", "b.txt", "c.txt"].iter().map(|n| ItemProps::load(&dir.join(n))).collect();
        let mut shown = ItemProps::group(&items);
        assert!(!shown.read_only && !shown.hidden);

        shown.read_only = true;
        assert!(apply_group(&mut items, &mut shown, Touched { read_only: true, ..Touched::default() }));
        for n in ["a.txt", "b.txt", "c.txt"] {
            assert!(!writable(&dir.join(n)), "{n} read-only");
        }
        assert!(shown.read_only);

        shown.hidden = true;
        assert!(apply_group(&mut items, &mut shown, Touched { hidden: true, ..Touched::default() }));
        for n in [".a.txt", ".b.txt", ".c.txt"] {
            assert!(dir.join(n).exists(), "{n} hidden, its name kept");
            assert!(!writable(&dir.join(n)), "{n} still read-only: not clicked this time");
        }
        assert!(shown.hidden && shown.count == 3);

        shown.read_only = false;
        shown.hidden = false;
        assert!(apply_group(&mut items, &mut shown, Touched { read_only: true, hidden: true, ..Touched::default() }));
        for n in ["a.txt", "b.txt", "c.txt"] {
            assert!(dir.join(n).exists() && writable(&dir.join(n)), "{n} back as it was");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_box_not_clicked_leaves_each_as_it_was() {
        let dir = scratch("mixed");
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        let mut ro = ItemProps::load(&dir.join("a.txt"));
        ro.read_only = true;
        assert!(ro.apply_to_disk("a.txt"));
        let mut items = vec![ItemProps::load(&dir.join("a.txt")), ItemProps::load(&dir.join("b.txt"))];
        let mut shown = ItemProps::group(&items);
        assert!(!shown.read_only, "ticked only if all are");
        shown.hidden = true;
        assert!(apply_group(&mut items, &mut shown, Touched { hidden: true, ..Touched::default() }));
        assert!(!writable(&dir.join(".a.txt")), "the read-only one stays read-only");
        assert!(writable(&dir.join(".b.txt")), "the other stays writable");
        let _ = fs::remove_dir_all(&dir);
    }
}
