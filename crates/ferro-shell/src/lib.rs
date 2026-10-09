//! The FerroOS desktop: a Windows 95 style shell rendered in software.
//!
//! The shell core is platform-independent. A backend (DRM/KMS on Linux in
//! `fbdev.rs`, or the desktop preview) feeds it [`Event`]s, calls
//! [`Shell::draw`] and copies the [`Surface`] to the screen.

pub mod keymap;
mod logon;
mod menu;
mod netpriv;
mod privacy;
mod setup;
mod taskmgr;
pub mod term;

use ferro_gfx::color::*;
use ferro_gfx::icons::{self, Icon};
use ferro_gfx::{ellipsize, text_width, wrap, Bevel, Rect, Surface};
use ferro_path::DriveTable;
use ferro_sandbox::store::Store;
use ferro_sys::{ProcInfo, IDLE_BUDGET_KB};

#[cfg(target_os = "linux")]
mod drm;
#[cfg(target_os = "linux")]
mod fbdev;
#[cfg(target_os = "linux")]
mod link;
#[cfg(target_os = "linux")]
mod pty;

/// Runs the desktop (`/bin/ferro-shell`).
#[cfg(target_os = "linux")]
pub fn main() {
    fbdev::run()
}

/// Runs the desktop (`/bin/ferro-shell`).
#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("ferro-shell drives a Linux framebuffer. On this host, run `cargo run -p ferro-preview`.");
    std::process::exit(1);
}

pub use logon::{SystemLink, VaultState};
use menu::ContextMenu;
pub use netpriv::NetStatus;
use privacy::BudgetInput;
pub use setup::DiskInfo;
use std::collections::VecDeque;
use std::path::Path;
use taskmgr::TaskState;
use term::Term;

/// Command Prompt character cell: 8x8 glyphs with 4px of leading.
const CELL_W: i32 = 8;
const CELL_H: i32 = 12;

/// Samples kept for the Task Manager graphs (one per second).
const HISTORY_LEN: usize = 60;

pub const TASKBAR_H: i32 = 28;
const TITLE_H: i32 = 18;
const DOUBLE_CLICK_MS: u64 = 500;
const MENU_W: i32 = 180;
const MENU_BANNER_W: i32 = 21;
const MENU_ITEM_H: i32 = 34;
const MENU_SEP_H: i32 = 8;
const ROW_H: i32 = 18;
const LIST_COL_W: i32 = 150;

/// Exit code ferro-shell uses to ask init to power off.
pub const EXIT_POWEROFF: i32 = 100;
/// Exit code ferro-shell uses to ask init to reboot.
pub const EXIT_REBOOT: i32 = 101;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseDown {
        x: i32,
        y: i32,
        button: MouseButton,
    },
    MouseUp {
        x: i32,
        y: i32,
        button: MouseButton,
    },
    /// A key press or autorepeat (releases are not reported).
    Key {
        key: Key,
        mods: Mods,
    },
}

/// A running command interpreter behind a Command Prompt window: a pty on
/// FerroOS, an in-process session in the desktop preview.
pub trait TermSession {
    /// Keyboard input for the program.
    fn write(&mut self, data: &[u8]);
    /// Appends any output produced since the last call; never blocks.
    fn read_into(&mut self, out: &mut Vec<u8>);
    fn resize(&mut self, cols: u16, rows: u16);
    fn is_alive(&mut self) -> bool;
}

/// Starts a new session with the given grid size (columns, rows) and an
/// optional first command (like `cmd /K`).
pub type TermSpawner = Box<dyn FnMut(u16, u16, Option<&str>) -> std::io::Result<Box<dyn TermSession>>>;

struct TermWindow {
    term: Term,
    session: Box<dyn TermSession>,
}

/// Requests the shell makes of its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PowerOff,
    Reboot,
    /// Send SIGTERM to this PID (from Task Manager's "End Process").
    EndProcess(u32),
    /// Give a process a RAM budget in bytes (`None` removes it).
    SetMemoryLimit {
        pid: u32,
        bytes: Option<u64>,
    },
    /// A tray kill switch changed; apply [`Shell::switches`] now.
    SwitchesChanged,
}

