//! ferro-files: FerroOS Heavy's file manager. A folder as Windows 95 showed
//! it (the `view` module draws it with ferro-gfx), with paths the Windows
//! way: `C:\Users\you\Documents`, whatever Linux calls the place.
//!
//! Click to select, double-click (or Enter) to open, Up (or Backspace) for
//! the folder above, the wheel to scroll. Opening a folder goes into it;
//! opening a program starts it; an archive is unpacked beside itself; any
//! other file opens in its program. Right-click, or the keys Windows has
//! (Ctrl+C, Ctrl+X, Ctrl+V, Delete, F2, F5, the arrows), copy, move,
//! rename and delete; the slow ones run aside, so the window never hangs.
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod drives;
mod ops;
mod view;

use ferro_gfx::{Rect, Surface};
use ferro_path::DriveTable;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use view::{ContextMenu, Entry, Hit, MenuAction, Prompt};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Two clicks this close together (ms) are a double-click.
const DOUBLE_CLICK_MS: u32 = 650;
/// Key codes on the standard PC keyboard map every X server here uses.
const KEY_BACKSPACE: u8 = 22;
const KEY_RETURN: u8 = 36;
const KEY_KP_ENTER: u8 = 104;
const KEY_ESCAPE: u8 = 9;
const KEY_DELETE: u8 = 119;
const KEY_F2: u8 = 68;
const KEY_F5: u8 = 71;
const KEY_UP: u8 = 111;
const KEY_DOWN: u8 = 116;
const KEY_LEFT: u8 = 113;
const KEY_RIGHT: u8 = 114;
const KEY_C: u8 = 54;
const KEY_X: u8 = 53;
const KEY_V: u8 = 55;
const KEY_A: u8 = 38;
const KEY_L: u8 = 46;
const KEY_D: u8 = 40;

fn files_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("files.conf")
    } else {
        PathBuf::from("/etc/ferro/files.conf")
    }
}

fn load_setting(key: &str) -> Option<String> {
    let content = std::fs::read_to_string(files_config_path()).ok()?;
    content.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_owned())
}

fn load_show_hidden() -> bool {
    load_setting("show_hidden").is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

fn load_icon_size() -> usize {
    load_setting("icon_size").and_then(|v| v.parse().ok()).unwrap_or(1)
}

/// The order last chosen: a column and whether going up (Name, A to Z, if none).
fn load_sort() -> (view::Col, bool) {
    let col = load_setting("sort").and_then(|v| view::Col::from_key(&v)).unwrap_or(view::Col::Name);
    let up = load_setting("sort_up").map_or(col.starts_up(), |v| !(v.eq_ignore_ascii_case("false") || v == "0"));
    (col, up)
}

fn save_settings(show_hidden: bool, icon_size: usize, sort: (view::Col, bool)) {
    let path = files_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (col, up) = sort;
    let _ = std::fs::write(
        &path,
        format!("# FerroOS File Explorer Settings\nshow_hidden = {show_hidden}\nicon_size = {icon_size}\nsort = {}\nsort_up = {up}\n", col.key()),
    );
}

/// What's in `dir`: folders first, then by name; hidden files included if `show_hidden` is set.
fn list(dir: &Path, drives: &DriveTable, show_hidden: bool) -> Vec<Entry> {
    let dir_text = dir.to_string_lossy();
    let mut out: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let real = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && real.starts_with('.') {
                return None;
            }
            let is_dir = e.path().is_dir();
            let meta = e.metadata().ok();
            let size = if is_dir { 0 } else { meta.as_ref().map_or(0, |m| m.len()) };
            let modified =
                meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            // A restricted folder (Properties > Restricted) wears a padlock.
            let icon = (is_dir && ferro_restrict::is_restricted(&e.path())).then_some(ferro_gfx::icons::FOLDER_LOCKED);
            Some(Entry { name: drives.shown_name(&dir_text, &real).to_owned(), real, is_dir, icon, size, modified })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Starts a program, detached from the file manager.
fn launch(program: &str, args: &[&str]) {
    launch_in_dir(program, args, None);
}

fn clipboard_file() -> PathBuf {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime).join("ferro-files.clipboard")
    } else {
        std::env::temp_dir().join(format!("ferro-files-clipboard-{}", unsafe { libc::getuid() }))
    }
}

fn launch_in_dir(program: &str, args: &[&str], cwd: Option<&Path>) {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    match cmd.spawn() {
        // Waited for aside, so it isn't left a zombie when it ends.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("ferro-files: {program}: {e}"),
    }
}

/// What a prompt is asking about.
#[derive(Clone, Debug)]
enum Asking {
    Delete(PathBuf),
    DeleteMulti(Vec<PathBuf>),
    /// Delete, with some or all of these on a removable drive: those can't
    /// go to the Recycle Bin, so they're deleted for good (written over).
    OffDrive(Vec<PathBuf>),
    #[allow(dead_code)]
    Rename(PathBuf),
    /// Only telling: OK puts it away.
    Nothing,
}

fn is_program(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    // Shortcuts are run (or not) by what they say; and only programs put
    // here on purpose are started, never just anything with the
    // executable bit, as every file on a USB stick has. Nor is a picture or
    // a .exe that merely has the bit (files copied off NTFS or out of a zip
    // often do): only a Linux program or a script runs as itself.
    !ext.eq_ignore_ascii_case("desktop") && ferro_associations::trusted_launcher(path) && looks_runnable(path)
}

/// A Linux program (ELF) or a script (`#!`), by its first bytes.
fn looks_runnable(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].starts_with(b"\x7fELF") || head[..n].starts_with(b"#!")
}

fn find_ferro_dir(conn: &RustConnection, win: x11rb::protocol::xproto::Window, atom_dir: Atom) -> Option<(x11rb::protocol::xproto::Window, PathBuf)> {
    if let Ok(cookie) = conn.get_property(false, win, atom_dir, AtomEnum::ANY, 0, 1024) {
        if let Ok(reply) = cookie.reply() {
            if !reply.value.is_empty() {
                if let Ok(s) = std::str::from_utf8(&reply.value) {
                    if s != "My Computer" {
                        let p = PathBuf::from(s);
                        if p.is_dir() {
                            return Some((win, p));
                        }
                    }
                }
            }
        }
    }
    if let Ok(cookie) = conn.query_tree(win) {
        if let Ok(tree) = cookie.reply() {
            for child in tree.children {
                if let Some(res) = find_ferro_dir(conn, child, atom_dir) {
                    return Some(res);
                }
            }
        }
    }
    None
}

use ferro_associations::thumbs::{IMAGE_EXTS, VIDEO_EXTS};

/// The places down the left, as Windows' navigation pane has them (without
/// Quick Access): My Computer, the home folders that are there, the drives
/// plugged in, and the Recycle Bin.
fn places() -> Vec<view::Place> {
    use ferro_gfx::icons;
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut out = vec![view::Place { label: "My Computer".into(), path: PathBuf::from("My Computer"), icon: icons::COMPUTER }];
    for name in ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"] {
        let path = home.join(name);
        if path.is_dir() {
            out.push(view::Place { label: name.into(), path, icon: icons::FOLDER });
        }
    }
    let user = std::env::var("USER").unwrap_or_default();
    for media in [PathBuf::from("/media").join(&user), PathBuf::from("/run/media").join(&user)] {
        let mut drives: Vec<PathBuf> = std::fs::read_dir(&media).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        drives.sort();
        for path in drives {
            let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(view::Place { label, path, icon: icons::DRIVE });
        }
    }
    out.push(view::Place { label: "Recycle Bin".into(), path: home.join(".local/share/Trash"), icon: icons::RECYCLE });
    out
}

/// The desktop's Recycle Bin icon, as the window manager publishes it on
/// the root window: x, y, width, height.
/// The desktop folder icon at (x, y), as the desktop lists them on the
/// root (`_FERRO_DESKTOP_FOLDERS`: "x y w h path" a line).
fn desktop_folder_at(conn: &RustConnection, root: u32, atom: Atom, x: i32, y: i32) -> Option<PathBuf> {
    let reply = conn.get_property(false, root, atom, AtomEnum::ANY, 0, 1 << 16).ok()?.reply().ok()?;
    folder_at(&String::from_utf8_lossy(&reply.value), x, y)
}

fn folder_at(list: &str, x: i32, y: i32) -> Option<PathBuf> {
    list.lines().find_map(|line| {
        let mut f = line.splitn(5, ' ');
        let mut n = || f.next()?.parse::<i32>().ok();
        let (rx, ry, rw, rh) = (n()?, n()?, n()?, n()?);
        let path = PathBuf::from(f.next()?);
        ((rx..rx + rw).contains(&x) && (ry..ry + rh).contains(&y) && path.is_dir()).then_some(path)
    })
}

fn recycle_bin_rect(conn: &RustConnection, root: u32, atom: Atom) -> Option<(i32, i32, i32, i32)> {
    let reply = conn.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
    let v: Vec<u32> = reply.value32()?.collect();
    (v.len() == 4).then(|| (v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32))
}

