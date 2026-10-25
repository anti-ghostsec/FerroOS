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

use ferro_gfx::Surface;
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
const DOUBLE_CLICK_MS: u32 = 400;
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

/// What's in `dir`: folders first, then by name; hidden files left out.
fn list(dir: &Path, drives: &DriveTable) -> Vec<Entry> {
    let dir_text = dir.to_string_lossy();
    let mut out: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let real = e.file_name().to_string_lossy().into_owned();
            if real.starts_with('.') {
                return None;
            }
            let is_dir = e.path().is_dir();
            Some(Entry { name: drives.shown_name(&dir_text, &real).to_owned(), real, is_dir, icon: None })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Starts a program, detached from the file manager.
fn launch(program: &str, args: &[&str]) {
    launch_in_dir(program, args, None);
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
    Rename(PathBuf),
    /// Only telling: OK puts it away.
    Nothing,
}

fn is_program(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    // Shortcuts are run (or not) by what they say; and only programs put
    // here on purpose are started, never just anything with the
    // executable bit, as every file on a USB stick has.
    !ext.eq_ignore_ascii_case("desktop") && ferro_associations::trusted_launcher(path)
}

struct Window {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    net_wm_name: Atom,
    utf8_string: Atom,
    w: i32,
    h: i32,
    drives: DriveTable,
    detected_drives: Vec<drives::DetectedDrive>,
    is_my_computer: bool,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    scroll: usize,
    /// The last click: when, and on which entry.
    last_click: (u32, Option<usize>),
    menu: Option<ContextMenu>,
    keymap: ferro_ui::Keymap,
    /// What Copy or Cut put by: the path, and whether it was cut.
    clipboard: Option<(PathBuf, bool)>,
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
    /// The folder as people read it.
    fn shown_path(&self) -> String {
        if self.is_my_computer {
            "My Computer".to_string()
        } else {
            self.drives.to_windows(&self.dir.to_string_lossy()).unwrap_or_else(|| self.dir.to_string_lossy().into_owned())
        }
    }

    fn go_computer(&mut self) -> Res<()> {
        let (drives, detected) = drives::system_drives_with_detected();
        self.drives = drives;
        self.detected_drives = detected.clone();
        self.is_my_computer = true;
        self.dir = PathBuf::from("My Computer");
        self.selected = None;
        self.scroll = 0;
        self.menu = None;
        self.prompt = None;

        self.entries = detected
            .into_iter()
            .map(|d| Entry {
                name: format!("{} ({}:)", d.label, d.letter),
                real: d.path.to_string_lossy().into_owned(),
                is_dir: true,
                icon: Some(d.icon),
            })
            .collect();

        let title = "My Computer";
        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.net_wm_name, self.utf8_string, title.as_bytes())?;
        self.paint()
    }

    fn go(&mut self, dir: PathBuf) -> Res<()> {
        if dir == Path::new("My Computer") || dir.to_string_lossy() == "My Computer" {
            return self.go_computer();
        }
        self.is_my_computer = false;
        if let Err(e) = std::fs::read_dir(&dir) {
            return self.tell(ops::explain("open", &dir.to_string_lossy(), &e));
        }
        if dir != self.dir {
            self.status.clear();
            self.selected = None;
            self.scroll = 0;
        }
        self.entries = list(&dir, &self.drives);
        self.dir = dir;
        self.selected = self.selected.filter(|&i| i < self.entries.len());
        self.scroll = self.scroll.min(view::max_scroll(self.w, self.h, self.entries.len()));
        self.menu = None;
        self.prompt = None;
        let title = self.shown_path();
        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.net_wm_name, self.utf8_string, title.as_bytes())?;
        self.paint()
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
        if ops::is_archive(&e.real) {
            // Opened, an archive is unpacked beside itself (Windows opens a
            // zip as a folder; this ends the same: its files, in a folder).
            return self.extract(path);
        }
        let text = path.to_string_lossy().into_owned();
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
        view::draw(
            &mut s,
            w,
            h,
            &self.shown_path(),
            &self.entries,
            self.selected,
            self.scroll,
            self.menu.as_ref(),
            status,
            self.prompt.as_ref().map(|(p, _)| p),
        );
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, w, h)
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
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
        if let Some(menu) = self.menu.take() {
            if let Some(action) = view::menu_hit(&menu, px, py) {
                self.action(action)?;
            }
            return self.paint();
        }
        let most = view::max_scroll(self.w, self.h, self.entries.len());
        match e.detail {
            // The wheel.
            4 => self.scroll = self.scroll.saturating_sub(1),
            5 => self.scroll = (self.scroll + 1).min(most),
            1 => match view::hit(self.w, self.h, self.entries.len(), self.scroll, px, py) {
                Hit::Up => return self.up(),
                Hit::Entry(i) => {
                    let (before, which) = self.last_click;
                    self.last_click = (e.time, Some(i));
                    if which == Some(i) && e.time.wrapping_sub(before) < DOUBLE_CLICK_MS {
                        return self.open(i);
                    }
                    self.selected = Some(i);
                }
                Hit::Nothing => {
                    self.selected = None;
                    self.last_click = (e.time, None);
                }
            },
            3 => match view::hit(self.w, self.h, self.entries.len(), self.scroll, px, py) {
                Hit::Entry(i) => {
                    self.selected = Some(i);
                    let items = if self.is_my_computer {
                        vec![("Open", MenuAction::Open), ("Properties", MenuAction::Properties)]
                    } else {
                        let mut items = vec![("Open", MenuAction::Open)];
                        if self.entries.get(i).is_some_and(|e| ops::is_archive(&e.real)) {
                            items.push(("Extract Here", MenuAction::ExtractHere));
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
                    self.menu = Some(ContextMenu { x: px, y: py, items });
                }
                _ => {
                    self.selected = None;
                    let items = if self.is_my_computer {
                        vec![("Refresh", MenuAction::Refresh)]
                    } else {
                        let mut items = Vec::new();
                        if self.clipboard.is_some() {
                            items.push(("Paste", MenuAction::Paste));
                        }
                        items.extend([
                            ("New Folder", MenuAction::NewFolder),
                            ("New Document", MenuAction::NewDocument),
                            ("Open in Terminal", MenuAction::Terminal),
                            ("Refresh", MenuAction::Refresh),
                        ]);
                        items
                    };
                    self.menu = Some(ContextMenu { x: px, y: py, items });
                }
            },
            _ => return Ok(()),
        }
        self.paint()
    }

    fn action(&mut self, action: MenuAction) -> Res<()> {
        match action {
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
                            let link_name = format!("Shortcut to {}", e.real);
                            let _ = std::os::unix::fs::symlink(&e.real, self.dir.join(&link_name));
                        }
                        #[cfg(not(unix))]
                        let _ = &e;
                        self.go(self.dir.clone())?;
                    }
                }
            }
            MenuAction::ExtractHere => {
                if let Some(path) = self.selected_path() {
                    self.extract(path)?;
                }
            }
            MenuAction::Cut | MenuAction::Copy => {
                if let Some(path) = self.selected_path() {
                    let cut = action == MenuAction::Cut;
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.status = format!("'{name}' {}: right-click a folder's space and Paste (Ctrl+V).", if cut { "cut" } else { "copied" });
                    self.clipboard = Some((path, cut));
                    self.paint()?;
                }
            }
            MenuAction::Paste => {
                if let Some((from, cut)) = self.clipboard.clone() {
                    if self.is_my_computer {
                        return Ok(());
                    }
                    if cut {
                        // A cut thing goes once.
                        self.clipboard = None;
                    }
                    let dir = self.dir.clone();
                    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.status = format!("{} '{name}'...", if cut { "Moving" } else { "Copying" });
                    self.aside(move || ops::paste(&from, &dir, cut))?;
                }
            }
            MenuAction::Rename => {
                if let Some(path) = self.selected_path() {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let message = format!("Rename '{name}' to:");
                    self.prompt = Some((Prompt { message, input: Some(name), notice: false }, Asking::Rename(path)));
                    self.paint()?;
                }
            }
            MenuAction::Delete => {
                if let Some(path) = self.selected_path() {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let what = if path.is_dir() { "the folder" } else { "" };
                    let message = format!("Delete {what} '{name}' for good? It can't be got back.").replace("  ", " ");
                    self.prompt = Some((Prompt { message, input: None, notice: false }, Asking::Delete(path)));
                    self.paint()?;
                }
            }
            MenuAction::Properties => {
                if let Some(i) = self.selected {
                    if let Some(e) = self.entries.get(i) {
                        let path = if self.is_my_computer { PathBuf::from(&e.real) } else { self.dir.join(&e.real) };
                        self.status = match std::fs::metadata(&path) {
                            Ok(m) if m.is_dir() => {
                                let n = std::fs::read_dir(&path).map(|d| d.count()).unwrap_or(0);
                                let ro = if m.permissions().readonly() || !writable(&path) { ", read-only for you" } else { "" };
                                format!("{}: folder, {n} item(s){ro}", e.name)
                            }
                            Ok(m) => format!("{}: {}", e.name, ops::human_size(m.len())),
                            Err(err) => ops::explain("read", &e.name, &err),
                        };
                        self.paint()?;
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
                if let Err(e) = made {
                    self.tell(ops::explain("make", &name, &e))?;
                }
            }
            MenuAction::NewDocument => {
                let mut name = "New Document.txt".to_string();
                let mut n = 1;
                while self.dir.join(&name).exists() {
                    n += 1;
                    name = format!("New Document ({n}).txt");
                }
                let made = std::fs::write(self.dir.join(&name), "");
                self.go(self.dir.clone())?;
                if let Err(e) = made {
                    self.tell(ops::explain("make", &name, &e))?;
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

    /// Unpacks an archive beside itself, aside.
    fn extract(&mut self, path: PathBuf) -> Res<()> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.status = format!("Unpacking '{name}'...");
        self.aside(move || ops::extract_here(&path).map(|(said, _)| said))
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

    /// What the work aside said, now it's done.
    fn finished(&mut self) -> Res<()> {
        let mut failed = None;
        while let Ok(said) = self.reports.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match said {
                Ok(done) => self.status = done,
                Err(why) => failed = Some(why),
            }
        }
        if self.is_my_computer {
            self.paint()?;
        } else {
            self.go(self.dir.clone())?;
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
                    if self.clipboard.as_ref().is_some_and(|(p, _)| *p == path) {
                        self.clipboard = None;
                    }
                    return self.aside(move || ops::delete(&path));
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
        if let Some((prompt, _)) = &mut self.prompt {
            match (e.detail, self.keymap.typed(e)) {
                (KEY_RETURN | KEY_KP_ENTER, _) => return self.answer(true),
                (KEY_ESCAPE, _) => return self.answer(false),
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
            KEY_DELETE => self.action(MenuAction::Delete)?,
            KEY_F2 => self.action(MenuAction::Rename)?,
            KEY_F5 => self.action(MenuAction::Refresh)?,
            KEY_C if ctrl => self.action(MenuAction::Copy)?,
            KEY_X if ctrl => self.action(MenuAction::Cut)?,
            KEY_V if ctrl => self.action(MenuAction::Paste)?,
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
fn wait_for(conn: &RustConnection, wake_read: i32) -> bool {
    use std::os::fd::AsRawFd;
    let mut fds = [
        libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: wake_read, events: libc::POLLIN, revents: 0 },
    ];
    // SAFETY: `fds` is two valid pollfd entries for the length given.
    if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } <= 0 || fds[1].revents == 0 {
        return false;
    }
    let mut buf = [0u8; 64];
    // SAFETY: reads into a live buffer of the size given; non-blocking.
    while unsafe { libc::read(wake_read, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    true
}

#[cfg(not(unix))]
fn wait_for(conn: &RustConnection, _: i32) -> bool {
    let _ = conn;
    std::thread::sleep(std::time::Duration::from_millis(50));
    true
}

/// Whether this account may make things in `dir`.
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
    let (drives, detected) = drives::system_drives_with_detected();
    let raw_arg = std::env::args().nth(1);
    let open_my_computer = match &raw_arg {
        None => true,
        Some(a) => a == "My Computer" || a == "::",
    };
    let start = raw_arg.map(|a| if ferro_path::is_windows_path(&a) { drives.to_posix(&a).unwrap_or(a) } else { a });

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let win = conn.generate_id()?;
    let (w, h) = (520, 360);
    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;
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
        &CreateWindowAux::new().event_mask(events),
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
    let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);
    let (wake_read, wake) = wake_pipe()?;
    let keymap = ferro_ui::Keymap::load(&conn)?;
    let (report, reports) = channel();
    // The window manager's Close button asks us to close, instead of
    // cutting the connection.
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    let mut window = Window {
        win,
        gc,
        depth: screen.root_depth,
        net_wm_name,
        utf8_string,
        w,
        h,
        drives,
        detected_drives: detected,
        is_my_computer: false,
        dir: PathBuf::new(),
        entries: Vec::new(),
        selected: None,
        scroll: 0,
        last_click: (0, None),
        menu: None,
        keymap,
        clipboard: None,
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
        window.go(PathBuf::from(start.unwrap_or_else(|| "/".into())))?;
    }

    loop {
        let Some(event) = window.conn.poll_for_event()? else {
            // Nothing from the server: sleep until it sends something, or
            // the work aside says it's done.
            window.conn.flush()?;
            if wait_for(&window.conn, wake_read) {
                window.finished()?;
            }
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => window.paint()?,
            Event::ConfigureNotify(e) => {
                let (nw, nh) = (i32::from(e.width), i32::from(e.height));
                if (nw, nh) != (window.w, window.h) {
                    (window.w, window.h) = (nw, nh);
                    window.scroll = window.scroll.min(view::max_scroll(nw, nh, window.entries.len()));
                    window.paint()?;
                }
            }
            Event::ButtonPress(e) => window.click(&e)?,
            Event::KeyPress(e) => window.key(&e)?,
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => return Ok(()),
            _ => {}
        }
    }
}