impl Action {
    /// The shell's exit code for actions that end the session.
    pub fn exit_code(self) -> Option<i32> {
        match self {
            Action::PowerOff => Some(EXIT_POWEROFF),
            Action::Reboot => Some(EXIT_REBOOT),
            Action::EndProcess(_) | Action::SetMemoryLimit { .. } | Action::SwitchesChanged => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    pub clock: String,
    pub mem_used_kb: Option<u64>,
    pub mem_total_kb: Option<u64>,
    pub kernel: Option<String>,
    pub cpu_percent: Option<f32>,
    pub uptime_secs: Option<u64>,
    /// Only filled in while [`Shell::wants_processes`] is true.
    pub processes: Option<Vec<ProcInfo>>,
    /// VPN and Tor state.
    pub net: NetStatus,
    /// Started from installation media: Setup is offered.
    pub install_media: bool,
    /// Setup's progress, as ferro-system reports it.
    pub setup_status: String,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    is_dir: bool,
}

enum Content {
    MyComputer,
    Explorer { path: String, entries: Vec<Entry>, error: Option<String> },
    Message { lines: Vec<String>, status: String },
    About,
    ShutDown { choice: usize },
    TaskManager(TaskState),
    Properties { icon: Icon, name: String, rows: Vec<(&'static str, String)> },
    Terminal(Box<TermWindow>),
    Input(Box<BudgetInput>),
    AppPermissions { selected: Option<usize> },
    NetworkPrivacy,
    Setup(Box<setup::SetupState>),
}

struct Window {
    id: u32,
    title: String,
    icon: Icon,
    rect: Rect,
    /// Pre-maximize geometry while maximized.
    restore: Option<Rect>,
    minimized: bool,
    content: Content,
    selected: Option<usize>,
}

impl Window {
    fn has_minmax(&self) -> bool {
        !matches!(self.content, Content::About | Content::ShutDown { .. } | Content::Properties { .. } | Content::Input(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Nothing,
    DesktopIcon(usize),
    StartButton,
    StartMenu,
    StartItem(usize),
    Taskbar,
    TaskButton(u32),
    Title(u32),
    Close(u32),
    Min(u32),
    Max(u32),
    Item(u32, usize),
    Client(u32),
    MenuItem(usize),
    MenuFrame,
    TrayIcon(usize),
}

const DESKTOP_ICONS: [(&str, Icon); 3] = [("My Computer", icons::COMPUTER), ("Recycle Bin", icons::RECYCLE), ("About FerroOS", icons::FERRO)];
/// Shown in a live session started from installation media.
const SETUP_ICON: (&str, Icon) = ("Install FerroOS", icons::DRIVE);

struct MenuItem {
    label: &'static str,
    icon: Icon,
    submenu: bool,
    enabled: bool,
}

const START_ITEMS: [MenuItem; 7] = [
    MenuItem { label: "Programs", icon: icons::FOLDER, submenu: true, enabled: true },
    MenuItem { label: "Documents", icon: icons::FILE, submenu: true, enabled: true },
    MenuItem { label: "Settings", icon: icons::COMPUTER, submenu: true, enabled: true },
    MenuItem { label: "Find", icon: icons::FILE, submenu: true, enabled: false },
    MenuItem { label: "Help", icon: icons::HELP, submenu: false, enabled: true },
    MenuItem { label: "Run...", icon: icons::FILE, submenu: false, enabled: false },
    MenuItem { label: "Shut Down...", icon: icons::COMPUTER, submenu: false, enabled: true },
];

pub struct Shell {
    w: i32,
    h: i32,
    drives: DriveTable,
    info: SystemInfo,
    /// Bottom to top.
    windows: Vec<Window>,
    next_id: u32,
    cursor: (i32, i32),
    start_open: bool,
    selected_icon: Option<usize>,
    pressed: Option<Hit>,
    drag: Option<(u32, i32, i32)>,
    last_click: Option<(Hit, u64)>,
    menu: Option<ContextMenu>,
    /// Fractions in 0..=1, oldest first.
    cpu_history: VecDeque<f32>,
    mem_history: VecDeque<f32>,
    spawner: Option<TermSpawner>,
    /// Remembered choices and kill-switch states.
    store: Store,
    /// The privileged side: vault, kill switches, RAM budgets.
    link: Option<Box<dyn SystemLink>>,
    /// The "Welcome to FerroOS" password dialog, shown before the desktop.
    logon: Option<logon::Logon>,
    /// What needs redrawing since the last [`Shell::take_damage`].
    damage: Damage,
}

/// What changed on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    None,
    /// Only these areas (e.g. where the cursor was and is).
    Rects(Vec<Rect>),
    Full,
}

impl Damage {
    fn add(&mut self, r: Rect) {
        match self {
            Damage::None => *self = Damage::Rects(vec![r]),
            Damage::Rects(v) => v.push(r),
            Damage::Full => {}
        }
    }
}

/// Area covered by the cursor sprite drawn at (x, y).
fn cursor_rect(x: i32, y: i32) -> Rect {
    Rect::new(x, y, 12, 19)
}

impl Shell {
    pub fn new(width: usize, height: usize, drives: DriveTable) -> Self {
        Self {
            w: width as i32,
            h: height as i32,
            drives,
            info: SystemInfo::default(),
            windows: Vec::new(),
            next_id: 1,
            cursor: (width as i32 / 2, height as i32 / 2),
            start_open: false,
            selected_icon: None,
            pressed: None,
            drag: None,
            last_click: None,
            menu: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            mem_history: VecDeque::with_capacity(HISTORY_LEN),
            spawner: None,
            store: Store::default(),
            link: None,
            logon: None,
            damage: Damage::Full,
        }
    }

    /// Lets the shell open Command Prompt windows.
    pub fn set_terminal_spawner(&mut self, spawner: TermSpawner) {
        self.spawner = Some(spawner);
    }

    pub fn open_terminal(&mut self) {
        self.open_terminal_with(None);
    }

    /// Opens a Command Prompt that first runs `command`.
    pub fn open_terminal_with(&mut self, command: Option<&str>) {
        let (cols, rows) = (80, 25);
        let Some(spawn) = self.spawner.as_mut() else {
            return self.show_error("Command Prompt", "This host has no terminal backend.");
        };
        match spawn(cols as u16, rows as u16, command) {
            Ok(session) => {
                let size = (cols * CELL_W + 8 + 8, rows * CELL_H + 8 + 8 + TITLE_H + 2);
                let content = Content::Terminal(Box::new(TermWindow { term: Term::new(cols as usize, rows as usize), session }));
                self.open("Command Prompt".into(), icons::TERMINAL, size, content);
            }
            Err(e) => self.show_error("Command Prompt", &format!("Cannot start ferro-cmd: {e}")),
        }
    }

    /// True while any Command Prompt is open; backends then poll often
    /// enough (~60 Hz) to keep typing responsive.
    pub fn has_terminals(&self) -> bool {
        self.windows.iter().any(|w| matches!(w.content, Content::Terminal(_)))
    }

    /// Pumps terminal sessions: output into the grids, query replies back,
    /// grid resizes after maximize, and closes windows whose program exited
    /// (like cmd.exe after EXIT). Returns true if anything needs a redraw.
    pub fn poll_terminals(&mut self) -> bool {
        let mut changed = false;
        let mut dead = Vec::new();
        let mut buf = Vec::new();
        for w in &mut self.windows {
            let Content::Terminal(t) = &mut w.content else { continue };
            let c = terminal_rect(w.rect);
            let size = ((c.w / CELL_W).max(1) as usize, (c.h / CELL_H).max(1) as usize);
            if size != t.term.size() {
                t.term.resize(size.0, size.1);
                t.session.resize(size.0 as u16, size.1 as u16);
                changed = true;
            }
            buf.clear();
            t.session.read_into(&mut buf);
            if !buf.is_empty() {
                t.term.feed(&buf);
                changed = true;
            }
            if !t.term.replies.is_empty() {
                let replies = std::mem::take(&mut t.term.replies);
                t.session.write(&replies);
            }
            if buf.is_empty() && !t.session.is_alive() {
                dead.push(w.id);
            }
        }
        for id in &dead {
            self.close(*id);
        }
        if changed || !dead.is_empty() {
            self.damage = Damage::Full;
        }
        changed || !dead.is_empty()
    }

    fn on_key(&mut self, key: Key, mods: Mods) -> Option<Action> {
        match (key, mods.ctrl, mods.alt, mods.shift) {
            (Key::Delete, true, true, _) | (Key::Escape, true, false, true) => {
                self.open_task_manager();
                return None;
            }
            (Key::Escape, true, false, false) => {
                self.menu = None;
                self.toggle_start_menu();
                return None;
            }
            (Key::F(4), false, true, _) => {
                if let Some(id) = self.active_id() {
                    self.close(id);
                }
                return None;
            }
            (Key::Escape, ..) if self.menu.is_some() || self.start_open => {
                self.menu = None;
                self.start_open = false;
                return None;
            }
            _ => {}
        }
        let id = self.active_id()?;
        match &mut self.window_mut(id)?.content {
            Content::Terminal(t) => t.session.write(&term::encode_key(key, mods)),
            Content::Input(_) => return self.input_key(id, key, mods),
            _ => {}
        }
        None
    }

    /// Call about once a second; each call adds one point to the graphs.
    pub fn set_info(&mut self, info: SystemInfo) {
        fn push(h: &mut VecDeque<f32>, v: f32) {
            if h.len() == HISTORY_LEN {
                h.pop_front();
            }
            h.push_back(v.clamp(0.0, 1.0));
        }
        self.damage = Damage::Full; // clock, tray and graphs change
        push(&mut self.cpu_history, info.cpu_percent.unwrap_or(0.0) / 100.0);
        if let (Some(used), Some(total)) = (info.mem_used_kb, info.mem_total_kb) {
            push(&mut self.mem_history, used as f32 / total.max(1) as f32);
        }
        self.info = info;
        let progress = std::mem::take(&mut self.info.setup_status);
        if !progress.is_empty() {
            self.setup_progress(&progress);
        }
        // Choices changed at the prompt (PERMS, VPN, TOR)? Pick them up, so
        // the next save here doesn't undo them.
        if let Ok(text) = std::fs::read_to_string(&self.store.path) {
            if text != self.store.to_text() {
                self.store = Store::load(self.store.path.clone());
            }
        }
    }

    /// True while a visible window needs the per-process list, so backends
    /// skip walking /proc the rest of the time.
    pub fn wants_processes(&self) -> bool {
        self.windows.iter().any(|w| !w.minimized && matches!(w.content, Content::TaskManager(_)))
    }

    // ---- opening windows -------------------------------------------------

    fn open(&mut self, title: String, icon: Icon, size: (i32, i32), content: Content) {
        let n = self.windows.len() as i32 % 8;
        let (w, h) = (size.0.min(self.w - 20), size.1.min(self.h - TASKBAR_H - 20));
        let rect = Rect::new((100 + n * 24).min(self.w - w), (40 + n * 24).min(self.h - TASKBAR_H - h).max(0), w, h);
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push(Window { id, title, icon, rect, restore: None, minimized: false, content, selected: None });
    }

    /// Focuses an existing window whose content matches, instead of opening a duplicate.
    fn focus_existing(&mut self, pred: impl Fn(&Content) -> bool) -> bool {
        match self.windows.iter().find(|w| pred(&w.content)).map(|w| w.id) {
            Some(id) => {
                self.focus(id);
                true
            }
            None => false,
        }
    }

    pub fn open_my_computer(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::MyComputer)) {
            self.open("My Computer".into(), icons::COMPUTER, (360, 240), Content::MyComputer);
        }
    }

    pub fn open_about(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::About)) {
            self.open("About FerroOS".into(), icons::FERRO, (380, 230), Content::About);
        }
    }

    pub fn open_recycle_bin(&mut self) {
        if let Some(id) = self.windows.iter().find(|w| w.title == "Recycle Bin").map(|w| w.id) {
            return self.focus(id);
        }
        let content = Content::Message { lines: vec!["The Recycle Bin is empty.".into()], status: "0 object(s)".into() };
        self.open("Recycle Bin".into(), icons::RECYCLE, (320, 200), content);
    }

    fn show_error(&mut self, title: &str, text: &str) {
        let lines = wrap(text, 40);
        let content = Content::Message { lines, status: String::new() };
        self.open(title.into(), icons::HELP, (360, 160), content);
    }

    /// A Properties sheet for a file, folder or drive given as a Windows path.
    fn open_properties(&mut self, win_path: &str) {
        let posix = match self.drives.to_posix(win_path) {
            Ok(p) => p,
            Err(e) => return self.show_error("Properties", &e.to_string()),
        };
        let md = std::fs::metadata(&posix);
        let parent = ferro_path::parent(win_path);
        let is_root = parent.is_none();
        let name = match &parent {
            None => format!("Local Disk ({})", &win_path[..2.min(win_path.len())]),
            Some(_) => win_path.trim_end_matches('\\').rsplit(['\\', '/']).next().unwrap_or(win_path).to_owned(),
        };
        let mut rows: Vec<(&'static str, String)> = Vec::new();
        let (icon, kind) = match &md {
            _ if is_root => (icons::DRIVE, "Local Disk".to_owned()),
            Ok(m) if m.is_dir() => (icons::FOLDER, "File Folder".to_owned()),
            _ => (icons::FILE, file_type(Path::new(&posix), &name)),
        };
        rows.push(("Type:", kind));
        if let Some(p) = parent {
            rows.push(("Location:", p));
        }
        rows.push((if is_root { "Mounted at:" } else { "POSIX path:" }, posix.clone()));
        match &md {
            Ok(m) => {
                if m.is_file() {
                    rows.push(("Size:", format!("{} bytes", group_digits(m.len()))));
                }
                let secs = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                if let Some(d) = secs {
                    rows.push(("Modified:", format!("{} UTC", ferro_sys::format_utc(d.as_secs()))));
                }
                if m.permissions().readonly() {
                    rows.push(("Attributes:", "Read-only".into()));
                }
            }
            Err(e) => rows.push(("Error:", e.to_string())),
        }
        let h = 130 + rows.len() as i32 * 16;
        let content = Content::Properties { icon, name: name.clone(), rows };
        self.open(format!("{name} Properties"), icon, (380, h), content);
    }

    pub fn open_explorer(&mut self, path: &str) {
        let (entries, error) = read_dir(&self.drives, path);
        let content = Content::Explorer { path: path.to_owned(), entries, error };
        self.open(path.to_owned(), icons::FOLDER, (480, 320), content);
    }

    pub fn open_shut_down(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::ShutDown { .. })) {
            let (w, h) = (340, 170);
            let id = self.next_id;
            self.open("Shut Down FerroOS".into(), icons::COMPUTER, (w, h), Content::ShutDown { choice: 0 });
            // Dialogs open centered.
            let (sw, sh) = (self.w, self.h - TASKBAR_H);
            if let Some(win) = self.window_mut(id) {
                win.rect.x = (sw - win.rect.w) / 2;
                win.rect.y = (sh - win.rect.h) / 2;
            }
        }
    }

    pub fn toggle_start_menu(&mut self) {
        self.start_open = !self.start_open;
    }

    pub fn move_cursor(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
    }

    // ---- window management ---------------------------------------------

    fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn window_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    fn focus(&mut self, id: u32) {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            let mut w = self.windows.remove(i);
            w.minimized = false;
            self.windows.push(w);
        }
    }