struct Window {
    conn: RustConnection,
    win: u32,
    root: u32,
    gc: u32,
    depth: u8,
    net_wm_name: Atom,
    utf8_string: Atom,
    ferro_current_dir: Atom,
    ferro_refresh: Atom,
    /// Where the desktop's Recycle Bin icon is (`_FERRO_RECYCLE_BIN` on the root).
    ferro_recycle_bin: Atom,
    /// Where the desktop's folder icons are (`_FERRO_DESKTOP_FOLDERS` on the root).
    ferro_desktop_folders: Atom,
    /// Tells the desktop where files dropped on it go (where they were let go).
    ferro_place: Atom,
    screen: Screen,
    /// What's being dragged, drawn beside the pointer over everything.
    drag_icon: Option<ferro_ui::DragIcon>,
    w: i32,
    h: i32,
    drives: DriveTable,
    detected_drives: Vec<drives::DetectedDrive>,
    is_my_computer: bool,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    selected_set: std::collections::HashSet<usize>,
    rubberband: Option<(i32, i32, i32, i32)>,
    /// The scroll when the box was started (its start moves with the icons).
    rubber_scroll: usize,
    renaming: Option<(usize, ferro_ui::TextEdit)>,
    /// The places down the left: My Computer, the home folders, the drives
    /// plugged in, the Recycle Bin.
    places: Vec<view::Place>,
    /// The address being typed, after a click on it (or Ctrl+L).
    address: Option<ferro_ui::TextEdit>,
    /// The mouse held down in the name being typed: it selects as it moves.
    rename_drag: bool,
    /// A click on the one item already selected: if it's let go without a
    /// drag, and no second click follows (opening it), its name is typed.
    rename_armed: Option<usize>,
    /// What the entries are in order of (Name first), and whether going up.
    sort: (view::Col, bool),
    rename_soon: Option<(usize, std::time::Instant)>,
    show_hidden: bool,
    scroll: usize,
    /// The last click: when, and on which entry.
    last_click: (u32, Option<usize>),
    menu: Option<ContextMenu>,
    thumbnails: std::collections::HashMap<String, ferro_gfx::DecodedImage>,
    /// The folder `thumbnails` are for, what's been asked for there, and
    /// which round of asking is current (older rounds' work is dropped).
    thumbs_dir: PathBuf,
    thumbs_asked: std::collections::HashSet<String>,
    thumbs_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    thumbs_tx: Sender<(u64, String, ferro_gfx::DecodedImage)>,
    thumbs_rx: Receiver<(u64, String, ferro_gfx::DecodedImage)>,
    keymap: ferro_ui::Keymap,
    /// What Copy or Cut put by: the path, and whether it was cut.
    clipboard: Option<(PathBuf, bool)>,
    drag_start: Option<(usize, i32, i32)>,
    dragging: Option<usize>,
    drag_pos: Option<(i32, i32)>,
    drag_target: Option<usize>,
    drag_scroll: Option<(i32, usize)>,
    /// Something changed that the next quiet moment draws (see the loop).
    want_paint: bool,
    /// The last thing done, or what went wrong, for the status bar.
    status: String,
    prompt: Option<(Prompt, Asking)>,
    /// The work running aside reports here, then wakes the window by
    /// writing a byte to this pipe.
    wake: i32,
    report: Sender<Result<String, String>>,
    reports: Receiver<Result<String, String>>,
    busy: usize,
}

impl Window {
    fn get_clipboard(&self) -> Option<(Vec<PathBuf>, bool)> {
        if let Ok(content) = std::fs::read_to_string(clipboard_file()) {
            let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
            if let Some(first) = lines.next() {
                if let Some((action, path_str)) = first.split_once(':') {
                    let cut = action.trim() == "CUT";
                    let path = PathBuf::from(path_str.trim());
                    if path.exists() {
                        return Some((vec![path], cut));
                    }
                }
                let cut = first.eq_ignore_ascii_case("CUT");
                let mut paths = Vec::new();
                for line in lines {
                    let p = PathBuf::from(line);
                    if p.exists() {
                        paths.push(p);
                    }
                }
                if !paths.is_empty() {
                    return Some((paths, cut));
                }
            }
        }
        self.clipboard.as_ref().map(|(p, c)| (vec![p.clone()], *c))
    }

    fn set_clipboard(&mut self, paths: Vec<PathBuf>, cut: bool) {
        if let Some(first) = paths.first() {
            self.clipboard = Some((first.clone(), cut));
        }
        let mode = if cut { "CUT" } else { "COPY" };
        let mut content = format!("{mode}\n");
        for p in &paths {
            content.push_str(&p.to_string_lossy());
            content.push('\n');
        }
        let _ = std::fs::write(clipboard_file(), content);
    }

    fn clear_clipboard(&mut self) {
        self.clipboard = None;
        let _ = std::fs::remove_file(clipboard_file());
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        if !self.selected_set.is_empty() {
            let mut paths: Vec<PathBuf> = self.selected_set.iter().filter_map(|&i| self.entries.get(i).map(|e| self.dir.join(&e.real))).collect();
            paths.sort();
            paths
        } else if let Some(i) = self.selected {
            self.entries.get(i).map(|e| vec![self.dir.join(&e.real)]).unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// The folder as people read it.
    fn shown_path(&self) -> String {
        if self.is_my_computer {
            "My Computer".to_string()
        } else {
            self.drives.to_windows(&self.dir.to_string_lossy()).unwrap_or_else(|| self.dir.to_string_lossy().into_owned())
        }
    }

    fn go_computer(&mut self) -> Res<()> {
        view::set_icon_size(load_icon_size());
        let (drives, detected) = drives::system_drives_with_detected();
        self.drives = drives;
        self.detected_drives = detected.clone();
        self.is_my_computer = true;
        self.dir = PathBuf::from("My Computer");
        self.selected = None;
        self.selected_set.clear();
        self.scroll = 0;
        self.menu = None;
        self.prompt = None;
        // Back in a folder later, its thumbnails are made afresh.
        self.thumbs_dir = PathBuf::new();

        self.entries = detected
            .into_iter()
            .map(|d| Entry {
                name: format!("{} ({}:)", d.label, d.letter),
                real: d.path.to_string_lossy().into_owned(),
                is_dir: true,
                icon: Some(d.icon),
                size: 0,
                modified: 0,
            })
            .collect();

        let title = "My Computer";
        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.net_wm_name, self.utf8_string, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.ferro_current_dir, self.utf8_string, b"My Computer")?;
        self.paint()
    }

    fn go(&mut self, dir: PathBuf) -> Res<()> {
        // Drives come and go: the places as they are now.
        self.places = places();
        if dir == Path::new("My Computer") || dir.to_string_lossy() == "My Computer" {
            return self.go_computer();
        }
        self.is_my_computer = false;
        if let Err(e) = std::fs::read_dir(&dir) {
            if ferro_restrict::is_restricted(&dir) {
                let name = dir.file_name().map_or_else(|| dir.to_string_lossy(), |n| n.to_string_lossy());
                return self.tell(format!("'{name}' is restricted. Untick Restricted in its Properties to open it."));
            }
            return self.tell(ops::explain("open", &dir.to_string_lossy(), &e));
        }
        // What's selected is kept by name, not by place in the list: after a
        // refresh the same files are still the ones selected, wherever they
        // now are, and another folder starts with nothing selected (a drag
        // there never takes files picked somewhere else).
        let same_dir = dir == self.dir;
        let name_of = |i: usize| self.entries.get(i).map(|e| e.real.clone());
        let kept: Vec<_> = if same_dir { self.selected_set.iter().filter_map(|&i| name_of(i)).collect() } else { Vec::new() };
        let kept_one = if same_dir { self.selected.and_then(name_of) } else { None };
        if !same_dir {
            self.status.clear();
            self.scroll = 0;
        }
        self.entries = list(&dir, &self.drives, self.show_hidden);
        self.sort_entries();
        self.dir = dir;
        let index_of = |name: &_| self.entries.iter().position(|e| &e.real == name);
        self.selected_set = kept.iter().filter_map(index_of).collect();
        self.selected = kept_one.as_ref().and_then(index_of);
        self.scroll = self.scroll.min(view::max_scroll(self.w, self.h, self.entries.len()));
        self.menu = None;
        self.prompt = None;

        let title = self.shown_path();
        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.net_wm_name, self.utf8_string, title.as_bytes())?;
        let dir_str = self.dir.to_string_lossy();
        self.conn.change_property8(PropMode::REPLACE, self.win, self.ferro_current_dir, self.utf8_string, dir_str.as_bytes())?;
        self.paint()?;

        self.load_thumbnails();
        Ok(())
    }

