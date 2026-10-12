//! ferro-files: FerroOS Heavy's file manager. A folder as Windows 95 showed
//! it (the `view` module draws it with ferro-gfx), with paths the Windows
//! way: `C:\Users\you\Documents`, whatever Linux calls the place.
//!
//! Click to select, double-click (or Enter) to open, Up (or Backspace) for
//! the folder above, the wheel to scroll. Opening a folder goes into it;
//! opening a program starts it; any other file is shown in a terminal.
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod view;

use ferro_gfx::Surface;
use ferro_path::DriveTable;
use std::path::{Path, PathBuf};
use view::{Entry, Hit};
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

/// FerroOS Heavy shows `/home` as `C:\Users`.
fn system_drives() -> DriveTable {
    let heavy = std::fs::read_to_string("/etc/os-release").is_ok_and(|r| r.lines().any(|l| l == "ID=ferroos-heavy"));
    if heavy {
        DriveTable::heavy()
    } else {
        DriveTable::default()
    }
}

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
            Some(Entry { name: drives.shown_name(&dir_text, &real).to_owned(), real, is_dir })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Starts a program, detached from the file manager.
fn launch(program: &str, args: &[&str]) {
    if let Err(e) = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).spawn() {
        eprintln!("ferro-files: {program}: {e}");
    }
}

fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
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
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    scroll: usize,
    /// The last click: when, and on which entry.
    last_click: (u32, Option<usize>),
}

impl Window {
    /// The folder as people read it.
    fn shown_path(&self) -> String {
        self.drives.to_windows(&self.dir.to_string_lossy()).unwrap_or_else(|| self.dir.to_string_lossy().into_owned())
    }

    fn go(&mut self, dir: PathBuf) -> Res<()> {
        self.entries = list(&dir, &self.drives);
        self.dir = dir;
        self.selected = None;
        self.scroll = 0;
        let title = self.shown_path();
        self.conn.change_property8(PropMode::REPLACE, self.win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        self.conn.change_property8(PropMode::REPLACE, self.win, self.net_wm_name, self.utf8_string, title.as_bytes())?;
        self.paint()
    }

    fn up(&mut self) -> Res<()> {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(parent) => self.go(parent),
            None => Ok(()),
        }
    }

    fn open(&mut self, i: usize) -> Res<()> {
        let Some(e) = self.entries.get(i) else { return Ok(()) };
        let path = self.dir.join(&e.real);
        if e.is_dir {
            return self.go(path);
        }
        let text = path.to_string_lossy().into_owned();
        if is_program(&path) {
            launch(&text, &[]);
        } else {
            launch("x-terminal-emulator", &["-e", "less", &text]);
        }
        Ok(())
    }

    fn paint(&self) -> Res<()> {
        let (w, h) = (self.w, self.h);
        let mut s = Surface::new(w as usize, h as usize);
        view::draw(&mut s, w, h, &self.shown_path(), &self.entries, self.selected, self.scroll);
        let px = s.pixels();
        // In pieces, so no request outgrows what the server accepts.
        let rows_per = (48 * 1024 / (w as usize * 4)).max(1) as i32;
        let mut row = 0;
        while row < h {
            let n = rows_per.min(h - row);
            let mut data = Vec::with_capacity((w * n * 4) as usize);
            for p in &px[(row * w) as usize..((row + n) * w) as usize] {
                data.extend_from_slice(&p.to_le_bytes());
            }
            self.conn.put_image(ImageFormat::Z_PIXMAP, self.win, self.gc, w as u16, n as u16, 0, row as i16, 0, self.depth, &data)?;
            row += n;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn click(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let most = view::max_scroll(self.w, self.h, self.entries.len());
        match e.detail {
            // The wheel.
            4 => self.scroll = self.scroll.saturating_sub(1),
            5 => self.scroll = (self.scroll + 1).min(most),
            1 => match view::hit(self.w, self.h, self.entries.len(), self.scroll, i32::from(e.event_x), i32::from(e.event_y)) {
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
            _ => return Ok(()),
        }
        self.paint()
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
    let drives = system_drives();
    // The folder to show: a Windows-style or ordinary path, or your own.
    let start = std::env::args()
        .nth(1)
        .map(|a| if ferro_path::is_windows_path(&a) { drives.to_posix(&a).unwrap_or(a) } else { a })
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| "/".into());

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
        dir: PathBuf::new(),
        entries: Vec::new(),
        selected: None,
        scroll: 0,
        last_click: (0, None),
        conn,
    };
    window.conn.map_window(win)?;
    window.go(PathBuf::from(start))?;

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