    fn active_id(&self) -> Option<u32> {
        self.windows.iter().rev().find(|w| !w.minimized).map(|w| w.id)
    }

    fn close(&mut self, id: u32) {
        self.windows.retain(|w| w.id != id);
    }

    fn toggle_maximize(&mut self, id: u32) {
        let full = Rect::new(0, 0, self.w, self.h - TASKBAR_H);
        if let Some(w) = self.window_mut(id) {
            match w.restore.take() {
                Some(r) => w.rect = r,
                None => {
                    w.restore = Some(w.rect);
                    w.rect = full;
                }
            }
        }
    }

    // ---- geometry (shared by drawing and hit-testing) --------------------

    fn start_button_rect(&self) -> Rect {
        Rect::new(2, self.h - TASKBAR_H + 4, 70, 22)
    }

    fn tray_text(&self) -> String {
        let text = match self.info.mem_used_kb {
            Some(kb) => format!("{} MB  {}", (kb + 512) / 1024, self.info.clock),
            None => self.info.clock.clone(),
        };
        match self.tunnel_label() {
            Some(t) => format!("{t}  {text}"),
            None => text,
        }
    }

    fn tray_rect(&self) -> Rect {
        let w = text_width(&self.tray_text()) + 14 + 3 * privacy::TRAY_ICON_W + 2;
        Rect::new(self.w - 2 - w, self.h - TASKBAR_H + 4, w, 22)
    }

    /// Taskbar buttons in creation order.
    fn task_buttons(&self) -> Vec<(u32, Rect)> {
        let mut ids: Vec<u32> = self.windows.iter().map(|w| w.id).collect();
        ids.sort_unstable();
        if ids.is_empty() {
            return Vec::new();
        }
        let x0 = self.start_button_rect().right() + 4;
        let avail = self.tray_rect().x - 4 - x0;
        let bw = (avail / ids.len() as i32 - 3).min(160);
        ids.into_iter().enumerate().map(|(i, id)| (id, Rect::new(x0 + i as i32 * (bw + 3), self.h - TASKBAR_H + 4, bw, 22))).collect()
    }

    fn start_menu_rect(&self) -> Rect {
        let h = 6 + START_ITEMS.len() as i32 * MENU_ITEM_H + MENU_SEP_H;
        Rect::new(0, self.h - TASKBAR_H - h + 2, MENU_W, h)
    }

    fn start_item_rect(&self, i: usize) -> Rect {
        let m = self.start_menu_rect();
        let sep = if i == START_ITEMS.len() - 1 { MENU_SEP_H } else { 0 };
        Rect::new(m.x + 3 + MENU_BANNER_W, m.y + 3 + i as i32 * MENU_ITEM_H + sep, m.w - 6 - MENU_BANNER_W, MENU_ITEM_H)
    }

    fn desktop_icon_rect(&self, i: usize) -> Rect {
        Rect::new(8, 8 + i as i32 * 76, 76, 70)
    }

    fn uses_pane(content: &Content) -> bool {
        matches!(content, Content::MyComputer | Content::Explorer { .. } | Content::Message { .. })
    }

    /// White list area of explorer-like windows (above the status bar).
    fn pane_rect(win: &Window) -> Rect {
        let c = client_rect(win.rect);
        Rect::new(c.x, c.y, c.w, c.h - 22)
    }

    fn status_rect(win: &Window) -> Rect {
        let c = client_rect(win.rect);
        Rect::new(c.x, c.bottom() - 20, c.w, 20)
    }

    /// Clickable items inside a window's client area.
    fn item_rects(&self, win: &Window) -> Vec<Rect> {
        let c = client_rect(win.rect);
        match &win.content {
            Content::MyComputer => {
                let p = Self::pane_rect(win).inset(2);
                let cols = ((p.w - 8) / 84).max(1);
                (0..self.drives.drives().count() as i32).map(|i| Rect::new(p.x + 6 + (i % cols) * 84, p.y + 6 + (i / cols) * 70, 80, 64)).collect()
            }
            Content::Explorer { entries, .. } => {
                let p = Self::pane_rect(win).inset(2);
                let rows = ((p.h - 4) / ROW_H).max(1);
                let cols = ((p.w - 4) / LIST_COL_W).max(1);
                (0..entries.len().min((rows * cols) as usize) as i32)
                    .map(|i| Rect::new(p.x + 2 + (i / rows) * LIST_COL_W, p.y + 2 + (i % rows) * ROW_H, LIST_COL_W - 4, ROW_H))
                    .collect()
            }
            Content::Message { .. } | Content::Terminal(_) => Vec::new(),
            Content::Input(_) => self.input_items(win),
            Content::AppPermissions { .. } => self.perm_items(win),
            Content::NetworkPrivacy => self.netpriv_items(win),
            Content::Setup(st) => self.setup_items(win, st),
            Content::About | Content::Properties { .. } => vec![Rect::new(c.right() - 82, c.bottom() - 31, 75, 23)],
            Content::TaskManager(st) => self.tm_items(win, st),
            Content::ShutDown { .. } => vec![
                Rect::new(c.x + 62, c.y + 38, c.w - 70, 16),
                Rect::new(c.x + 62, c.y + 58, c.w - 70, 16),
                Rect::new(c.x + c.w / 2 - 80, c.bottom() - 33, 75, 23),
                Rect::new(c.x + c.w / 2 + 5, c.bottom() - 33, 75, 23),
            ],
        }
    }

    // ---- hit testing -----------------------------------------------------

    fn hit(&self, x: i32, y: i32) -> Hit {
        if let Some(m) = &self.menu {
            if let Some(i) = m.item_at(x, y) {
                return Hit::MenuItem(i);
            }
            if m.rect.contains(x, y) {
                return Hit::MenuFrame;
            }
        }
        if self.start_open {
            for i in 0..START_ITEMS.len() {
                if self.start_item_rect(i).contains(x, y) {
                    return Hit::StartItem(i);
                }
            }
            if self.start_menu_rect().contains(x, y) {
                return Hit::StartMenu;
            }
        }
        if y >= self.h - TASKBAR_H {
            if self.start_button_rect().contains(x, y) {
                return Hit::StartButton;
            }
            if let Some(i) = self.tray_icon_rects().iter().position(|r| r.contains(x, y)) {
                return Hit::TrayIcon(i);
            }
            for (id, r) in self.task_buttons() {
                if r.contains(x, y) {
                    return Hit::TaskButton(id);
                }
            }
            return Hit::Taskbar;
        }
        for win in self.windows.iter().rev().filter(|w| !w.minimized) {
            let r = win.rect;
            if !r.contains(x, y) {
                continue;
            }
            let id = win.id;
            if close_rect(r).contains(x, y) {
                return Hit::Close(id);
            }
            if win.has_minmax() {
                if max_rect(r).contains(x, y) {
                    return Hit::Max(id);
                }
                if min_rect(r).contains(x, y) {
                    return Hit::Min(id);
                }
            }
            if title_rect(r).contains(x, y) {
                return Hit::Title(id);
            }
            if let Some(i) = self.item_rects(win).iter().position(|ir| ir.contains(x, y)) {
                return Hit::Item(id, i);
            }
            return Hit::Client(id);
        }
        for i in 0..self.desktop_icons().len() {
            if self.desktop_icon_rect(i).contains(x, y) {
                return Hit::DesktopIcon(i);
            }
        }
        Hit::Nothing
    }

    // ---- input -----------------------------------------------------------

    /// Applies one input event. `now_ms` is any monotonic clock, used for
    /// double-click detection.
    pub fn handle(&mut self, ev: Event, now_ms: u64) -> Option<Action> {
        let old = self.cursor;
        let cursor_only = matches!(ev, Event::MouseMove { .. })
            && self.logon.is_none()
            && self.drag.is_none()
            && self.pressed.is_none()
            && self.menu.is_none()
            && !self.start_open;
        let result = self.handle_inner(ev, now_ms);
        if cursor_only {
            // Nothing reacts to hover here: repaint just the cursor's trail.
            self.damage.add(cursor_rect(old.0, old.1));
            self.damage.add(cursor_rect(self.cursor.0, self.cursor.1));
        } else {
            self.damage = Damage::Full;
        }
        result
    }

    /// Takes what changed since the last call (backends redraw just that).
    pub fn take_damage(&mut self) -> Damage {
        std::mem::replace(&mut self.damage, Damage::None)
    }

    /// Marks the whole screen for redrawing.
    pub fn invalidate(&mut self) {
        self.damage = Damage::Full;
    }

    fn handle_inner(&mut self, ev: Event, now_ms: u64) -> Option<Action> {
        if self.logon.is_some() {
            return self.logon_event(ev);
        }
        match ev {
            Event::MouseMove { x, y } => {
                self.cursor = (x, y);
                if let Some((id, dx, dy)) = self.drag {
                    let (sw, sh) = (self.w, self.h);
                    if let Some(w) = self.window_mut(id) {
                        w.rect.x = (x - dx).clamp(-w.rect.w + 40, sw - 40);
                        w.rect.y = (y - dy).clamp(0, sh - TASKBAR_H - TITLE_H);
                    }
                }
                None
            }
            Event::MouseDown { x, y, button: MouseButton::Left } => {
                self.cursor = (x, y);
                let hit = self.hit(x, y);
                if !matches!(hit, Hit::MenuItem(_) | Hit::MenuFrame) {
                    self.menu = None;
                }
                self.pressed = Some(hit);
                let double = self.is_double_click(hit, now_ms);
                self.on_press(hit, double)
            }
            Event::MouseUp { x, y, button: MouseButton::Left } => {
                self.cursor = (x, y);
                self.drag = None;
                let hit = self.hit(x, y);
                if self.pressed.take() == Some(hit) {
                    self.on_click(hit)
                } else {
                    None
                }
            }
            Event::MouseDown { x, y, button: MouseButton::Right } => {
                self.cursor = (x, y);
                let hit = self.hit(x, y);
                if !matches!(hit, Hit::MenuItem(_) | Hit::MenuFrame) {
                    self.start_open = false;
                    self.menu = self.context_menu_for(hit, x, y);
                }
                None
            }
            Event::Key { key, mods } => self.on_key(key, mods),
            // Like Windows: press, drag onto an item, release to choose it.
            Event::MouseUp { x, y, button: MouseButton::Right } => {
                self.cursor = (x, y);
                match self.hit(x, y) {
                    Hit::MenuItem(i) => self.run_menu(i),
                    _ => None,
                }
            }
        }
    }

