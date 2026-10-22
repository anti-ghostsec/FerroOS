//! ferro-files: FerroOS Heavy's file manager. A folder as Windows 95 showed
//! it (the `view` module draws it with ferro-gfx), with paths the Windows
//! way: `C:\Users\you\Documents`, whatever Linux calls the place.
//!
//! Click to select, double-click (or Enter) to open, Up (or Backspace) for
//! the folder above, the wheel to scroll. Opening a folder goes into it;
//! opening a program starts it; any other file is shown in a terminal.
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod drives;
mod view;

use ferro_gfx::Surface;
use ferro_path::DriveTable;
use std::path::{Path, PathBuf};
use view::{ContextMenu, Entry, Hit, MenuAction};
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
    if let Err(e) = cmd.spawn() {
        eprintln!("ferro-files: {program}: {e}");
    }
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
        self.entries = list(&dir, &self.drives);
        self.dir = dir;
        self.selected = None;
        self.scroll = 0;
        self.menu = None;
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
        view::draw(&mut s, w, h, &self.shown_path(), &self.entries, self.selected, self.scroll, self.menu.as_ref());
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, w, h)
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
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
                        vec![
                            ("Open", MenuAction::Open),
                            ("Create Shortcut", MenuAction::CreateShortcut),
                            ("Delete", MenuAction::Delete),
                            ("Properties", MenuAction::Properties),
                        ]
                    };
                    self.menu = Some(ContextMenu { x: px, y: py, items });
                }
                _ => {
                    self.selected = None;
                    let items = if self.is_my_computer {
                        vec![("Refresh", MenuAction::Refresh)]
                    } else {
                        vec![
                            ("New Folder", MenuAction::NewFolder),
                            ("New Document", MenuAction::NewDocument),
                            ("Open in Terminal", MenuAction::Terminal),
                            ("Refresh", MenuAction::Refresh),
                        ]
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
            MenuAction::Delete => {
                if let Some(i) = self.selected {
                    if let Some(e) = self.entries.get(i) {
                        let path = self.dir.join(&e.real);
                        if e.is_dir {
                            let _ = std::fs::remove_dir_all(path);
                        } else {
                            let _ = std::fs::remove_file(path);
                        }
                        self.go(self.dir.clone())?;
                    }
                }
            }
            MenuAction::Properties => {
                if let Some(i) = self.selected {
                    if let Some(e) = self.entries.get(i) {
                        let path = self.dir.join(&e.real);
                        if let Ok(m) = std::fs::metadata(path) {
                            eprintln!("ferro-files: {}: {} bytes", e.name, m.len());
                        }
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
                let _ = std::fs::create_dir(self.dir.join(&name));
                self.go(self.dir.clone())?;
            }
            MenuAction::NewDocument => {
                let mut name = "New Document.txt".to_string();
                let mut n = 1;
                while self.dir.join(&name).exists() {
                    n += 1;
                    name = format!("New Document ({n}).txt");
                }
                let _ = std::fs::write(self.dir.join(&name), "");
                self.go(self.dir.clone())?;
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
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-files: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    // Finished programs we started are forgotten, not kept as zombies.
    #[cfg(unix)]
    // SAFETY: installing SIG_IGN has no preconditions.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }
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
        conn,
    };
    window.conn.map_window(win)?;
    if open_my_computer {
        window.go_computer()?;
    } else {
        window.go(PathBuf::from(start.unwrap_or_else(|| "/".into())))?;
    }

    loop {
        match window.conn.wait_for_event()? {
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
            Event::KeyPress(e) => match e.detail {
                KEY_BACKSPACE => window.up()?,
                KEY_RETURN => {
                    if let Some(i) = window.selected {
                        window.open(i)?;
                    }
                }
                _ => {}
            },
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => return Ok(()),
            _ => {}
        }
    }
}