    /// Starts making the thumbnails this folder still lacks: pictures and
    /// videos, every one of them, aside so the window never waits. They
    /// arrive through `thumbs_rx` as they're made. Kept while the folder
    /// stays the same, so a refresh doesn't make them all again.
    fn load_thumbnails(&mut self) {
        if self.is_my_computer {
            return;
        }
        if self.dir != self.thumbs_dir {
            self.thumbnails.clear();
            self.thumbs_asked.clear();
            self.thumbs_dir = self.dir.clone();
            // Whatever is still being made for the last folder stops.
            self.thumbs_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let jobs: Vec<(String, PathBuf, bool)> = self
            .entries
            .iter()
            .filter(|e| !e.is_dir && !self.thumbnails.contains_key(&e.real) && !self.thumbs_asked.contains(&e.real))
            .filter_map(|e| {
                let path = self.dir.join(&e.real);
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                if IMAGE_EXTS.contains(&ext.as_str()) {
                    Some((e.real.clone(), path, false))
                } else if VIDEO_EXTS.contains(&ext.as_str()) {
                    Some((e.real.clone(), path, true))
                } else {
                    None
                }
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.thumbs_asked.extend(jobs.iter().map(|(name, _, _)| name.clone()));
        let gen_now = self.thumbs_gen.load(std::sync::atomic::Ordering::Relaxed);
        let (current, tx, wake) = (self.thumbs_gen.clone(), self.thumbs_tx.clone(), self.wake);
        // As big as the icons are drawn.
        let size = view::icon_px() as u32;
        std::thread::spawn(move || {
            for (name, path, is_video) in jobs {
                if current.load(std::sync::atomic::Ordering::Relaxed) != gen_now {
                    return;
                }
                let thumb = if is_video {
                    ferro_associations::thumbs::video_frame(&path, size)
                } else {
                    std::fs::read(&path).ok().and_then(|b| ferro_gfx::DecodedImage::thumbnail_fit(&b, size))
                };
                let Some(thumb) = thumb else { continue };
                if tx.send((gen_now, name, thumb)).is_err() {
                    return;
                }
                #[cfg(unix)]
                // SAFETY: writes one byte from a live buffer to our own pipe.
                unsafe {
                    libc::write(wake, [1u8].as_ptr().cast(), 1);
                }
                #[cfg(not(unix))]
                let _ = wake;
            }
        });
    }

    /// Takes the thumbnails made since last time, for this folder.
    fn take_thumbnails(&mut self) {
        let now = self.thumbs_gen.load(std::sync::atomic::Ordering::Relaxed);
        while let Ok((gen, name, thumb)) = self.thumbs_rx.try_recv() {
            if gen == now {
                self.thumbnails.insert(name, thumb);
            }
        }
    }

    fn up(&mut self) -> Res<()> {
        if self.is_my_computer {
            return Ok(());
        }
        let is_root = self.dir == Path::new("/")
            || self.detected_drives.iter().any(|d| d.path == self.dir)
            || self.drives.to_windows(&self.dir.to_string_lossy()).is_some_and(|w| w.len() <= 3);

        if is_root {
            return self.go_computer();
        }

        match self.dir.parent().map(Path::to_path_buf) {
            Some(parent) => {
                if self.drives.to_windows(&self.dir.to_string_lossy()).is_some_and(|w| w.len() <= 3) {
                    self.go_computer()
                } else {
                    self.go(parent)
                }
            }
            None => self.go_computer(),
        }
    }

    fn open(&mut self, i: usize) -> Res<()> {
        let Some(e) = self.entries.get(i) else { return Ok(()) };
        if self.is_my_computer {
            #[allow(unused_mut)]
            let mut target = PathBuf::from(&e.real);
            #[cfg(unix)]
            if target.starts_with("/dev/") {
                if let Ok(output) = std::process::Command::new("udisksctl").args(["mount", "-b", &e.real, "--no-user-interaction"]).output() {
                    let out_str = String::from_utf8_lossy(&output.stdout);
                    if let Some(pos) = out_str.find(" at ") {
                        let mnt = out_str[pos + 4..].trim().trim_end_matches('.');
                        if !mnt.is_empty() {
                            target = PathBuf::from(mnt);
                        }
                    }
                }
            }
            return self.go(target);
        }
        let path = self.dir.join(&e.real);
        let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if e.is_dir || resolved.is_dir() {
            return self.go(resolved);
        }
        let target_path = if resolved.exists() { &resolved } else { &path };
        // A program chosen for this file alone (Properties) opens it, whatever it is.
        if ferro_associations::file_override(target_path).is_some() {
            let cmd = ferro_associations::resolve_command_for_path(target_path);
            launch_in_dir("sh", &["-c", &cmd], Some(&self.dir));
            return Ok(());
        }
        if ops::is_installable(&e.real) {
            // A program to install (a .deb, a .tar.gz): the installer asks.
            return self.install(&path);
        }
        if ops::is_archive(&e.real) {
            // Opened, an archive is unpacked beside itself (Windows opens a
            // zip as a folder; this ends the same: its files, in a folder).
            return self.extract(path);
        }
        let text = path.to_string_lossy().into_owned();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("appimage") {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mut perms = metadata.permissions();
                    let mode = perms.mode();
                    if mode & 0o111 == 0 {
                        perms.set_mode(mode | 0o755);
                        let _ = std::fs::set_permissions(&path, perms);
                    }
                }
            }
            launch_in_dir(&text, &[], Some(&self.dir));
            return Ok(());
        }
        if is_program(target_path) {
            launch_in_dir(&text, &[], Some(&self.dir));
        } else {
            let cmd = ferro_associations::resolve_command_for_path(target_path);
            launch_in_dir("sh", &["-c", &cmd], Some(&self.dir));
        }
        Ok(())
    }

    fn paint(&self) -> Res<()> {
        let (w, h) = (self.w, self.h);
        let mut s = Surface::new(w as usize, h as usize);
        let status = if self.busy > 0 && self.status.is_empty() { "Working..." } else { &self.status };
        // What's dragged is drawn beside the pointer by its own window (DragIcon).
        let dragging = None;
        let rb = self.rubber_rect();
        let ren = self.renaming.as_ref().map(|(i, e)| (*i, e));
        view::draw(
            &mut s,
            w,
            h,
            &self.shown_path(),
            &self.entries,
            self.selected,
            Some(&self.selected_set),
            self.scroll,
            self.menu.as_ref(),
            status,
            self.prompt.as_ref().map(|(p, _)| p),
            dragging,
            self.drag_target,
            rb,
            ren,
            Some(&self.thumbnails),
            self.sort,
        );
        let here = if self.is_my_computer { PathBuf::from("My Computer") } else { self.dir.clone() };
        view::draw_side(&mut s, w, h, &self.places, &here, None);
        if let Some(edit) = &self.address {
            edit.draw(&mut s, view::address_rect(w).inset(2));
        }
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, w, h)
    }