    fn is_double_click(&mut self, hit: Hit, now: u64) -> bool {
        let double = matches!(self.last_click, Some((h, t)) if h == hit && now.saturating_sub(t) <= DOUBLE_CLICK_MS);
        self.last_click = if double { None } else { Some((hit, now)) };
        double
    }

    fn on_press(&mut self, hit: Hit, double: bool) -> Option<Action> {
        if hit == Hit::StartButton {
            self.toggle_start_menu();
            return None;
        }
        if self.start_open && !matches!(hit, Hit::StartItem(_) | Hit::StartMenu | Hit::MenuItem(_) | Hit::MenuFrame) {
            self.start_open = false;
        }
        match hit {
            Hit::Nothing => self.selected_icon = None,
            Hit::DesktopIcon(i) => {
                self.selected_icon = Some(i);
                if double {
                    match i {
                        0 => self.open_my_computer(),
                        1 => self.open_recycle_bin(),
                        2 => self.open_about(),
                        _ => self.open_setup(),
                    }
                }
            }
            Hit::TaskButton(id) => {
                let minimize = self.active_id() == Some(id);
                if minimize {
                    if let Some(w) = self.window_mut(id) {
                        w.minimized = true;
                    }
                } else {
                    self.focus(id);
                }
            }
            Hit::Title(id) => {
                self.focus(id);
                let can_max = self.window(id).is_some_and(|w| w.has_minmax());
                if double && can_max {
                    self.toggle_maximize(id);
                } else if let Some(w) = self.window(id).filter(|w| w.restore.is_none()) {
                    self.drag = Some((id, self.cursor.0 - w.rect.x, self.cursor.1 - w.rect.y));
                }
            }
            Hit::Close(id) | Hit::Min(id) | Hit::Max(id) => self.focus(id),
            Hit::Client(id) => {
                self.focus(id);
                if let Some(w) = self.window_mut(id) {
                    w.selected = None;
                }
            }
            Hit::Item(id, i) => {
                self.focus(id);
                match self.window(id).map(|w| &w.content) {
                    Some(Content::TaskManager(_)) => {
                        self.tm_press(id, i);
                        return None;
                    }
                    Some(Content::Input(_)) => {
                        self.input_press(id, i);
                        return None;
                    }
                    Some(Content::AppPermissions { .. }) => {
                        self.perm_press(id, i);
                        return None;
                    }
                    _ => {}
                }
                if let Some(w) = self.window_mut(id) {
                    match &mut w.content {
                        Content::ShutDown { choice } if i < 2 => *choice = i,
                        Content::MyComputer | Content::Explorer { .. } => w.selected = Some(i),
                        _ => {}
                    }
                }
                if double {
                    self.activate_item(id, i);
                }
            }
            Hit::TrayIcon(i) => return self.toggle_switch(i),
            Hit::StartButton | Hit::StartMenu | Hit::StartItem(_) | Hit::Taskbar => {}
            Hit::MenuItem(_) | Hit::MenuFrame => {}
        }
        None
    }

    fn on_click(&mut self, hit: Hit) -> Option<Action> {
        match hit {
            Hit::Close(id) => self.close(id),
            Hit::Min(id) => {
                if let Some(w) = self.window_mut(id) {
                    w.minimized = true;
                }
            }
            Hit::Max(id) => self.toggle_maximize(id),
            Hit::StartItem(i @ (0 | 2)) => {
                // Programs / Settings: a flyout beside the item; Start stays open.
                let r = self.start_item_rect(i);
                self.menu = Some(if i == 0 { self.programs_menu(r.right() - 2, r.y) } else { self.settings_menu(r.right() - 2, r.y) });
            }
            Hit::StartItem(i) if START_ITEMS[i].enabled => {
                self.start_open = false;
                match i {
                    1 => self.open_explorer(r"C:\"),
                    2 => self.open_my_computer(),
                    4 => self.open_about(),
                    6 => self.open_shut_down(),
                    _ => {}
                }
            }
            Hit::MenuItem(i) => return self.run_menu(i),
            Hit::Item(id, i) => {
                enum Kind {
                    TaskManager,
                    Input,
                    Permissions,
                    NetPriv,
                    Setup,
                    Dialog,
                    ShutDown(usize),
                    Other,
                }
                let kind = self.window(id).map_or(Kind::Other, |w| match &w.content {
                    Content::TaskManager(_) => Kind::TaskManager,
                    Content::Input(_) => Kind::Input,
                    Content::AppPermissions { .. } => Kind::Permissions,
                    Content::NetworkPrivacy => Kind::NetPriv,
                    Content::Setup(_) => Kind::Setup,
                    Content::About | Content::Properties { .. } => Kind::Dialog,
                    Content::ShutDown { choice } => Kind::ShutDown(*choice),
                    _ => Kind::Other,
                });
                match (kind, i) {
                    (Kind::TaskManager, i) => return self.tm_click(id, i),
                    (Kind::Input, i) => return self.input_click(id, i),
                    (Kind::Permissions, i) => self.perm_click(id, i),
                    (Kind::NetPriv, i) => return self.netpriv_click(id, i),
                    (Kind::Setup, i) => return self.setup_click(id, i),
                    (Kind::Dialog, 0) => self.close(id),
                    (Kind::ShutDown(choice), 2) => {
                        return Some(if choice == 0 { Action::PowerOff } else { Action::Reboot });
                    }
                    (Kind::ShutDown(_), 3) => self.close(id),
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// Double-click on a drive or folder entry.
    fn activate_item(&mut self, id: u32, i: usize) {
        let drives = self.drives.clone();
        let Some(w) = self.window_mut(id) else { return };
        match &mut w.content {
            Content::MyComputer => {
                if let Some((letter, _)) = drives.drives().nth(i) {
                    self.open_explorer(&format!("{letter}:\\"));
                }
            }
            Content::Explorer { path, entries, error } => {
                let Some(entry) = entries.get(i) else { return };
                if !entry.is_dir {
                    let file = ferro_path::join(path, &entry.name);
                    return self.open_file(&file);
                }
                let target =
                    if entry.name == ".." { ferro_path::parent(path).unwrap_or_else(|| path.clone()) } else { ferro_path::join(path, &entry.name) };
                (*entries, *error) = read_dir(&drives, &target);
                *path = target.clone();
                w.title = target;
                w.selected = None;
            }
            _ => {}
        }
    }

    /// File associations, by content rather than extension: Linux programs
    /// run sandboxed in a Command Prompt; everything else explains itself.
    fn open_file(&mut self, win_path: &str) {
        let Ok(posix) = self.drives.to_posix(win_path) else { return };
        match sniff(Path::new(&posix)) {
            Kind::Elf | Kind::Script => self.open_terminal_with(Some(&quote_arg(win_path))),
            Kind::Pe => {
                self.show_error("Windows Program", "This is a Windows program. Running it needs the Wine runtime, which isn't installed yet.")
            }
            Kind::Other => {
                let name = win_path.rsplit('\\').next().unwrap_or(win_path);
                self.show_error("Open With", &format!("No program is associated with {name}. Use Properties to see what it is."));
            }
        }
    }

    /// "Remove Metadata": strips photo metadata in place (written to a
    /// temporary file, then renamed over the original).
    fn remove_personal_info(&mut self, win_path: &str) {
        let name = win_path.rsplit('\\').next().unwrap_or(win_path).to_owned();
        let result = self.drives.to_posix(win_path).map_err(|e| e.to_string()).and_then(|posix| {
            let data = std::fs::read(&posix).map_err(|e| e.to_string())?;
            let s = ferro_meta::strip(&data).map_err(|e| e.to_string())?;
            if !s.removed.is_empty() {
                let tmp = format!("{posix}.ferro-tmp");
                std::fs::write(&tmp, &s.data).and_then(|()| std::fs::rename(&tmp, &posix)).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    e.to_string()
                })?;
            }
            Ok(s)
        });
        let (title, lines) = match result {
            Ok(s) if s.removed.is_empty() => ("Remove Metadata", vec![format!("{name} has no metadata to remove.")]),
            Ok(s) => {
                let mut lines = vec![format!("Removed from {name}:"), String::new()];
                for r in &s.removed {
                    lines.push(format!("  {} ({})", r.kind, human_bytes(r.bytes)));
                }
                lines.push(String::new());
                lines.push("Image pixels were not changed.".into());
                ("Remove Metadata", lines)
            }
            Err(e) => ("Remove Metadata", vec![format!("Couldn't clean {name}:"), e]),
        };
        let lines: Vec<String> = lines.iter().flat_map(|l| if l.is_empty() { vec![String::new()] } else { wrap(l, 52) }).collect();
        let h = 90 + lines.len() as i32 * 14;
        let content = Content::Message { lines, status: String::new() };
        self.open(title.into(), icons::FILE, (460, h), content);
    }

    // ---- drawing ---------------------------------------------------------

    pub fn draw(&self, s: &mut Surface) {
        if let Some(l) = &self.logon {
            return self.draw_logon(s, l);
        }
        s.clear(DESKTOP);
        self.draw_desktop_icons(s);
        let active = self.active_id();
        for w in self.windows.iter().filter(|w| !w.minimized) {
            self.draw_window(s, w, Some(w.id) == active);
        }
        self.draw_taskbar(s);
        if self.start_open {
            self.draw_start_menu(s);
        }
        if let Some(m) = &self.menu {
            self.draw_context_menu(s, m);
        }
        s.sprite(self.cursor.0, self.cursor.1, icons::CURSOR, 1);
    }

    /// True while the button under `hit` is held and the cursor is still on it.
    fn is_held(&self, hit: Hit, r: Rect) -> bool {
        self.pressed == Some(hit) && r.contains(self.cursor.0, self.cursor.1)
    }

    fn desktop_icons(&self) -> Vec<(&'static str, Icon)> {
        let mut v = DESKTOP_ICONS.to_vec();
        if self.info.install_media {
            v.push(SETUP_ICON);
        }
        v
    }

    fn draw_desktop_icons(&self, s: &mut Surface) {
        for (i, (label, icon)) in self.desktop_icons().iter().enumerate() {
            let r = self.desktop_icon_rect(i);
            s.sprite(r.x + (r.w - 32) / 2, r.y + 2, icon, 2);
            let selected = self.selected_icon == Some(i);
            draw_label(s, r.x + r.w / 2, r.y + 38, label, 9, selected, WHITE);
        }
    }

    fn draw_window(&self, s: &mut Surface, win: &Window, active: bool) {
        let r = win.rect;
        s.fill(r, FACE);
        s.bevel(r, Bevel::Window);

        let t = title_rect(r);
        s.fill(t, if active { NAVY } else { SHADOW });
        s.sprite(t.x + 1, t.y + 1, win.icon, 1);
        let buttons_w = if win.has_minmax() { 56 } else { 20 };
        let max_chars = ((t.w - 22 - buttons_w) / 8).max(0) as usize;
        s.text_bold(t.x + 20, t.y + 5, &ellipsize(&win.title, max_chars), if active { WHITE } else { FACE });

        let close = close_rect(r);
        self.draw_caption_button(s, close, Hit::Close(win.id), icons::GLYPH_CLOSE);
        if win.has_minmax() {
            let max_glyph = if win.restore.is_some() { icons::GLYPH_RESTORE } else { icons::GLYPH_MAX };
            self.draw_caption_button(s, max_rect(r), Hit::Max(win.id), max_glyph);
            self.draw_caption_button(s, min_rect(r), Hit::Min(win.id), icons::GLYPH_MIN);
        }

        if Self::uses_pane(&win.content) {
            let pane = Self::pane_rect(win);
            s.fill(pane, WHITE);
            s.bevel(pane, Bevel::Sunken);
            let status = Self::status_rect(win);
            s.bevel(status, Bevel::Field);
            let text = self.status_text(win);
            s.text(status.x + 4, status.y + 6, &ellipsize(&text, ((status.w - 8) / 8) as usize), BLACK);
        }

        let items = self.item_rects(win);
        let c = client_rect(r);
        match &win.content {
            Content::MyComputer => {
                for (i, ((letter, _), ir)) in self.drives.drives().zip(&items).enumerate() {
                    let selected = win.selected == Some(i);
                    s.sprite(ir.x + (ir.w - 32) / 2, ir.y + 2, icons::DRIVE, 2);
                    draw_label(s, ir.x + ir.w / 2, ir.y + 38, &format!("({letter}:)"), 10, selected, BLACK);
                }
            }
            Content::Explorer { entries, error, .. } => {
                if let Some(e) = error {
                    let p = Self::pane_rect(win).inset(8);
                    for (i, line) in wrap(e, (p.w / 8).max(1) as usize).iter().enumerate() {
                        s.text(p.x, p.y + i as i32 * 12, line, BLACK);
                    }
                }
                for (i, (entry, ir)) in entries.iter().zip(&items).enumerate() {
                    let icon = if entry.is_dir { icons::FOLDER } else { icons::FILE };
                    s.sprite(ir.x + 1, ir.y + 1, icon, 1);
                    let name = ellipsize(&entry.name, ((ir.w - 22) / 8) as usize);
                    let tx = ir.x + 20;
                    if win.selected == Some(i) {
                        s.fill(Rect::new(tx - 1, ir.y + 3, text_width(&name) + 2, 12), NAVY);
                        s.text(tx, ir.y + 5, &name, WHITE);
                    } else {
                        s.text(tx, ir.y + 5, &name, BLACK);
                    }
                }
            }
            Content::Message { lines, .. } => {
                let p = Self::pane_rect(win).inset(10);
                for (i, line) in lines.iter().enumerate() {
                    s.text(p.x, p.y + i as i32 * 14, line, BLACK);
                }
            }
            Content::About => self.draw_about(s, c, win, &items),
            Content::TaskManager(st) => self.draw_task_manager(s, win, st, &items),
            Content::Terminal(t) => draw_terminal(s, terminal_rect(r), &t.term, active),
            Content::Input(input) => self.draw_input(s, win, input, &items),
            Content::AppPermissions { selected } => self.draw_permissions(s, win, *selected, &items),
            Content::NetworkPrivacy => self.draw_network_privacy(s, win, &items),
            Content::Setup(st) => self.draw_setup(s, win, st, &items),
            Content::Properties { icon, name, rows } => {
                s.sprite(c.x + 12, c.y + 10, icon, 2);
                s.text_bold(c.x + 62, c.y + 22, &ellipsize(name, ((c.w - 74) / 8) as usize), BLACK);
                s.etched_hline(c.x + 8, c.y + 52, c.w - 16);
                for (k, (label, value)) in rows.iter().enumerate() {
                    let y = c.y + 64 + k as i32 * 16;
                    s.text(c.x + 12, y, label, BLACK);
                    s.text(c.x + 116, y, &ellipsize(value, ((c.w - 124) / 8) as usize), BLACK);
                }
                self.draw_push_button(s, items[0], Hit::Item(win.id, 0), "OK");
            }
            Content::ShutDown { choice } => {
                s.sprite(c.x + 12, c.y + 10, icons::COMPUTER, 2);
                s.text(c.x + 62, c.y + 16, "Are you sure you want to:", BLACK);
                for (i, label) in ["Shut down the computer?", "Restart the computer?"].iter().enumerate() {
                    let ir = items[i];
                    s.sprite(ir.x, ir.y + 2, icons::RADIO, 1);
                    if *choice == i {
                        s.fill(Rect::new(ir.x + 4, ir.y + 6, 4, 4), BLACK);
                    }
                    s.text(ir.x + 18, ir.y + 4, label, BLACK);
                }
                self.draw_push_button(s, items[2], Hit::Item(win.id, 2), "Yes");
                self.draw_push_button(s, items[3], Hit::Item(win.id, 3), "No");
            }
        }
    }

    fn draw_about(&self, s: &mut Surface, c: Rect, win: &Window, items: &[Rect]) {
        s.sprite(c.x + 12, c.y + 12, icons::FERRO, 2);
        let x = c.x + 62;
        let kernel = self.info.kernel.as_deref().unwrap_or("n/a (preview)");
        let lines = [format!("FerroOS {}", env!("CARGO_PKG_VERSION")), "Rust userspace, Linux kernel".to_owned(), format!("Kernel: {kernel}")];
        for (i, line) in lines.iter().enumerate() {
            s.text(x, c.y + 14 + i as i32 * 14, line, BLACK);
        }
        let y = c.y + 70;
        let budget_mb = IDLE_BUDGET_KB / 1024;
        match self.info.mem_used_kb {
            Some(used) => {
                let total = self.info.mem_total_kb.map_or(String::new(), |t| format!(" of {}", t / 1024));
                s.text(x, y, &format!("Memory: {} MB{total} in use", used / 1024), BLACK);
                s.text(x, y + 14, &format!("Idle budget: {budget_mb} MB"), BLACK);
                // Win95-style segmented progress bar against the idle budget.
                let bar = Rect::new(x, y + 30, c.right() - x - 12, 18);
                s.fill(bar, WHITE);
                s.bevel(bar, Bevel::Sunken);
                let inner = bar.inset(3);
                let frac = (used as f64 / IDLE_BUDGET_KB as f64).min(1.0);
                let fill_w = (inner.w as f64 * frac) as i32;
                let col = if used > IDLE_BUDGET_KB { RED } else { NAVY };
                let mut bx = inner.x;
                while bx + 8 <= inner.x + fill_w {
                    s.fill(Rect::new(bx, inner.y, 8, inner.h), col);
                    bx += 10;
                }
            }
            None => {
                s.text(x, y, "Memory: n/a on this host", BLACK);
                s.text(x, y + 14, &format!("Idle budget: {budget_mb} MB"), BLACK);
            }
        }
        self.draw_push_button(s, items[0], Hit::Item(win.id, 0), "OK");
    }

    fn status_text(&self, win: &Window) -> String {
        match &win.content {
            Content::MyComputer => {
                let roots: Vec<String> = self.drives.drives().map(|(l, r)| format!("{l}: = {r}")).collect();
                roots.join("  ")
            }
            Content::Explorer { entries, error, .. } => {
                if error.is_some() {
                    "0 object(s)".into()
                } else {
                    let n = entries.iter().filter(|e| e.name != "..").count();
                    let shown = self.item_rects(win).len();
                    if shown < entries.len() {
                        format!("{n} object(s), {} not shown", entries.len() - shown)
                    } else {
                        format!("{n} object(s)")
                    }
                }
            }
            Content::Message { status, .. } => status.clone(),
            _ => String::new(),
        }
    }

    fn draw_caption_button(&self, s: &mut Surface, r: Rect, hit: Hit, glyph: Icon) {
        let held = self.is_held(hit, r);
        s.button(r, held);
        let (gw, gh) = (glyph[0].len() as i32, glyph.len() as i32);
        let off = held as i32;
        s.sprite_mono(r.x + (r.w - gw) / 2 + off, r.y + (r.h - gh) / 2 + off, glyph, BLACK);
    }

    fn draw_push_button(&self, s: &mut Surface, r: Rect, hit: Hit, label: &str) {
        let held = self.is_held(hit, r);
        s.button(r, held);
        let off = held as i32;
        s.text(r.x + (r.w - text_width(label)) / 2 + off, r.y + 8 + off, label, BLACK);
    }

    fn draw_taskbar(&self, s: &mut Surface) {
        let y = self.h - TASKBAR_H;
        s.fill(Rect::new(0, y, self.w, TASKBAR_H), FACE);
        s.hline(0, y, self.w, LIGHT);
        s.hline(0, y + 1, self.w, WHITE);

        let sb = self.start_button_rect();
        s.button(sb, self.start_open);
        let off = self.start_open as i32;
        s.sprite(sb.x + 4 + off, sb.y + 3 + off, icons::FERRO, 1);
        s.text_bold(sb.x + 22 + off, sb.y + 8 + off, "Start", BLACK);

        let active = self.active_id();
        for (id, r) in self.task_buttons() {
            let Some(win) = self.window(id) else { continue };
            let on = Some(id) == active;
            s.button(r, on);
            if on {
                // Win95 dithers the face of the active task button.
                for py in r.y + 2..r.bottom() - 2 {
                    for px in r.x + 2..r.right() - 2 {
                        if (px + py) % 2 == 0 {
                            s.put(px, py, WHITE);
                        }
                    }
                }
            }
            s.sprite(r.x + 4 + on as i32, r.y + 3 + on as i32, win.icon, 1);
            let label = ellipsize(&win.title, ((r.w - 26) / 8).max(0) as usize);
            if on {
                s.text_bold(r.x + 22 + 1, r.y + 8 + 1, &label, BLACK);
            } else {
                s.text(r.x + 22, r.y + 8, &label, BLACK);
            }
        }

        let tray = self.tray_rect();
        s.bevel(tray, Bevel::Field);
        self.draw_tray_icons(s);
        s.text(tray.x + 7 + 3 * privacy::TRAY_ICON_W, tray.y + 8, &self.tray_text(), BLACK);
    }

    fn draw_start_menu(&self, s: &mut Surface) {
        let m = self.start_menu_rect();
        s.fill(m, FACE);
        s.bevel(m, Bevel::Window);
        let banner = Rect::new(m.x + 3, m.y + 3, MENU_BANNER_W, m.h - 6);
        s.fill(banner, SHADOW);
        let bottom = banner.bottom() - 6;
        let used = s.text_vertical(banner.x + 7, bottom, "Ferro", WHITE);
        s.text_vertical(banner.x + 8, bottom, "Ferro", WHITE);
        s.text_vertical(banner.x + 7, bottom - used, "OS", FACE);

        let (cx, cy) = self.cursor;
        for (i, item) in START_ITEMS.iter().enumerate() {
            let r = self.start_item_rect(i);
            if i == START_ITEMS.len() - 1 {
                let sy = r.y - MENU_SEP_H / 2 - 1;
                s.hline(r.x + 2, sy, r.w - 4, SHADOW);
                s.hline(r.x + 2, sy + 1, r.w - 4, WHITE);
            }
            let hover = item.enabled && r.contains(cx, cy);
            if hover {
                s.fill(r, NAVY);
            }
            s.sprite(r.x + 3, r.y + 1, item.icon, 2);
            let ty = r.y + 13;
            match (item.enabled, hover) {
                (false, _) => {
                    s.text_disabled(r.x + 44, ty, item.label);
                }
                (true, true) => {
                    s.text(r.x + 44, ty, item.label, WHITE);
                }
                (true, false) => {
                    s.text(r.x + 44, ty, item.label, BLACK);
                }
            }
            if item.submenu {
                let col = if hover {
                    WHITE
                } else if item.enabled {
                    BLACK
                } else {
                    SHADOW
                };
                s.sprite_mono(r.right() - 12, r.y + 13, icons::GLYPH_SUBMENU, col);
            }
        }
    }
}

fn title_rect(r: Rect) -> Rect {
    Rect::new(r.x + 3, r.y + 3, r.w - 6, TITLE_H)
}

fn close_rect(r: Rect) -> Rect {
    Rect::new(r.right() - 5 - 16, r.y + 5, 16, 14)
}

fn max_rect(r: Rect) -> Rect {
    let c = close_rect(r);
    Rect::new(c.x - 2 - 16, c.y, 16, 14)
}

fn min_rect(r: Rect) -> Rect {
    let m = max_rect(r);
    Rect::new(m.x - 16, m.y, 16, 14)
}

fn client_rect(r: Rect) -> Rect {
    Rect::new(r.x + 4, r.y + 4 + TITLE_H + 2, r.w - 8, r.h - 8 - TITLE_H - 2)
}

/// The character grid area of a Command Prompt window.
fn terminal_rect(r: Rect) -> Rect {
    client_rect(r).inset(4)
}

/// The Command Prompt's character grid: green-on-black by default, with a
/// DOS-style underline cursor while the window is focused.
fn draw_terminal(s: &mut Surface, area: Rect, term: &Term, focused: bool) {
    let frame = area.inset(-4);
    s.fill(frame, term::DEFAULT_BG);
    s.bevel(frame, Bevel::Sunken);
    let (cols, rows) = term.size();
    for y in 0..rows {
        let py = area.y + y as i32 * CELL_H;
        // Run-length fill backgrounds; most rows are one color.
        let mut x = 0;
        while x < cols {
            let bg = term.cell(x, y).bg();
            let start = x;
            while x < cols && term.cell(x, y).bg() == bg {
                x += 1;
            }
            if bg != term::DEFAULT_BG {
                let w = (x - start) as i32 * CELL_W;
                s.fill(Rect::new(area.x + start as i32 * CELL_W, py, w, CELL_H), bg);
            }
        }
        for x in 0..cols {
            let cell = term.cell(x, y);
            if cell.ch != b' ' {
                let ch = [cell.ch];
                let text = std::str::from_utf8(&ch).unwrap_or("?");
                s.text(area.x + x as i32 * CELL_W, py + 2, text, cell.fg());
            }
        }
    }
    let (cx, cy) = term.cursor();
    if focused && term.cursor_visible && cx < cols && cy < rows {
        let color = term.cell(cx, cy).fg();
        s.fill(Rect::new(area.x + cx as i32 * CELL_W, area.y + cy as i32 * CELL_H + CELL_H - 3, CELL_W, 2), color);
    }
}

/// Centered, word-wrapped icon label; selection draws it white-on-navy.
fn draw_label(s: &mut Surface, center_x: i32, y: i32, text: &str, max_chars: usize, selected: bool, color: u32) {
    for (i, line) in wrap(text, max_chars).iter().take(2).enumerate() {
        let w = text_width(line);
        let ly = y + i as i32 * 11;
        let x = center_x - w / 2;
        if selected {
            s.fill(Rect::new(x - 1, ly - 1, w + 2, 11), NAVY);
            s.text(x, ly + 1, line, WHITE);
        } else {
            s.text(x, ly + 1, line, color);
        }
    }
}

/// `1234567` -> `1,234,567`.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Elf,
    Script,
    Pe,
    Other,
}

fn sniff(path: &Path) -> Kind {
    use std::io::Read;
    let mut head = [0u8; 4];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    match &head[..n] {
        h if h.starts_with(b"\x7fELF") => Kind::Elf,
        h if h.starts_with(b"#!") => Kind::Script,
        h if h.starts_with(b"MZ") => Kind::Pe,
        _ => Kind::Other,
    }
}

/// Quotes a path for ferro-cmd only if it contains spaces.
fn quote_arg(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_owned()
    }
}

fn human_bytes(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} bytes")
    }
}