    /// New, View or Sort by, beside item `idx` of `menu`.
    fn open_sub(&self, menu: &mut ContextMenu, idx: usize, kind: u8) {
        let (items, checked) = if kind == view::SUB_NEW {
            (vec![("Folder", MenuAction::NewFolder), ("Text Document", MenuAction::NewDocument)], Vec::new())
        } else if kind == view::SUB_SORT {
            // The columns, then which way: the one of each in use checked.
            let mut items: Vec<(&'static str, MenuAction)> = view::SORT_COLS.iter().map(|&(c, name)| (name, MenuAction::SortBy(c))).collect();
            let mut checked: Vec<usize> = view::SORT_COLS.iter().position(|&(c, _)| c == self.sort.0).into_iter().collect();
            items.extend([("Ascending", MenuAction::SortUp(true)), ("Descending", MenuAction::SortUp(false))]);
            checked.push(if self.sort.1 { items.len() - 2 } else { items.len() - 1 });
            (items, checked)
        } else {
            // Details first, then the icon sizes smallest up; then hidden files.
            let order = [view::DETAILS, 0, 1, 2, 3];
            let mut items: Vec<(&'static str, MenuAction)> = order.iter().map(|&i| (view::SIZES[i].0, MenuAction::IconSize(i))).collect();
            let mut checked: Vec<usize> = order.iter().position(|&i| i == view::icon_size()).into_iter().collect();
            items.push(("Show hidden files", MenuAction::ToggleHidden));
            if self.show_hidden {
                checked.push(items.len() - 1);
            }
            (items, checked)
        };
        view::open_sub(menu, idx, kind, items, checked, self.w, self.h);
    }

    /// A new order: the entries put in it, the selection let go (its
    /// numbers are for the old order) and the choice kept for next time.
    fn set_sort(&mut self, sort: (view::Col, bool)) {
        self.sort = sort;
        self.sort_entries();
        self.selected = None;
        self.selected_set.clear();
        save_settings(self.show_hidden, view::icon_size(), sort);
    }

    /// The entries in the order chosen (a column heading clicked): folders
    /// first, as Windows keeps them, then by that column.
    fn sort_entries(&mut self) {
        let (col, up) = self.sort;
        let lower = |e: &Entry| e.name.to_lowercase();
        self.entries.sort_by(|a, b| {
            let by = match col {
                view::Col::Name => lower(a).cmp(&lower(b)),
                view::Col::Modified => a.modified.cmp(&b.modified),
                view::Col::Type => view::kind_name(a).cmp(&view::kind_name(b)).then_with(|| lower(a).cmp(&lower(b))),
                view::Col::Size => a.size.cmp(&b.size),
            };
            b.is_dir.cmp(&a.is_dir).then(if up { by } else { by.reverse() })
        });
    }

    /// The address typed: to that folder (C:\ and ~ as well as /).
    fn go_typed(&mut self, typed: &str) -> Res<()> {
        let typed = typed.trim();
        if typed.eq_ignore_ascii_case("My Computer") || typed.is_empty() {
            return self.go_computer();
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let posix = if ferro_path::is_windows_path(typed) {
            self.drives.to_posix(typed).unwrap_or_else(|_| typed.to_owned())
        } else if let Some(rest) = typed.strip_prefix('~') {
            format!("{home}{rest}")
        } else {
            typed.to_owned()
        };
        let path = PathBuf::from(posix);
        // A file: its folder, with it selected.
        if path.is_file() {
            if let Some(dir) = path.parent() {
                self.go(dir.to_path_buf())?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(i) = self.entries.iter().position(|e| e.real == name) {
                    self.selected = Some(i);
                    self.selected_set = std::iter::once(i).collect();
                }
                return self.paint();
            }
        }
        self.go(path)
    }

    /// A click on the address, or on a place down the left: true if it was.
    fn bar_click(&mut self, e: &ButtonPressEvent, px: i32, py: i32) -> Res<bool> {
        let area = view::address_rect(self.w);
        if area.contains(px, py) {
            if e.detail != 1 {
                return Ok(e.detail != 4 && e.detail != 5);
            }
            match &mut self.address {
                Some(edit) => edit.click(area.inset(2), px, e.state.contains(KeyButMask::SHIFT)),
                None => self.address = Some(ferro_ui::TextEdit::new(&self.shown_path())),
            }
            self.paint()?;
            return Ok(true);
        }
        // Anywhere else: the address is as it was.
        if self.address.take().is_some() {
            self.paint()?;
        }
        if e.detail == 1 {
            if let Some(i) = view::side_hit(self.w, self.h, self.places.len(), px, py) {
                let to = self.places[i].path.clone();
                self.go(to)?;
                return Ok(true);
            }
        }
        Ok(view::side_rect(self.w, self.h).contains(px, py))
    }

    /// Starts typing a new name for entry `i`, as Windows does: the name
    /// selected but for its extension, so typing replaces just that.
    fn start_rename(&mut self, i: usize) -> bool {
        let Some(e) = self.entries.get(i) else { return false };
        let name = e.real.clone();
        let stem = if e.is_dir { name.len() } else { Path::new(&name).file_stem().and_then(|s| s.to_str()).map_or(name.len(), str::len) };
        self.renaming = Some((i, ferro_ui::TextEdit::with_selection(&name, 0, stem)));
        self.selected = Some(i);
        self.selected_set.clear();
        self.selected_set.insert(i);
        // In sight.
        let cols = view::columns(self.w).max(1);
        let (row, visible) = (i / cols, view::visible_rows(self.h));
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + visible {
            self.scroll = (row + 1 - visible).min(view::max_scroll(self.w, self.h, self.entries.len()));
        }
        true
    }

    /// Something new was made here called `name`: its name, ready to type.
    fn rename_new(&mut self, name: &str) -> Res<()> {
        if let Some(i) = self.entries.iter().position(|e| e.real == name) {
            self.start_rename(i);
        }
        self.paint()
    }

    /// A click on the name being typed, if it's there: placing the cursor,
    /// selecting, or the Cut / Copy / Paste menu. True if it was.
    fn rename_click(&mut self, e: &ButtonPressEvent, px: i32, py: i32) -> Res<bool> {
        let Some((i, edit)) = &mut self.renaming else { return Ok(false) };
        let area = view::rename_rect(self.w, *i, self.scroll, edit.text());
        if !area.contains(px, py) {
            return Ok(false);
        }
        let inner = area.inset(1);
        match e.detail {
            1 => {
                let (before, which) = self.last_click;
                self.last_click = (e.time, None);
                if which.is_none() && e.time.wrapping_sub(before) < 400 {
                    edit.select_word(inner, px);
                } else {
                    edit.click(inner, px, e.state.contains(KeyButMask::SHIFT));
                    self.rename_drag = true;
                }
            }
            3 => {
                let (a, b) = edit.selection();
                let mut items = Vec::new();
                if edit.can_undo() {
                    items.push(("Undo", MenuAction::EditUndo));
                }
                if a != b {
                    items.extend([("Cut", MenuAction::EditCut), ("Copy", MenuAction::EditCopy)]);
                }
                items.push(("Paste", MenuAction::EditPaste));
                if a != b {
                    items.push(("Delete", MenuAction::EditDelete));
                }
                items.push(("Select All", MenuAction::EditSelectAll));
                self.menu = Some(view::make_menu(px, py, items, self.w, self.h));
            }
            _ => {}
        }
        self.paint()?;
        Ok(true)
    }

    fn commit_rename(&mut self) -> Res<()> {
        let Some((idx, edit)) = self.renaming.take() else { return Ok(()) };
        self.rename_drag = false;
        let new_name = edit.text().trim();
        if let Some(entry) = self.entries.get(idx) {
            if !new_name.is_empty() && new_name != entry.real {
                let from = self.dir.join(&entry.real);
                let said = ops::rename(&from, new_name);
                self.go(self.dir.clone())?;
                match said {
                    Ok(done) => self.status = done,
                    Err(why) => return self.tell(why),
                }
                self.selected = self.entries.iter().position(|e| e.real == new_name).or(Some(idx));
                return Ok(());
            }
        }
        self.paint()
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
        // A second click: a double click (opening), not a rename.
        self.rename_soon = None;
        self.rename_armed = None;
        if self.renaming.is_some() {
            // The menu on the name being typed acts on it.
            if let Some(menu) = self.menu.take() {
                if let Some((_, action)) = view::menu_hit(&menu, px, py) {
                    self.action(action)?;
                }
                return self.paint();
            }
            if self.rename_click(e, px, py)? {
                return Ok(());
            }
            self.commit_rename()?;
        }
        if self.menu.is_none() && self.prompt.is_none() && self.bar_click(e, px, py)? {
            return Ok(());
        }
        // A column heading: by it, or the other way round if already.
        if let Some(col) = view::header_hit(self.w, px, py).filter(|_| e.detail == 1 && self.menu.is_none() && self.prompt.is_none()) {
            let up = if self.sort.0 == col { !self.sort.1 } else { col.starts_up() };
            self.set_sort((col, up));
            return self.paint();
        }
        if self.prompt.is_some() {
            let (ok, cancel) = view::prompt_buttons(self.w, self.h);
            let notice = self.prompt.as_ref().is_some_and(|(p, _)| p.notice);
            if e.detail == 1 && ok.contains(px, py) && !notice {
                return self.answer(true);
            }
            if e.detail == 1 && cancel.contains(px, py) {
                return self.answer(false);
            }
            return Ok(());
        }
        if let Some(mut menu) = self.menu.take() {
            match view::menu_hit(&menu, px, py) {
                // New or View: opened beside, the menu stays.
                Some((idx, MenuAction::Submenu(k))) => {
                    self.open_sub(&mut menu, idx, k);
                    self.menu = Some(menu);
                }
                Some((_, action)) => self.action(action)?,
                None => {}
            }
            return self.paint();
        }
        let most = view::max_scroll(self.w, self.h, self.entries.len());
        let ctrl = e.state.contains(KeyButMask::CONTROL);
        match e.detail {
            // Ctrl and the wheel: bigger or smaller icons.
            4 if ctrl => return self.set_icon_size(view::icon_size() + 1),
            5 if ctrl => return self.set_icon_size(view::icon_size().saturating_sub(1)),
            // The wheel (dragging too: to a folder further up or down).
            4 => {
                self.scroll = self.scroll.saturating_sub(3);
                self.after_scroll();
            }
            5 => {
                self.scroll = (self.scroll + 3).min(most);
                self.after_scroll();
            }
            1 => match view::hit(self.w, self.h, &self.entries, self.scroll, px, py) {
                Hit::Up => return self.up(),
                Hit::ScrollUp => {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                Hit::ScrollDown => {
                    self.scroll = (self.scroll + 1).min(most);
                }
                Hit::ScrollPage(down) => {
                    let vis = view::visible_rows(self.h);
                    if down {
                        self.scroll = (self.scroll + vis).min(most);
                    } else {
                        self.scroll = self.scroll.saturating_sub(vis);
                    }
                }
                Hit::ScrollThumb => {
                    self.drag_scroll = Some((py, self.scroll));
                }
                Hit::Entry(i) => {
                    self.drag_start = Some((i, px, py));
                    let (before, which) = self.last_click;
                    self.last_click = (e.time, Some(i));
                    let ctrl = e.state.contains(KeyButMask::CONTROL);
                    if which == Some(i) && e.time.wrapping_sub(before) < DOUBLE_CLICK_MS {
                        self.last_click = (0, None);
                        return self.open(i);
                    }
                    // Already the one selected: renamed once let go (see release).
                    let already_selected = self.selected == Some(i) && self.selected_set.len() <= 1;
                    let shift = e.state.contains(KeyButMask::SHIFT);
                    self.rename_armed = (already_selected && !ctrl && !shift && !self.is_my_computer).then_some(i);
                    if ctrl {
                        if self.selected_set.contains(&i) {
                            self.selected_set.remove(&i);
                            if self.selected == Some(i) {
                                self.selected = self.selected_set.iter().copied().next();
                            }
                        } else {
                            self.selected_set.insert(i);
                            self.selected = Some(i);
                        }
                    } else if !self.selected_set.contains(&i) {
                        self.selected_set.clear();
                        self.selected_set.insert(i);
                        self.selected = Some(i);
                    } else {
                        self.selected = Some(i);
                    }
                }
                Hit::Nothing => {
                    self.selected = None;
                    self.selected_set.clear();
                    self.rubberband = Some((px, py, px, py));
                    self.rubber_scroll = self.scroll;
                    self.last_click = (e.time, None);
                }
            },
            3 => match view::hit(self.w, self.h, &self.entries, self.scroll, px, py) {
                Hit::Entry(i) => {
                    if !self.selected_set.contains(&i) {
                        self.selected_set.clear();
                        self.selected_set.insert(i);
                        self.selected = Some(i);
                    }
                    let items = if self.is_my_computer {
                        vec![("Open", MenuAction::Open), ("Properties", MenuAction::Properties)]
                    } else {
                        let is_folder = self.entries.get(i).is_some_and(|e| e.is_dir);
                        let mut items = vec![("Open", MenuAction::Open)];
                        if self.entries.get(i).is_some_and(|e| ops::is_installable(&e.real)) {
                            items.push(("Install", MenuAction::Install));
                        }
                        if self.entries.get(i).is_some_and(|e| ops::is_archive(&e.real)) {
                            items.push(("Extract Here", MenuAction::ExtractHere));
                        }
                        if is_folder && self.get_clipboard().is_some() {
                            items.push(("Paste", MenuAction::Paste));
                        }
                        items.extend([
                            ("Cut", MenuAction::Cut),
                            ("Copy", MenuAction::Copy),
                            ("Rename", MenuAction::Rename),
                            ("Delete", MenuAction::Delete),
                            ("Create Shortcut", MenuAction::CreateShortcut),
                            ("Properties", MenuAction::Properties),
                        ]);
                        items
                    };
                    self.menu = Some(view::make_menu(px, py, items, self.w, self.h));
                }
                _ => {
                    self.selected = None;
                    self.selected_set.clear();
                    // As Windows has it: View and Refresh first, then Paste
                    // and New.
                    let mut items = vec![
                        ("View", MenuAction::Submenu(view::SUB_VIEW)),
                        ("Sort by", MenuAction::Submenu(view::SUB_SORT)),
                        ("Refresh", MenuAction::Refresh),
                    ];
                    if !self.is_my_computer {
                        if self.get_clipboard().is_some() {
                            items.push(("Paste", MenuAction::Paste));
                        }
                        items.extend([("New", MenuAction::Submenu(view::SUB_NEW)), ("Open in Terminal", MenuAction::Terminal)]);
                    }
                    self.menu = Some(view::make_menu(px, py, items, self.w, self.h));
                }
            },
            _ => return Ok(()),
        }
        self.paint()
    }

    fn motion(&mut self, e: &MotionNotifyEvent) -> Res<()> {
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
        if self.rename_drag {
            if let Some((i, edit)) = &mut self.renaming {
                let area = view::rename_rect(self.w, *i, self.scroll, edit.text()).inset(1);
                edit.drag(area, px);
                return self.paint();
            }
        }
        if let Some((start_y, start_scroll)) = self.drag_scroll {
            if let Some(sb) = view::scrollbar_layout(self.w, self.h, self.entries.len(), self.scroll) {
                let track_len = (sb.track.h - sb.thumb.h).max(1);
                let dy = py - start_y;
                let most = view::max_scroll(self.w, self.h, self.entries.len());
                let delta_scroll = ((dy as f32 / track_len as f32) * most as f32).round() as i32;
                let new_scroll = (start_scroll as i32 + delta_scroll).clamp(0, most as i32) as usize;
                if new_scroll != self.scroll {
                    self.scroll = new_scroll;
                    return self.paint();
                }
            }
            return Ok(());
        }
        if let Some(mut menu) = self.menu.take() {
            let before = (menu.hover, menu.sub.as_ref().map(|(k, m)| (*k, m.hover)));
            match menu.sub.as_mut().filter(|(_, sub)| sub.rect.contains(px, py)) {
                Some((_, sub)) => sub.hover = view::menu_item_at(sub, px, py),
                None => {
                    menu.hover = view::menu_item_at(&menu, px, py);
                    // Over New or View: it opens beside; over another, it goes.
                    match menu.hover.map(|i| (i, menu.items[i].1)) {
                        Some((i, MenuAction::Submenu(k))) if menu.sub.as_ref().map(|s| s.0) != Some(k) => self.open_sub(&mut menu, i, k),
                        Some((_, MenuAction::Submenu(_))) | None => {}
                        Some(_) => menu.sub = None,
                    }
                }
            }
            let changed = before != (menu.hover, menu.sub.as_ref().map(|(k, m)| (*k, m.hover)));
            self.menu = Some(menu);
            return if changed { self.paint() } else { Ok(()) };
        }
        if let Some((sx, sy, _, _)) = self.rubberband {
            self.rubberband = Some((sx, sy, px, py));
            self.select_in_box();
            // Drawn once the pointer's moves so far are all in.
            self.want_paint = true;
            return Ok(());
        }
        if let Some((i, sx, sy)) = self.drag_start {
            if (px - sx).abs() > 6 || (py - sy).abs() > 6 {
                self.dragging = Some(i);
            }
        }
        if let Some(i) = self.dragging {
            let at = (i32::from(e.root_x), i32::from(e.root_y));
            match &self.drag_icon {
                Some(d) => d.move_to(&self.conn, at)?,
                None => {
                    let many = self.selected_set.contains(&i) && self.selected_set.len() > 1;
                    if let Some(entry) = self.entries.get(i) {
                        let icon = entry.icon.unwrap_or(if entry.is_dir { ferro_gfx::icons::FOLDER } else { ferro_gfx::icons::FILE });
                        let label = if many { format!("{} items", self.selected_set.len()) } else { entry.name.clone() };
                        let thumb = if many { None } else { self.thumbnails.get(&entry.real) };
                        let grab = self.drag_start.map_or((16, 16), |(_, sx, sy)| view::grab_point(self.w, i, self.scroll, &entry.name, (sx, sy)));
                        self.drag_icon = Some(ferro_ui::DragIcon::new(&self.conn, &self.screen, icon, thumb, &label, at, grab)?);
                    }
                }
            }
            self.drag_pos = Some((px, py));
            let hit = view::hit(self.w, self.h, &self.entries, self.scroll, px, py);
            self.drag_target = match hit {
                Hit::Entry(dst) if self.entries.get(dst).is_some_and(|e| e.is_dir) => Some(dst),
                _ => None,
            };
            self.want_paint = true;
        }
        Ok(())
    }

    /// The selection box on screen: from where it was started (which moves
    /// with the icons as they scroll) to the pointer.
    fn rubber_rect(&self) -> Option<Rect> {
        let (x0, y0, x1, y1) = self.rubberband?;
        let y0 = y0 - (self.scroll as i32 - self.rubber_scroll as i32) * view::row_h();
        Some(Rect::new(x0.min(x1), y0.min(y1), (x0.max(x1) - x0.min(x1)).max(1), (y0.max(y1) - y0.min(y1)).max(1)))
    }

    /// Selects what the selection box takes in.
    fn select_in_box(&mut self) {
        let Some(r) = self.rubber_rect() else { return };
        self.selected_set.clear();
        // Only the rows the box spans: as quick with thousands of files.
        let cols = view::columns(self.w);
        let first = self.scroll as i64 + i64::from((r.y - view::pane_y_range(self.h).0) / view::row_h()) - 1;
        let last = self.scroll as i64 + i64::from((r.bottom() - view::pane_y_range(self.h).0) / view::row_h()) + 1;
        let (from, to) = ((first.max(0) as usize) * cols, ((last.max(0) as usize + 1) * cols).min(self.entries.len()));
        for i in from..to {
            if view::item_overlaps(self.w, i, self.scroll, &self.entries[i].name, &r) {
                self.selected_set.insert(i);
            }
        }
        if self.selected_set.is_empty() {
            self.selected = None;
        } else if self.selected.is_none_or(|i| !self.selected_set.contains(&i)) {
            self.selected = self.selected_set.iter().copied().min();
        }
    }

    /// What's under a held pointer changes when the icons move under it.
    fn after_scroll(&mut self) {
        self.select_in_box();
        if let (Some(_), Some((px, py))) = (self.dragging, self.drag_pos) {
            self.drag_target = match view::hit(self.w, self.h, &self.entries, self.scroll, px, py) {
                Hit::Entry(dst) if self.entries.get(dst).is_some_and(|e| e.is_dir) => Some(dst),
                _ => None,
            };
        }
    }

    /// Small, Medium, Large or Extra large icons: kept for next time, and
    /// the thumbnails made again at the new size.
    fn set_icon_size(&mut self, i: usize) -> Res<()> {
        if i == view::icon_size() || i >= view::SIZES.len() {
            return Ok(());
        }
        let first_row = self.scroll * view::columns(self.w);
        view::set_icon_size(i);
        save_settings(self.show_hidden, i, self.sort);
        self.scroll = (first_row / view::columns(self.w)).min(view::max_scroll(self.w, self.h, self.entries.len()));
        self.thumbs_dir = PathBuf::new();
        self.load_thumbnails();
        self.status = format!("{}.", view::SIZES[i].0);
        self.paint()
    }

    fn release(&mut self, e: &ButtonReleaseEvent) -> Res<()> {
        // Only the left button ends a drag: each notch of the wheel is a
        // press and a release too, and scrolls while the drag goes on.
        if e.detail != 1 {
            return Ok(());
        }
        self.rename_drag = false;
        // Let go where it was pressed: the name is typed in a moment, unless
        // a second click makes it a double click first.
        if let Some(i) = self.rename_armed.take().filter(|_| self.dragging.is_none()) {
            self.rename_soon = Some((i, std::time::Instant::now() + std::time::Duration::from_millis(u64::from(DOUBLE_CLICK_MS))));
        }
        if let Some(d) = self.drag_icon.take() {
            d.close(&self.conn)?;
        }
        self.drag_scroll = None;
        self.drag_start = None;
        let _ = self.drag_pos.take();
        let _ = self.drag_target.take();
        if self.rubberband.take().is_some() {
            return self.paint();
        }
        if let Some(src_idx) = self.dragging.take() {
            if self.is_my_computer {
                return self.paint();
            }
            let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
            if px < 0 || px >= self.w || py < 0 || py >= self.h {
                let to_move: Vec<PathBuf> = if self.selected_set.contains(&src_idx) && !self.selected_set.is_empty() {
                    self.selected_paths()
                } else if let Some(src_e) = self.entries.get(src_idx) {
                    vec![self.dir.join(&src_e.real)]
                } else {
                    vec![]
                };
                if !to_move.is_empty() {
                    let cut = !e.state.contains(KeyButMask::CONTROL);
                    let mut target_dir: Option<PathBuf> = None;
                    let mut notify_win: Option<x11rb::protocol::xproto::Window> = None;
                    let mut drop_on_recycle_bin = false;

                    if let Ok(cookie) = self.conn.query_pointer(self.root) {
                        if let Ok(qp) = cookie.reply() {
                            let child = qp.child;
                            let over_folder = if child != x11rb::NONE && child != self.root {
                                find_ferro_dir(&self.conn, child, self.ferro_current_dir)
                            } else {
                                None
                            };
                            match over_folder {
                                Some((twin, tdir)) => {
                                    if tdir != self.dir {
                                        target_dir = Some(tdir);
                                        notify_win = Some(twin);
                                    }
                                }
                                // Over the desktop: on the Recycle Bin's icon, wherever
                                // it's been put (the desktop says where it is).
                                None => {
                                    let (px, py) = (i32::from(qp.root_x), i32::from(qp.root_y));
                                    drop_on_recycle_bin = recycle_bin_rect(&self.conn, self.root, self.ferro_recycle_bin)
                                        .is_some_and(|(x, y, w, h)| (x..x + w).contains(&px) && (y..y + h).contains(&py));
                                    // On a folder's icon: into that folder (never a folder
                                    // into itself, or into one inside it).
                                    if !drop_on_recycle_bin {
                                        target_dir = desktop_folder_at(&self.conn, self.root, self.ferro_desktop_folders, px, py)
                                            .filter(|d| *d != self.dir && !to_move.iter().any(|m| d.starts_with(m)));
                                    }
                                }
                            }
                        }
                    }

                    // Onto a window showing the Recycle Bin: the same as onto its icon
                    // (never copied there from a removable drive).
                    drop_on_recycle_bin |= target_dir.as_ref().is_some_and(|d| d.starts_with(ferro_associations::trash::dir()));
                    if drop_on_recycle_bin {
                        if !self.recycle(to_move)? {
                            return Ok(());
                        }
                        let msg = ClientMessageEvent::new(32, self.root, self.ferro_refresh, [0; 5]);
                        let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, msg);
                        let _ = self.conn.flush();
                        let cur = self.dir.clone();
                        self.go(cur)?;
                        return self.paint();
                    }

                    let dest_is_desktop = target_dir.is_none();
                    let dest = if let Some(tdir) = target_dir {
                        tdir
                    } else if let Ok(home) = std::env::var("HOME") {
                        let desk = PathBuf::from(home).join("Desktop");
                        if desk.is_dir() {
                            desk
                        } else {
                            PathBuf::from("/")
                        }
                    } else {
                        PathBuf::from("/")
                    };

                    let names: Vec<String> = to_move.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
                    let count = to_move.len();
                    let verb = if cut { "Moving" } else { "Copying" };
                    let dest_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dest.to_string_lossy().into_owned());
                    self.status = format!("{verb} {count} item(s) to {dest_name}...");
                    self.transfer(to_move, dest.clone(), cut, format!("to {dest_name}"))?;

                    if let Some(twin) = notify_win {
                        let msg = ClientMessageEvent::new(32, twin, self.ferro_refresh, [0; 5]);
                        let _ = self.conn.send_event(false, twin, EventMask::NO_EVENT, msg);
                        let _ = self.conn.flush();
                    } else if dest_is_desktop {
                        // Where they were let go: the desktop puts them there.
                        let list = names.join("\n");
                        let _ = self.conn.change_property8(PropMode::REPLACE, self.root, self.ferro_place, self.utf8_string, list.as_bytes());
                        let at = [i32::from(e.root_x) as u32, i32::from(e.root_y) as u32, 0, 0, 0];
                        let place = ClientMessageEvent::new(32, self.root, self.ferro_place, at);
                        let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, place);
                        let msg = ClientMessageEvent::new(32, self.root, self.ferro_refresh, [0; 5]);
                        let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, msg);
                        let _ = self.conn.flush();
                    }

                    if cut {
                        let cur = self.dir.clone();
                        self.go(cur)?;
                    }
                    return self.paint();
                }
            }
            let hit = view::hit(self.w, self.h, &self.entries, self.scroll, px, py);
            match hit {
                Hit::Entry(dst_idx) if dst_idx != src_idx => {
                    if let (Some(src_e), Some(dst_e)) = (self.entries.get(src_idx), self.entries.get(dst_idx)) {
                        if dst_e.is_dir {
                            let mut to_move: Vec<PathBuf> = if self.selected_set.contains(&src_idx) && !self.selected_set.is_empty() {
                                self.selected_paths()
                            } else {
                                vec![self.dir.join(&src_e.real)]
                            };
                            if dst_e.real.eq_ignore_ascii_case("trash") || dst_e.name.eq_ignore_ascii_case("recycle bin") {
                                if !self.recycle(to_move)? {
                                    return Ok(());
                                }
                                let msg = ClientMessageEvent::new(32, self.root, self.ferro_refresh, [0; 5]);
                                let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, msg);
                                let _ = self.conn.flush();
                                let cur = self.dir.clone();
                                self.go(cur)?;
                                return self.paint();
                            }
                            let to_dir = self.dir.join(&dst_e.real);
                            to_move.retain(|p| p != &to_dir && !to_dir.starts_with(p));
                            if to_move.is_empty() {
                                return self.paint();
                            }
                            let target_name = dst_e.name.clone();
                            let count = to_move.len();
                            self.status = format!("Moving {count} item(s) into '{target_name}'...");
                            self.transfer(to_move, to_dir, true, format!("into '{target_name}'"))?;
                        }
                    }
                }
                Hit::Up => {
                    if let Some(src_e) = self.entries.get(src_idx) {
                        if let Some(parent) = self.dir.parent() {
                            let to_dir = parent.to_path_buf();
                            let mut to_move: Vec<PathBuf> = if self.selected_set.contains(&src_idx) && !self.selected_set.is_empty() {
                                self.selected_paths()
                            } else {
                                vec![self.dir.join(&src_e.real)]
                            };
                            to_move.retain(|p| p != &to_dir && !to_dir.starts_with(p));
                            if to_move.is_empty() {
                                return self.paint();
                            }
                            let count = to_move.len();
                            self.status = format!("Moving {count} item(s) up...");
                            self.transfer(to_move, to_dir, true, "up".into())?;
                        }
                    }
                }
                _ => {}
            }
            return self.paint();
        }
        Ok(())
    }

    fn action(&mut self, action: MenuAction) -> Res<()> {
        match action {
            // Opened where it's clicked or pointed at, not here.
            MenuAction::Submenu(_) => {}
            MenuAction::Open => {
                if let Some(i) = self.selected {
                    self.open(i)?;
                }
            }
            MenuAction::CreateShortcut => {
                if let Some(i) = self.selected {
                    if let Some(e) = self.entries.get(i) {
                        #[cfg(unix)]
                        {
                            let target = self.dir.join(&e.real);
                            let desktop = if let Ok(home) = std::env::var("HOME") {
                                PathBuf::from(home).join("Desktop")
                            } else {
                                PathBuf::from("/home/ferro/Desktop")
                            };
                            let _ = std::fs::create_dir_all(&desktop);
                            let mut base_name = format!("Shortcut to {}", e.real);
                            let mut dest = desktop.join(&base_name);
                            let mut count = 2;
                            while dest.exists() || std::fs::symlink_metadata(&dest).is_ok() {
                                base_name = format!("Shortcut to {} ({count})", e.real);
                                dest = desktop.join(&base_name);
                                count += 1;
                            }
                            let _ = std::os::unix::fs::symlink(&target, &dest);
                            let msg = ClientMessageEvent::new(32, self.root, self.ferro_refresh, [0; 5]);
                            let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, msg);
                            self.status = format!("Created shortcut on Desktop: {base_name}");
                        }
                        #[cfg(not(unix))]
                        let _ = &e;
                    }
                }
            }
            MenuAction::IconSize(i) => self.set_icon_size(i)?,
            MenuAction::SortBy(col) => {
                let up = if self.sort.0 == col { self.sort.1 } else { col.starts_up() };
                self.set_sort((col, up));
            }
            MenuAction::SortUp(up) => self.set_sort((self.sort.0, up)),
            MenuAction::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                save_settings(self.show_hidden, view::icon_size(), self.sort);
                self.status =
                    if self.show_hidden { "Showing hidden files and folders.".into() } else { "Hidden files and folders are now hidden.".into() };
                let cur = self.dir.clone();
                self.go(cur)?;
            }
            MenuAction::Install => {
                if let Some(path) = self.selected_path() {
                    self.install(&path)?;
                }
            }
            MenuAction::ExtractHere => {
                if let Some(path) = self.selected_path() {
                    self.extract(path)?;
                }
            }
            MenuAction::Cut | MenuAction::Copy => {
                let paths = self.selected_paths();
                if !paths.is_empty() {
                    let cut = action == MenuAction::Cut;
                    if paths.len() == 1 {
                        let name = paths[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.status = format!("'{name}' {}: right-click a folder and Paste (Ctrl+V).", if cut { "cut" } else { "copied" });
                    } else {
                        self.status =
                            format!("{} items {}: right-click a folder and Paste (Ctrl+V).", paths.len(), if cut { "cut" } else { "copied" });
                    }
                    self.set_clipboard(paths, cut);
                    self.paint()?;
                }
            }
            MenuAction::Paste => {
                if let Some((from_paths, cut)) = self.get_clipboard() {
                    if self.is_my_computer {
                        return Ok(());
                    }
                    if cut {
                        self.clear_clipboard();
                    }
                    let dir = if let Some(i) = self.selected {
                        if self.entries.get(i).is_some_and(|e| e.is_dir) {
                            self.dir.join(&self.entries[i].real)
                        } else {
                            self.dir.clone()
                        }
                    } else {
                        self.dir.clone()
                    };
                    let count = from_paths.len();
                    let action = if cut { "Moving" } else { "Copying" };
                    self.status = format!("{action} {count} item(s)...");
                    self.transfer(from_paths, dir, cut, String::new())?;
                }
            }
            MenuAction::Rename => {
                if let Some(i) = self.selected {
                    if self.start_rename(i) {
                        return self.paint();
                    }
                }
            }
            // A drive in My Computer is never deleted, nor what's on it.
            MenuAction::Delete | MenuAction::DeleteForGood if self.is_my_computer => {}
            // Delete sends to the Recycle Bin at once (it can be got back
            // from there), but asks about what's on a removable drive;
            // Shift+Delete, and Delete on what's already in the Recycle Bin,
            // is for good and asks first.
            MenuAction::Delete if !self.dir.starts_with(ferro_associations::trash::dir()) => {
                let paths = self.selected_paths();
                if !paths.is_empty() {
                    self.recycle(paths)?;
                }
                return Ok(());
            }
            MenuAction::Delete | MenuAction::DeleteForGood => {
                let paths = self.selected_paths();
                if paths.len() == 1 {
                    let path = paths[0].clone();
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let what = if path.is_dir() { "the folder" } else { "" };
                    let message = format!("Delete {what} '{name}' for good? It can't be got back.").replace("  ", " ");
                    ferro_ui::sound("ding");
                    self.prompt = Some((Prompt { message, input: None, notice: false }, Asking::Delete(path)));
                    self.paint()?;
                } else if paths.len() > 1 {
                    let count = paths.len();
                    let message = format!("Delete these {count} items for good? They can't be got back.");
                    ferro_ui::sound("ding");
                    self.prompt = Some((Prompt { message, input: None, notice: false }, Asking::DeleteMulti(paths)));
                    self.paint()?;
                }
            }
            MenuAction::Properties => {
                let path = if self.is_my_computer {
                    if let Some(i) = self.selected {
                        self.entries.get(i).map(|e| PathBuf::from(&e.real))
                    } else {
                        None
                    }
                } else if let Some(p) = self.selected_path() {
                    Some(p)
                } else {
                    Some(self.dir.clone())
                };
                if let Some(p) = path {
                    let cmd = format!("ferro-properties {}", ferro_associations::shell_quote(&p.to_string_lossy()));
                    launch("sh", &["-c", &cmd]);
                } else {
                    launch("ferro-system", &[]);
                }
            }
            MenuAction::EditUndo
            | MenuAction::EditCut
            | MenuAction::EditCopy
            | MenuAction::EditPaste
            | MenuAction::EditDelete
            | MenuAction::EditSelectAll => {
                if let Some((_, edit)) = &mut self.renaming {
                    match action {
                        MenuAction::EditUndo => edit.undo(),
                        MenuAction::EditCut => edit.cut(),
                        MenuAction::EditCopy => edit.copy(),
                        MenuAction::EditPaste => edit.paste(),
                        MenuAction::EditDelete => edit.delete(),
                        _ => edit.select_all(),
                    }
                }
            }
            MenuAction::NewFolder => {
                let mut name = "New Folder".to_string();
                let mut n = 1;
                while self.dir.join(&name).exists() {
                    n += 1;
                    name = format!("New Folder ({n})");
                }
                let made = std::fs::create_dir(self.dir.join(&name));
                self.go(self.dir.clone())?;
                match made {
                    Err(e) => self.tell(ops::explain("make", &name, &e))?,
                    // Named as it's made, as on Windows.
                    Ok(()) => return self.rename_new(&name),
                }
            }
            MenuAction::NewDocument => {
                let mut name = "New Text Document.txt".to_string();
                let mut n = 1;
                while self.dir.join(&name).exists() {
                    n += 1;
                    name = format!("New Text Document ({n}).txt");
                }
                let made = std::fs::write(self.dir.join(&name), "");
                self.go(self.dir.clone())?;
                match made {
                    Err(e) => self.tell(ops::explain("make", &name, &e))?,
                    Ok(()) => return self.rename_new(&name),
                }
            }
            MenuAction::Terminal => {
                let cwd = self.dir.display().to_string();
                let cmd = format!("cd {} && (ferro-terminal || lxterminal || xterm)", ferro_associations::shell_quote(&cwd));
                launch("sh", &["-c", &cmd]);
            }
            MenuAction::Refresh => {
                if self.is_my_computer {
                    self.go_computer()?;
                } else {
                    let (drives, detected) = drives::system_drives_with_detected();
                    self.drives = drives;
                    self.detected_drives = detected;
                    self.go(self.dir.clone())?;
                }
            }
        }
        Ok(())
    }

    fn selected_path(&self) -> Option<PathBuf> {
        if self.is_my_computer {
            return None;
        }
        self.selected.and_then(|i| self.entries.get(i)).map(|e| self.dir.join(&e.real))
    }

    /// Runs the program installer on `path`, in a terminal window.
    fn install(&mut self, path: &Path) -> Res<()> {
        let q = ferro_associations::shell_quote(&path.to_string_lossy());
        launch("sh", &["-c", &format!("ferro-terminal -e /usr/local/bin/ferro-installer {q}")]);
        Ok(())
    }

    /// Unpacks an archive beside itself, aside.
    fn extract(&mut self, path: PathBuf) -> Res<()> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.status = format!("Unpacking '{name}'...");
        self.aside(move || ops::extract_here(&path).map(|(said, _)| said))
    }

    /// Copies (or moves, when `cut`) `from` into `to`, aside, with one
    /// progress figure for all of it in the status bar. `whither` ends the
    /// message ("to Desktop", "into 'Photos'").
    fn transfer(&mut self, from: Vec<PathBuf>, to: PathBuf, cut: bool, whither: String) -> Res<()> {
        let verb = if cut { "Moving" } else { "Copying" };
        let (rep, wake) = (self.report.clone(), self.wake);
        self.aside(move || {
            let said = ops::paste_all(&from, &to, cut, |pct, copied, total, what| {
                let msg = format!("PROGRESS:{pct}%  {verb} {what} ({} of {})", ops::human_size(copied), ops::human_size(total));
                let _ = rep.send(Ok(msg));
                #[cfg(unix)]
                // SAFETY: writes one byte from a live buffer to our own pipe.
                unsafe {
                    libc::write(wake, [1u8].as_ptr().cast(), 1);
                }
                #[cfg(not(unix))]
                let _ = wake;
            });
            match said {
                Ok(done) if !whither.is_empty() => Ok(format!("{} {whither}.", done.trim_end_matches('.'))),
                other => other,
            }
        })
    }

    /// Runs slow work (copying to a USB stick can take minutes) on its own
    /// thread; the window is told when it's done.
    fn aside(&mut self, work: impl FnOnce() -> Result<String, String> + Send + 'static) -> Res<()> {
        let (report, wake) = (self.report.clone(), self.wake);
        self.busy += 1;
        std::thread::spawn(move || {
            let _ = report.send(work());
            #[cfg(unix)]
            // SAFETY: writes one byte from a live buffer to our own pipe.
            unsafe {
                libc::write(wake, [1u8].as_ptr().cast(), 1);
            }
            #[cfg(not(unix))]
            let _ = wake;
        });
        self.paint()
    }

    /// Delete: into the Recycle Bin at once; but what's on a removable
    /// drive can't go there (it's never copied onto the main drive), so for
    /// that, as on Windows 95, the question whether to delete it for good.
    /// Whether it went to the Recycle Bin (`false`: it's asking).
    fn recycle(&mut self, paths: Vec<PathBuf>) -> Res<bool> {
        let off = paths.iter().filter(|p| !ferro_associations::trash::can_take(p)).count();
        if off == 0 {
            let count = paths.len();
            self.status = format!("Moving {count} item(s) to Recycle Bin...");
            self.forget(&paths);
            self.aside(move || ops::to_recycle_bin(&paths))?;
            return Ok(true);
        }
        let message = if paths.len() == 1 {
            let name = paths[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            format!("'{name}' is on a removable drive, so it can't go to the Recycle Bin. Delete it for good? It can't be got back.")
        } else if off == paths.len() {
            format!("These {off} items are on a removable drive, so they can't go to the Recycle Bin. Delete them for good? They can't be got back.")
        } else {
            format!(
                "{off} of these items are on a removable drive, so they can't go to the Recycle Bin. Delete those for good? They can't be got back."
            )
        };
        ferro_ui::sound("ding");
        self.prompt = Some((Prompt { message, input: None, notice: false }, Asking::OffDrive(paths)));
        self.paint()?;
        Ok(false)
    }

    /// Nothing chosen any more, nor on the clipboard, of what's going.
    fn forget(&mut self, paths: &[PathBuf]) {
        self.selected = None;
        self.selected_set.clear();
        if self.clipboard.as_ref().is_some_and(|(p, _)| paths.contains(p)) {
            self.clipboard = None;
        }
    }

    /// What the work aside said, now it's done.
    fn finished(&mut self) -> Res<()> {
        self.take_thumbnails();
        let mut failed = None;
        let mut reload = false;
        while let Ok(said) = self.reports.try_recv() {
            match said {
                Ok(done) if done.starts_with("PROGRESS:") => {
                    if let Some(msg) = done.strip_prefix("PROGRESS:") {
                        self.status = msg.to_string();
                    }
                }
                Ok(done) => {
                    self.busy = self.busy.saturating_sub(1);
                    self.status = done;
                    reload = true;
                }
                Err(why) => {
                    self.busy = self.busy.saturating_sub(1);
                    failed = Some(why);
                    reload = true;
                }
            }
        }
        if reload {
            if self.is_my_computer {
                self.paint()?;
            } else {
                self.go(self.dir.clone())?;
            }
        } else {
            self.paint()?;
        }
        if let Some(why) = failed {
            self.tell(why)?;
        }
        Ok(())
    }

    /// Says what went wrong, in a box that waits for OK.
    fn tell(&mut self, mut why: String) -> Res<()> {
        self.status.clear();
        // A Linux drive (ext4 and the like) belongs to whoever made it.
        if why.contains("permission") && self.dir.starts_with("/media") {
            why.push_str(" If this is a Linux drive, an administrator can give it to you in Terminal: sudo chown -R $USER on its folder.");
        }
        ferro_ui::sound("error");
        self.prompt = Some((Prompt { message: why, input: None, notice: true }, Asking::Nothing));
        self.paint()
    }

    /// Yes/OK (`true`) or No/Cancel on the prompt.
    fn answer(&mut self, yes: bool) -> Res<()> {
        let Some((prompt, asking)) = self.prompt.take() else { return Ok(()) };
        if yes {
            match asking {
                Asking::Delete(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.status = format!("Deleting '{name}'...");
                    self.selected = None;
                    self.selected_set.clear();
                    if self.clipboard.as_ref().is_some_and(|(p, _)| *p == path) {
                        self.clipboard = None;
                    }
                    return self.aside(move || ops::delete(&path));
                }
                Asking::DeleteMulti(paths) => {
                    let count = paths.len();
                    self.status = format!("Deleting {count} items...");
                    self.selected = None;
                    self.selected_set.clear();
                    self.clipboard = None;
                    return self.aside(move || {
                        for p in &paths {
                            ops::delete(p)?;
                        }
                        Ok(format!("Deleted {count} items"))
                    });
                }
                Asking::OffDrive(paths) => {
                    self.status = match paths.as_slice() {
                        [one] => format!("Deleting '{}'...", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                        _ => format!("Deleting {} items...", paths.len()),
                    };
                    self.forget(&paths);
                    return self.aside(move || ops::recycle_or_delete(&paths));
                }
                Asking::Nothing => {}
                Asking::Rename(path) => {
                    let new_name = prompt.input.unwrap_or_default();
                    let said = ops::rename(&path, &new_name);
                    let dir = self.dir.clone();
                    self.go(dir)?;
                    match said {
                        Ok(done) => self.status = done,
                        Err(why) => return self.tell(why),
                    }
                    // The renamed one stays chosen.
                    self.selected = self.entries.iter().position(|e| e.real == new_name.trim()).or(self.selected);
                }
            }
        }
        self.paint()
    }

    /// A key: the prompt's first, then the folder's.
    fn key(&mut self, e: &KeyPressEvent) -> Res<()> {
        let ctrl = e.state.contains(KeyButMask::CONTROL);
        if let Some(edit) = &mut self.address {
            let shift = e.state.contains(KeyButMask::SHIFT);
            return match edit.key(self.keymap.typed(e), ctrl, shift) {
                ferro_ui::Edit::Done => {
                    let typed = edit.text().to_owned();
                    self.address = None;
                    self.go_typed(&typed)
                }
                ferro_ui::Edit::Cancel => {
                    self.address = None;
                    self.paint()
                }
                ferro_ui::Edit::Changed => self.paint(),
                ferro_ui::Edit::Ignored => Ok(()),
            };
        }
        // Ctrl+L or Alt+D: to the address, to type one (as on Windows).
        let alt = e.state.contains(KeyButMask::MOD1);
        if self.renaming.is_none() && ((ctrl && e.detail == KEY_L) || (alt && e.detail == KEY_D)) {
            self.address = Some(ferro_ui::TextEdit::new(&self.shown_path()));
            return self.paint();
        }
        if let Some((_, edit)) = &mut self.renaming {
            let shift = e.state.contains(KeyButMask::SHIFT);
            return match edit.key(self.keymap.typed(e), ctrl, shift) {
                ferro_ui::Edit::Done => self.commit_rename(),
                ferro_ui::Edit::Cancel => {
                    self.renaming = None;
                    self.paint()
                }
                ferro_ui::Edit::Changed => self.paint(),
                ferro_ui::Edit::Ignored => Ok(()),
            };
        }
        if let Some((prompt, _)) = &mut self.prompt {
            if let Some(yes) = ferro_ui::dialog_answer(self.keymap.typed(e)) {
                return self.answer(yes);
            }
            match (e.detail, self.keymap.typed(e)) {
                (_, ferro_ui::Key::Char('y' | 'Y')) if prompt.input.is_none() && !prompt.notice => return self.answer(true),
                (_, ferro_ui::Key::Char('n' | 'N')) if prompt.input.is_none() => return self.answer(false),
                (_, ferro_ui::Key::Backspace) => {
                    if let Some(t) = &mut prompt.input {
                        t.pop();
                    }
                }
                (_, ferro_ui::Key::Char(c)) if !c.is_control() && !ctrl => {
                    if let Some(t) = &mut prompt.input {
                        if t.len() < 255 {
                            t.push(c);
                        }
                    }
                }
                _ => return Ok(()),
            }
            return self.paint();
        }
        if self.menu.take().is_some() && e.detail == KEY_ESCAPE {
            return self.paint();
        }
        match e.detail {
            KEY_BACKSPACE => self.up()?,
            KEY_RETURN | KEY_KP_ENTER => {
                if let Some(i) = self.selected {
                    self.open(i)?;
                }
            }
            KEY_DELETE if e.state.contains(KeyButMask::SHIFT) => self.action(MenuAction::DeleteForGood)?,
            KEY_DELETE => self.action(MenuAction::Delete)?,
            KEY_F2 => self.action(MenuAction::Rename)?,
            KEY_F5 => self.action(MenuAction::Refresh)?,
            KEY_C if ctrl => self.action(MenuAction::Copy)?,
            KEY_X if ctrl => self.action(MenuAction::Cut)?,
            KEY_V if ctrl => self.action(MenuAction::Paste)?,
            _ if ctrl && matches!(self.keymap.typed(e), ferro_ui::Key::Char('+' | '=')) => self.set_icon_size(view::icon_size() + 1)?,
            _ if ctrl && matches!(self.keymap.typed(e), ferro_ui::Key::Char('-')) => {
                self.set_icon_size(view::icon_size().saturating_sub(1))?;
            }
            _ if ctrl && matches!(self.keymap.typed(e), ferro_ui::Key::Char('h' | 'H')) => {
                self.action(MenuAction::ToggleHidden)?;
            }
            KEY_A if ctrl => {
                self.selected_set.clear();
                for i in 0..self.entries.len() {
                    self.selected_set.insert(i);
                }
                if !self.entries.is_empty() {
                    self.selected = Some(0);
                }
                self.paint()?;
            }
            KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN if !self.entries.is_empty() => {
                let (n, cols) = (self.entries.len(), view::columns(self.w));
                self.selected = Some(match (self.selected, e.detail) {
                    (None, _) => 0,
                    (Some(i), KEY_LEFT) => i.saturating_sub(1),
                    (Some(i), KEY_RIGHT) => (i + 1).min(n - 1),
                    (Some(i), KEY_UP) => i.checked_sub(cols).unwrap_or(i),
                    (Some(i), _) => {
                        if i + cols < n {
                            i + cols
                        } else {
                            i
                        }
                    }
                });
                // What the keys reach is what's selected, as a click would
                // make it: the bar at the bottom tells of it.
                self.selected_set.clear();
                self.selected_set.extend(self.selected);
                self.status.clear();
                // Keep it in sight.
                let row = self.selected.unwrap_or(0) / cols;
                let most = view::max_scroll(self.w, self.h, n);
                let visible = view::visible_rows(self.h);
                if row < self.scroll {
                    self.scroll = row;
                } else if row >= self.scroll + visible {
                    self.scroll = (row + 1 - visible).min(most);
                }
                self.paint()?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// The pipe the work aside wakes the window through: (read end, write end).
#[cfg(unix)]
fn wake_pipe() -> Res<(i32, i32)> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 fills in.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(not(unix))]
fn wake_pipe() -> Res<(i32, i32)> {
    Ok((-1, -1))
}

/// Sleeps until the X server has something or the pipe is written to;
/// `true` (with the pipe emptied) for the pipe.
#[cfg(unix)]
fn wait_for(conn: &RustConnection, wake_read: i32, timeout_ms: i32) -> bool {
    use std::os::fd::AsRawFd;
    let mut fds = [
        libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: wake_read, events: libc::POLLIN, revents: 0 },
    ];
    // SAFETY: `fds` is two valid pollfd entries for the length given.
    if unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) } <= 0 || fds[1].revents == 0 {
        return false;
    }
    let mut buf = [0u8; 64];
    // SAFETY: reads into a live buffer of the size given; non-blocking.
    while unsafe { libc::read(wake_read, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    true
}

#[cfg(not(unix))]
fn wait_for(conn: &RustConnection, _: i32, _: i32) -> bool {
    let _ = conn;
    std::thread::sleep(std::time::Duration::from_millis(50));
    true
}

/// Whether this account may make things in `dir`.
#[allow(dead_code)]
fn writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut c = dir.as_os_str().as_bytes().to_vec();
        c.push(0);
        // SAFETY: `c` is a NUL-terminated path that lives through the call.
        unsafe { libc::access(c.as_ptr().cast(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        true
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-files: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    // `ferro-files --extract ARCHIVE`: unpacked beside itself, no window (the
    // desktop opens a zip this way).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, archive] = args.as_slice() {
        if flag == "--extract" {
            return ops::extract_here(Path::new(archive)).map(|_| ()).map_err(Into::into);
        }
    }
    // `ferro-files --delete PATH`: its folder, asking about it as Delete
    // does (the desktop's way to delete what's on a removable drive).
    let delete = match args.as_slice() {
        [flag, path] if flag == "--delete" => Some(PathBuf::from(path)),
        _ => None,
    };
    let (drives, detected) = drives::system_drives_with_detected();
    let raw_arg = match &delete {
        Some(p) => p.parent().map(|d| d.to_string_lossy().into_owned()),
        None => std::env::args().nth(1),
    };
    let open_my_computer = match &raw_arg {
        None => true,
        Some(a) => a == "My Computer" || a == "::",
    };
    // The view chosen last time (icon size, or Details), wherever it opens.
    view::set_icon_size(load_icon_size());
    let start = raw_arg.map(|a| if ferro_path::is_windows_path(&a) { drives.to_posix(&a).unwrap_or(a) } else { a });

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let win = conn.generate_id()?;
    let (w, h) = (520, 360);
    let events = EventMask::EXPOSURE
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::BUTTON_MOTION
        | EventMask::POINTER_MOTION
        | EventMask::KEY_PRESS
        | EventMask::STRUCTURE_NOTIFY;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        0,
        0,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        // White, as the files are drawn on: what a resize uncovers shows that
        // until it's drawn, not the frame's gray.
        &CreateWindowAux::new().background_pixel(screen.white_pixel).event_mask(events),
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
    let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);
    let (ferro_current_dir, ferro_refresh) = (atom("_FERRO_CURRENT_DIR")?, atom("_FERRO_REFRESH")?);
    let ferro_recycle_bin = atom("_FERRO_RECYCLE_BIN")?;
    let ferro_desktop_folders = atom("_FERRO_DESKTOP_FOLDERS")?;
    let (wake_read, wake) = wake_pipe()?;
    let keymap = ferro_ui::Keymap::load(&conn)?;
    let (report, reports) = channel();
    let (thumbs_tx, thumbs_rx) = channel();
    // The window manager's Close button asks us to close, instead of
    // cutting the connection.
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    // One class for every folder window, so the desktop remembers one size
    // for them all (whichever was resized last), never which folder it was.
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-files\0ferro-files\0")?;
    let mut window = Window {
        win,
        root: screen.root,
        gc,
        depth: screen.root_depth,
        net_wm_name,
        utf8_string,
        ferro_current_dir,
        ferro_refresh,
        ferro_recycle_bin,
        ferro_desktop_folders,
        ferro_place: atom("_FERRO_PLACE")?,
        screen: screen.clone(),
        drag_icon: None,
        w,
        h,
        drives,
        detected_drives: detected,
        is_my_computer: false,
        dir: PathBuf::new(),
        entries: Vec::new(),
        selected: None,
        selected_set: std::collections::HashSet::new(),
        rubberband: None,
        rubber_scroll: 0,
        renaming: None,
        rename_drag: false,
        rename_armed: None,
        sort: load_sort(),
        rename_soon: None,
        places: places(),
        address: None,
        show_hidden: load_show_hidden(),
        scroll: 0,
        last_click: (0, None),
        menu: None,
        thumbnails: std::collections::HashMap::new(),
        thumbs_dir: PathBuf::new(),
        thumbs_asked: std::collections::HashSet::new(),
        thumbs_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        thumbs_tx,
        thumbs_rx,
        keymap,
        clipboard: None,
        drag_start: None,
        dragging: None,
        drag_pos: None,
        drag_target: None,
        drag_scroll: None,
        want_paint: false,
        status: String::new(),
        prompt: None,
        wake,
        report,
        reports,
        busy: 0,
        conn,
    };
    window.conn.map_window(win)?;
    if open_my_computer {
        window.go_computer()?;
    } else {
        // A file (Open file location): its folder, with it selected.
        window.go_typed(&start.unwrap_or_else(|| "/".into()))?;
    }
    if let Some(path) = delete {
        window.recycle(vec![path])?;
    }

    // A resize sends a stream of size changes: drawn once they stop coming,
    // not once per step (the whole window goes to the X server each time).
    let mut repaint = false;
    loop {
        let Some(event) = window.conn.poll_for_event()? else {
            if std::mem::take(&mut repaint) | std::mem::take(&mut window.want_paint) {
                window.paint()?;
            }
            // Nothing from the server: sleep until it sends something, or
            // the work aside says it's done.
            window.conn.flush()?;
            let timeout = window.rename_soon.map(|(_, at)| at.saturating_duration_since(std::time::Instant::now()).as_millis() as i32);
            if wait_for(&window.conn, wake_read, timeout.unwrap_or(-1)) {
                window.finished()?;
            }
            if let Some((i, at)) = window.rename_soon {
                if std::time::Instant::now() >= at {
                    window.rename_soon = None;
                    if window.renaming.is_none() && window.selected == Some(i) && window.start_rename(i) {
                        window.paint()?;
                    }
                }
            }
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => repaint = true,
            Event::ConfigureNotify(e) => {
                let (nw, nh) = (i32::from(e.width), i32::from(e.height));
                if (nw, nh) != (window.w, window.h) {
                    (window.w, window.h) = (nw, nh);
                    window.scroll = window.scroll.min(view::max_scroll(nw, nh, window.entries.len()));
                    repaint = true;
                }
            }
            Event::ButtonPress(e) => window.click(&e)?,
            Event::ButtonRelease(e) => window.release(&e)?,
            Event::MotionNotify(e) => window.motion(&e)?,
            Event::KeyPress(e) => window.key(&e)?,
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ClientMessage(e) if e.type_ == ferro_refresh && !window.is_my_computer => {
                let d = window.dir.clone();
                window.go(d)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drops_find_the_desktop_folder_under_the_pointer() {
        let dir = std::env::temp_dir().join(format!("ferro-desk-{}", std::process::id()));
        let inside = dir.join("Two Words");
        std::fs::create_dir_all(&inside).unwrap();
        let list = format!("20 20 64 64 /nonexistent-ferro-folder\n100 200 64 64 {}\n", inside.display());
        assert_eq!(super::folder_at(&list, 120, 230), Some(inside.clone()));
        assert_eq!(super::folder_at(&list, 30, 30), None, "a folder that's gone");
        assert_eq!(super::folder_at(&list, 99, 230), None);
        assert_eq!(super::folder_at("garbage\n1 2 x 4 /tmp\n", 1, 2), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