/// Identifies a file by its magic bytes, the basis for file associations:
/// ELF runs natively, PE runs through the Wine/Proton runtime.
fn file_type(path: &Path, name: &str) -> String {
    use std::io::Read;
    let mut head = [0u8; 512];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let h = &head[..n];
    if h.starts_with(b"\x7fELF") {
        return format!("Linux program (ELF {}-bit)", if h.get(4) == Some(&2) { 64 } else { 32 });
    }
    if h.starts_with(b"MZ") {
        let pe = h.get(0x3C..0x40).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let magic = pe.and_then(|o| Some((h.get(o..o + 4)?, h.get(o + 24..o + 26)?)));
        return match magic {
            Some((b"PE\0\0", [0x0b, 0x02])) => "Windows program (PE32+, 64-bit)".into(),
            Some((b"PE\0\0", [0x0b, 0x01])) => "Windows program (PE32, 32-bit)".into(),
            _ => "Windows/DOS program".into(),
        };
    }
    if h.starts_with(b"#!") {
        return "Script".into();
    }
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.eq_ignore_ascii_case("txt") => "Text Document".into(),
        Some((_, ext)) if !ext.is_empty() => format!("{} File", ext.to_uppercase()),
        _ => "File".into(),
    }
}

fn read_dir(drives: &DriveTable, path: &str) -> (Vec<Entry>, Option<String>) {
    let mut entries = Vec::new();
    if ferro_path::parent(path).is_some() {
        entries.push(Entry { name: "..".into(), is_dir: true });
    }
    let posix = match drives.to_posix(path) {
        Ok(p) => p,
        Err(e) => return (entries, Some(format!("Cannot open {path}: {e}."))),
    };
    let dir = match std::fs::read_dir(&posix) {
        Ok(d) => d,
        Err(e) => return (entries, Some(format!("Cannot open {path}: {e}."))),
    };
    let mut found: Vec<Entry> = dir
        .filter_map(Result::ok)
        .map(|e| Entry { name: e.file_name().to_string_lossy().into_owned(), is_dir: e.file_type().is_ok_and(|t| t.is_dir()) })
        .collect();
    found.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries.extend(found);
    (entries, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(shell: &mut Shell, x: i32, y: i32, t: u64) -> Option<Action> {
        shell.handle(Event::MouseDown { x, y, button: MouseButton::Left }, t);
        shell.handle(Event::MouseUp { x, y, button: MouseButton::Left }, t + 10)
    }

    #[test]
    fn double_click_opens_my_computer() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        let r = shell.desktop_icon_rect(0);
        click(&mut shell, r.x + 10, r.y + 10, 1000);
        assert!(shell.windows.is_empty());
        click(&mut shell, r.x + 10, r.y + 10, 1100);
        assert_eq!(shell.windows.len(), 1);
    }

    #[test]
    fn close_button_closes() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_about();
        let c = close_rect(shell.windows[0].rect);
        click(&mut shell, c.x + 4, c.y + 4, 0);
        assert!(shell.windows.is_empty());
    }

    #[test]
    fn shut_down_dialog_returns_action() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        click(&mut shell, 10, 590, 0);
        assert!(shell.start_open);
        let item = shell.start_item_rect(6);
        click(&mut shell, item.x + 5, item.y + 5, 2000);
        let win = &shell.windows[0];
        let items = shell.item_rects(win);
        let (restart, yes) = (items[1], items[2]);
        click(&mut shell, restart.x + 2, restart.y + 2, 4000);
        assert_eq!(click(&mut shell, yes.x + 5, yes.y + 5, 6000), Some(Action::Reboot));
    }

    #[test]
    fn title_drag_moves_window() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_about();
        let r = shell.windows[0].rect;
        let (x, y) = (r.x + 40, r.y + 10);
        shell.handle(Event::MouseDown { x, y, button: MouseButton::Left }, 0);
        shell.handle(Event::MouseMove { x: x + 30, y: y + 20 }, 5);
        shell.handle(Event::MouseUp { x: x + 30, y: y + 20, button: MouseButton::Left }, 10);
        assert_eq!((shell.windows[0].rect.x, shell.windows[0].rect.y), (r.x + 30, r.y + 20));
    }

    fn right_click(shell: &mut Shell, x: i32, y: i32) {
        shell.handle(Event::MouseDown { x, y, button: MouseButton::Right }, 0);
        shell.handle(Event::MouseUp { x, y, button: MouseButton::Right }, 0);
    }

    fn demo_procs() -> SystemInfo {
        // Owned by whoever runs the tests: the menu only budgets your own programs.
        let uid = ferro_sys::current_uid().unwrap_or(0);
        let p = |pid, name: &str, rss_kb| ProcInfo { pid, name: name.into(), rss_kb, uid, ..ProcInfo::default() };
        SystemInfo { processes: Some(vec![p(1, "init", 1000), p(40, "shell", 5000), p(41, "netd", 2000)]), ..SystemInfo::default() }
    }

    #[test]
    fn taskbar_menu_opens_task_manager() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        right_click(&mut shell, 500, 590);
        let m = shell.menu.as_ref().expect("taskbar menu");
        assert!(m.rect.bottom() <= 600, "menu flips up above the taskbar");
        let x = m.rect.x + 10;
        // Entry 6 is "Task Manager..." (after two separators).
        let y = (m.rect.y..m.rect.bottom()).find(|&y| m.item_at(x, y) == Some(6)).unwrap();
        click(&mut shell, x, y + 2, 0);
        assert!(shell.menu.is_none());
        assert!(shell.wants_processes());
    }

    #[test]
    fn task_manager_sorts_and_ends_process() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_task_manager();
        shell.set_info(demo_procs());
        let win = &shell.windows[0];
        let Content::TaskManager(st) = &win.content else { panic!() };
        let rows: Vec<u32> = shell.tm_rows(st).iter().map(|p| p.pid).collect();
        assert_eq!(rows, [40, 41, 1], "default sort is memory, biggest first");

        let items = shell.item_rects(win);
        let first_row = items[taskmgr_row0()];
        click(&mut shell, first_row.x + 5, first_row.y + 5, 0);
        let end = shell.item_rects(&shell.windows[0])[taskmgr::END_BUTTON];
        assert_eq!(click(&mut shell, end.x + 5, end.y + 5, 2000), Some(Action::EndProcess(40)));

        // Right-click init: End Process is disabled for PID 1.
        let init_row = shell.item_rects(&shell.windows[0])[taskmgr_row0() + 2];
        right_click(&mut shell, init_row.x + 5, init_row.y + 5);
        let m = shell.menu.as_ref().unwrap();
        assert!(matches!(m.target_for_test(), menu::Target::Process(1)));
    }

    fn taskmgr_row0() -> usize {
        taskmgr::ROW0
    }

    #[test]
    fn process_menu_sets_ram_budget() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_task_manager();
        shell.set_info(demo_procs());
        let row = shell.item_rects(&shell.windows[0])[taskmgr_row0()]; // pid 40, biggest
        right_click(&mut shell, row.x + 5, row.y + 5);
        let m = shell.menu.as_ref().unwrap();
        let x = m.rect.x + 10;
        // Entries: End Process, separator, Set RAM Budget..., Remove RAM Budget
        let y = (m.rect.y..m.rect.bottom()).find(|&y| m.item_at(x, y) == Some(2)).unwrap();
        let store = std::env::temp_dir().join(format!("ferro-budget-{}.conf", std::process::id()));
        shell.set_store_path(&store);
        assert_eq!(click(&mut shell, x, y + 2, 0), None, "opens the typed-budget dialog");
        assert!(matches!(shell.windows.last().unwrap().content, Content::Input(_)));
        let mut action = None;
        for key in [Key::Char('5'), Key::Char(' '), Key::Char('g'), Key::Char('b'), Key::Enter] {
            action = shell.handle(Event::Key { key, mods: Mods::default() }, 0);
        }
        assert_eq!(action, Some(Action::SetMemoryLimit { pid: 40, bytes: Some(5 << 30) }));
        // "Always use this budget" is ticked by default: remembered for the app.
        let saved = ferro_sandbox::store::Store::load(&store);
        assert_eq!(saved.app("shell").memory, Some(Some(5 << 30)));
        std::fs::remove_file(&store).unwrap();
    }

    #[test]
    fn ctrl_keys_are_not_typed_into_the_budget() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_budget_dialog(42, "viewer");
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        shell.handle(Event::Key { key: Key::Char('a'), mods: ctrl }, 0);
        for key in [Key::Char('5'), Key::Char('0'), Key::Char('m')] {
            shell.handle(Event::Key { key, mods: Mods::default() }, 0);
        }
        let action = shell.handle(Event::Key { key: Key::Enter, mods: Mods::default() }, 0);
        assert_eq!(action, Some(Action::SetMemoryLimit { pid: 42, bytes: Some(50 << 20) }));
    }

    #[test]
    fn bad_budget_text_shows_an_error() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.open_budget_dialog(42, "viewer");
        for key in [Key::Char('l'), Key::Char('o'), Key::Char('t'), Key::Char('s'), Key::Enter] {
            assert_eq!(shell.handle(Event::Key { key, mods: Mods::default() }, 0), None);
        }
        assert!(matches!(&shell.windows.last().unwrap().content, Content::Input(i) if i.error_text().is_some()));
    }

    /// A stand-in for ferro-system that records what the shell asked.
    #[derive(Default)]
    struct FakeLink {
        state: Option<VaultState>,
        password: Option<String>,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl SystemLink for FakeLink {
        fn vault_state(&mut self) -> VaultState {
            self.state.unwrap_or(VaultState::None)
        }
        fn create_vault(&mut self, password: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("create {password}"));
            Ok(())
        }
        fn unlock_vault(&mut self, password: &str) -> Result<(), String> {
            if Some(password) == self.password.as_deref() {
                Ok(())
            } else {
                Err("Wrong password.".into())
            }
        }
        fn amnesic(&mut self) {
            self.log.borrow_mut().push("amnesic".into());
        }
        fn apply_switches(&mut self, s: ferro_sandbox::store::Switches) {
            self.log.borrow_mut().push(format!("switches {} {} {} {} {}", s.network, s.microphone, s.camera, s.vpn, s.tor));
        }
        fn limit_memory(&mut self, pid: u32, bytes: Option<u64>) -> Result<(), String> {
            self.log.borrow_mut().push(format!("limit {pid} {bytes:?}"));
            Ok(())
        }
    }

    fn type_text(shell: &mut Shell, s: &str) {
        for c in s.chars() {
            shell.handle(Event::Key { key: Key::Char(c), mods: Mods::default() }, 0);
        }
    }

    fn press(shell: &mut Shell, key: Key) -> Option<Action> {
        shell.handle(Event::Key { key, mods: Mods::default() }, 0)
    }

    #[test]
    fn first_boot_creates_vault_with_confirmed_password() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.set_system_link(Box::new(FakeLink { state: Some(VaultState::New), log: log.clone(), ..FakeLink::default() }));
        shell.begin_logon();
        assert!(shell.logon_active());
        type_text(&mut shell, "short");
        press(&mut shell, Key::Enter); // to confirm field
        type_text(&mut shell, "short");
        assert_eq!(press(&mut shell, Key::Enter), None, "too short is refused");
        assert!(shell.logon_active());
        type_text(&mut shell, "correct horse");
        press(&mut shell, Key::Tab);
        type_text(&mut shell, "correct horse");
        assert_eq!(press(&mut shell, Key::Enter), Some(Action::SwitchesChanged));
        assert!(!shell.logon_active());
        assert_eq!(log.borrow()[0], "create correct horse");
        assert!(log.borrow()[1].starts_with("switches"), "saved switches are enforced");
    }

    #[test]
    fn wrong_password_keeps_the_desktop_locked() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        let link = FakeLink { state: Some(VaultState::Locked), password: Some("hunter22".into()), ..FakeLink::default() };
        shell.set_system_link(Box::new(link));
        shell.begin_logon();
        type_text(&mut shell, "guess123");
        assert_eq!(press(&mut shell, Key::Enter), None);
        assert!(shell.logon_active(), "still locked");
        // Clicks elsewhere don't reach the desktop while locked.
        click(&mut shell, 40, 40, 0);
        click(&mut shell, 40, 40, 100);
        assert!(shell.windows.is_empty());
        type_text(&mut shell, "hunter22");
        press(&mut shell, Key::Enter);
        assert!(!shell.logon_active());
    }

    #[test]
    fn long_password_stays_inside_its_field() {
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.set_system_link(Box::new(FakeLink { state: Some(VaultState::Locked), ..FakeLink::default() }));
        shell.begin_logon();
        type_text(&mut shell, &"x".repeat(200));
        let mut s = Surface::new(800, 600);
        shell.draw(&mut s);
        let f = shell.logon_layout(logon::Mode::Unlock).fields[0];
        for y in f.y..f.bottom() {
            for x in f.right()..f.right() + 16 {
                assert_ne!(s.pixels()[y as usize * 800 + x as usize], BLACK, "star drawn outside the field at ({x},{y})");
            }
        }
    }

    #[test]
    fn skip_means_amnesic() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.set_system_link(Box::new(FakeLink { state: Some(VaultState::Locked), log: log.clone(), ..FakeLink::default() }));
        shell.begin_logon();
        let skip = shell.logon_layout(logon::Mode::Unlock).other;
        click(&mut shell, skip.x + 5, skip.y + 5, 0);
        assert!(!shell.logon_active());
        assert_eq!(log.borrow()[0], "amnesic");
    }

    #[test]
    fn tray_kill_switches_toggle_and_persist() {
        let store = std::env::temp_dir().join(format!("ferro-switch-{}.conf", std::process::id()));
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.set_store_path(&store);
        assert!(shell.switches().network && !shell.switches().camera, "privacy-first defaults");
        let cam = shell.tray_icon_rects()[2];
        // Tray icons act on press, like Windows'.
        let press = shell.handle(Event::MouseDown { x: cam.x + 4, y: cam.y + 4, button: MouseButton::Left }, 0);
        assert_eq!(press, Some(Action::SwitchesChanged));
        assert!(shell.switches().camera);
        let mut again = Shell::new(800, 600, DriveTable::default());
        again.set_store_path(&store);
        assert!(again.switches().camera, "remembered across restarts");
        std::fs::remove_file(&store).unwrap();
    }

    #[test]
    fn app_permissions_forget() {
        let store = std::env::temp_dir().join(format!("ferro-perms-{}.conf", std::process::id()));
        let mut s = ferro_sandbox::store::Store::load(&store);
        s.set_network("browser", Some(ferro_sandbox::store::NetChoice::Allow));
        s.set_memory("viewer", Some(Some(50 << 20)));
        s.save().unwrap();
        let mut shell = Shell::new(800, 600, DriveTable::default());
        shell.set_store_path(&store);
        shell.open_app_permissions();
        let items = shell.item_rects(&shell.windows[0]);
        let row0 = items[3];
        click(&mut shell, row0.x + 5, row0.y + 5, 0); // select "browser"
        let forget = shell.item_rects(&shell.windows[0])[0];
        click(&mut shell, forget.x + 5, forget.y + 5, 1000);
        let saved = ferro_sandbox::store::Store::load(&store);
        assert!(!saved.apps.contains_key("browser") && saved.apps.contains_key("viewer"));
        let all = shell.item_rects(&shell.windows[0])[1];
        click(&mut shell, all.x + 5, all.y + 5, 2000);
        assert!(ferro_sandbox::store::Store::load(&store).apps.is_empty());
        std::fs::remove_file(&store).unwrap();
    }

    fn write_jpeg_with_gps(path: &std::path::Path) {
        let seg = |m: u8, p: &[u8]| [&[0xFF, m][..], &((p.len() + 2) as u16).to_be_bytes(), p].concat();
        let jpeg = [vec![0xFF, 0xD8], seg(0xE1, b"Exif\0\0GPS 51.5N"), seg(0xDA, &[0; 4]), vec![1, 2, 0xFF, 0xD9]].concat();
        std::fs::write(path, jpeg).unwrap();
    }

    #[test]
    fn remove_personal_info_from_explorer() {
        let dir = std::env::temp_dir().join(format!("ferro-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_jpeg_with_gps(&dir.join("holiday.jpg"));
        let mut drives = DriveTable::empty();
        drives.mount('T', &dir.to_string_lossy().replace('\\', "/"));
        let mut shell = Shell::new(800, 600, drives);
        shell.open_explorer(r"T:\");
        let items = shell.item_rects(&shell.windows[0]);
        let row = items[0]; // a drive root has no ".." entry
        right_click(&mut shell, row.x + 30, row.y + 5);
        let m = shell.menu.as_ref().unwrap();
        let x = m.rect.x + 10;
        // Entries: Open, separator, Remove Metadata, ...
        let y = (m.rect.y..m.rect.bottom()).find(|&y| m.item_at(x, y) == Some(2)).unwrap();
        click(&mut shell, x, y + 2, 0);
        let cleaned = std::fs::read(dir.join("holiday.jpg")).unwrap();
        assert!(!cleaned.windows(3).any(|w| w == b"GPS"), "GPS data removed");
        assert!(cleaned.ends_with(&[1, 2, 0xFF, 0xD9]), "image data kept");
        assert!(matches!(&shell.windows.last().unwrap().content, Content::Message { lines, .. } if lines[0].contains("holiday.jpg")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_files_by_content() {
        let dir = std::env::temp_dir().join(format!("ferro-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tool"), b"\x7fELF\x02rest").unwrap();
        std::fs::write(dir.join("game.exe"), b"MZ\x90\0").unwrap();
        assert_eq!(sniff(&dir.join("tool")), Kind::Elf);
        assert_eq!(sniff(&dir.join("game.exe")), Kind::Pe);
        let mut drives = DriveTable::empty();
        drives.mount('T', &dir.to_string_lossy().replace('\\', "/"));
        let mut shell = Shell::new(800, 600, drives);
        let opened = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
        let seen = opened.clone();
        shell.set_terminal_spawner(Box::new(move |_, _, cmd| {
            *seen.borrow_mut() = cmd.map(str::to_owned);
            Err(std::io::Error::other("test"))
        }));
        shell.open_file(r"T:\tool");
        assert_eq!(opened.borrow().as_deref(), Some(r"T:\tool"));
        shell.open_file(r"T:\game.exe");
        assert!(matches!(&shell.windows.last().unwrap().content, Content::Message { lines, .. } if lines.join(" ").contains("Wine")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn explorer_context_menu_creates_folder() {
        let dir = std::env::temp_dir().join(format!("ferro-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut drives = DriveTable::empty();
        drives.mount('T', &dir.to_string_lossy().replace('\\', "/"));
        let mut shell = Shell::new(800, 600, drives);
        shell.open_explorer(r"T:\");
        let pane = Shell::pane_rect(&shell.windows[0]);
        let (x, y) = (pane.right() - 20, pane.bottom() - 20);
        // Entry 2 of the folder menu is "New Folder"; the second use gets a
        // numbered name, like Windows.
        for expected in ["New Folder", "New Folder (2)"] {
            right_click(&mut shell, x, y);
            let m = shell.menu.as_ref().expect("folder menu");
            let mx = m.rect.x + 10;
            let my = (m.rect.y..m.rect.bottom()).find(|&yy| m.item_at(mx, yy) == Some(2)).unwrap();
            click(&mut shell, mx, my + 2, 0);
            assert!(dir.join(expected).is_dir(), "{expected} not created");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn digits_and_file_types() {
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(group_digits(999), "999");
        assert_eq!(file_type(Path::new("/nonexistent"), "notes.txt"), "Text Document");
        assert_eq!(file_type(Path::new("/nonexistent"), "a.rs"), "RS File");
    }

    #[test]
    fn cursor_moves_redraw_only_the_cursor_and_match_a_full_redraw() {
        let mut shell = Shell::new(320, 240, DriveTable::default());
        shell.open_about();
        let mut frame = Surface::new(320, 240);
        shell.draw(&mut frame);
        shell.take_damage();
        shell.handle(Event::MouseMove { x: 150, y: 120 }, 0);
        let Damage::Rects(rects) = shell.take_damage() else { panic!("expected partial damage") };
        assert_eq!(rects.len(), 2);
        for r in &rects {
            frame.set_clip(Some(*r));
            shell.draw(&mut frame);
        }
        frame.set_clip(None);
        let mut full = Surface::new(320, 240);
        shell.draw(&mut full);
        assert_eq!(frame.pixels(), full.pixels(), "partial redraw left a trail");
        // With a menu open, hover highlights change: redraw everything.
        shell.toggle_start_menu();
        shell.take_damage();
        shell.handle(Event::MouseMove { x: 20, y: 200 }, 0);
        assert_eq!(shell.take_damage(), Damage::Full);
    }

    #[test]
    fn draws_every_window_kind() {
        let mut shell = Shell::new(640, 480, DriveTable::default());
        shell.open_my_computer();
        shell.open_explorer(r"C:\");
        shell.open_recycle_bin();
        shell.open_about();
        shell.open_shut_down();
        shell.open_task_manager();
        shell.set_info(demo_procs());
        shell.open_properties(r"C:\");
        shell.open_app_permissions();
        shell.open_budget_dialog(7, "app");
        shell.toggle_start_menu();
        let mut s = Surface::new(640, 480);
        shell.draw(&mut s);
    }
}
