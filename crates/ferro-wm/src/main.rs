//! ferro-wm: FerroOS Heavy's window manager. Every program's window gets a
//! Windows 95 frame (drawn by the `frame` module with ferro-gfx): a title
//! bar to drag it by, minimize, maximize and close buttons, borders to
//! resize it by. Click a window to bring it forward and give it the keyboard.
//!
//! It speaks the X11 protocol itself (no C libraries), so it's one static
//! binary like the rest of FerroOS. Programs that ask for the whole screen
//! (games) get it, without a frame.
//!
//! The taskbar along the bottom is part of it (the `bar` module draws it):
//! Start opens a menu of every installed program (`apps`), each open window
//! has a button to bring it forward or put it away, and there's a clock.

mod appearance;
mod apps;
mod bar;
mod desktop;
mod frame;
mod windows;

use appearance::Appearance;
use bar::{BarPart, Item, Task};
use ferro_gfx::{color, icons, Rect, Surface};
use frame::{Look, Part};
use std::collections::HashMap;
use windows::WindowStore;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// The few X11 names (atoms) the window manager uses.
struct Atoms {
    wm_protocols: Atom,
    wm_delete_window: Atom,
    wm_state: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    net_wm_state: Atom,
    net_wm_state_fullscreen: Atom,
    net_wm_window_type: Atom,
    net_wm_window_type_dock: Atom,
    net_wm_window_type_desktop: Atom,
    net_wm_window_type_dialog: Atom,
    net_wm_window_type_menu: Atom,
    net_wm_window_type_popup_menu: Atom,
    net_wm_window_type_dropdown_menu: Atom,
    net_wm_window_type_tooltip: Atom,
    net_wm_window_type_notification: Atom,
    net_wm_window_type_combo: Atom,
    net_wm_window_type_dnd: Atom,
    net_wm_window_type_utility: Atom,
    net_wm_window_type_splash: Atom,
    net_active_window: Atom,
    net_supported: Atom,
    net_client_list: Atom,
    ferro_appearance: Atom,
    ferro_switches: Atom,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Res<Self> {
        let get = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        Ok(Self {
            wm_protocols: get("WM_PROTOCOLS")?,
            wm_delete_window: get("WM_DELETE_WINDOW")?,
            wm_state: get("WM_STATE")?,
            net_wm_name: get("_NET_WM_NAME")?,
            utf8_string: get("UTF8_STRING")?,
            net_wm_state: get("_NET_WM_STATE")?,
            net_wm_state_fullscreen: get("_NET_WM_STATE_FULLSCREEN")?,
            net_wm_window_type: get("_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_dock: get("_NET_WM_WINDOW_TYPE_DOCK")?,
            net_wm_window_type_desktop: get("_NET_WM_WINDOW_TYPE_DESKTOP")?,
            net_wm_window_type_dialog: get("_NET_WM_WINDOW_TYPE_DIALOG")?,
            net_wm_window_type_menu: get("_NET_WM_WINDOW_TYPE_MENU")?,
            net_wm_window_type_popup_menu: get("_NET_WM_WINDOW_TYPE_POPUP_MENU")?,
            net_wm_window_type_dropdown_menu: get("_NET_WM_WINDOW_TYPE_DROPDOWN_MENU")?,
            net_wm_window_type_tooltip: get("_NET_WM_WINDOW_TYPE_TOOLTIP")?,
            net_wm_window_type_notification: get("_NET_WM_WINDOW_TYPE_NOTIFICATION")?,
            net_wm_window_type_combo: get("_NET_WM_WINDOW_TYPE_COMBO")?,
            net_wm_window_type_dnd: get("_NET_WM_WINDOW_TYPE_DND")?,
            net_wm_window_type_utility: get("_NET_WM_WINDOW_TYPE_UTILITY")?,
            net_wm_window_type_splash: get("_NET_WM_WINDOW_TYPE_SPLASH")?,
            net_active_window: get("_NET_ACTIVE_WINDOW")?,
            net_supported: get("_NET_SUPPORTED")?,
            net_client_list: get("_NET_CLIENT_LIST")?,
            ferro_appearance: get("_FERRO_APPEARANCE")?,
            ferro_switches: get("_FERRO_SWITCHES")?,
        })
    }
}

/// One program window and the frame around it.
struct Managed {
    client: Window,
    frame: Window,
    class: String,
    /// The frame's place on screen.
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: String,
    /// Where it goes back to from maximized or full screen.
    restore: Option<(i32, i32, i32, i32)>,
    maximized: bool,
    fullscreen: bool,
    minimized: bool,
    is_dialog: bool,
    /// Unmaps we caused ourselves (reparenting, minimizing), not the program closing its window.
    ignore_unmaps: u32,
}

/// What the mouse is in the middle of doing.
enum Drag {
    Move { frame: Window, dx: i32, dy: i32 },
    Resize { frame: Window, sides: (bool, bool, bool, bool), start: (i32, i32), origin: (i32, i32, i32, i32) },
    Desktop { item: usize, dx: i32, dy: i32 },
}

struct Wm {
    conn: RustConnection,
    root: Window,
    screen_w: i32,
    screen_h: i32,
    depth: u8,
    visual: Visualid,
    gc: Gcontext,
    atoms: Atoms,
    /// By frame window.
    windows: HashMap<Window, Managed>,
    /// Frames, back to front.
    order: Vec<Window>,
    active: Option<Window>,
    drag: Option<Drag>,
    /// The taskbar's window.
    bar: Window,
    /// Frames in the order their windows opened: the taskbar's buttons.
    tasks: Vec<Window>,
    /// The taskbar shows something that has changed.
    bar_stale: bool,
    /// The Start menu or context menu, while it's open.
    menu: Option<Menu>,
    /// The flyout submenu (e.g. Programs or Settings), while it's open.
    submenu: Option<Menu>,
    /// The tertiary flyout submenu (e.g. category apps), while it's open.
    nested_submenu: Option<Menu>,
    /// In-memory cache of installed applications.
    app_cache: Option<Vec<apps::App>>,
    /// Timestamp when application cache was last updated.
    app_cache_time: Option<std::time::Instant>,
    /// Desktop items manager (files, folders, shortcuts).
    desktop: desktop::DesktopManager,
    /// Window placement persistence across sessions.
    window_store: WindowStore,
    /// Pinned applications on the taskbar.
    pinned_apps: Vec<bar::PinnedApp>,
    /// Last click time on window title bar for double click maximize detection.
    last_title_click: Option<(std::time::Instant, Window)>,
    /// Last click time and item index on desktop for double click launch.
    last_desktop_click: Option<(std::time::Instant, usize)>,
    last_clock_click: Option<std::time::Instant>,
    /// Desktop appearance and theme settings.
    appearance: Appearance,
    /// Cached decoded wallpaper image.
    cached_wallpaper: Option<ferro_gfx::DecodedImage>,
    /// Pre-rendered desktop background (wallpaper or solid color).
    desktop_bg: Surface<'static>,
    /// Full desktop buffer with background and items drawn.
    desktop_surface: Surface<'static>,
    /// Windows minimized by Show Desktop shortcut (Win+D).
    show_desktop_minimized: Option<Vec<Window>>,
    /// Privacy and kill switch choices.
    store: ferro_sandbox::store::Store,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SubmenuKind {
    Programs,
    Settings,
    Category(String),
    View,
    SortBy,
    New,
}

/// One line of an open menu and what choosing it does.
#[derive(Clone, Debug)]
enum Choice {
    Submenu(SubmenuKind),
    Run(apps::App),
    Action {
        label: &'static str,
        command: String,
        icon: Option<ferro_gfx::icons::Icon>,
    },
    PinTaskbar {
        name: String,
        exec: String,
    },
    CreateDesktopShortcut {
        name: String,
        exec: String,
    },
    DeleteDesktopItem(usize),
    RenameDesktopItem(usize),
    ToggleAutoArrange(bool),
    ToggleAlignToGrid(bool),
    ArrangeIconsByName,
    ArrangeIconsByType,
    #[allow(dead_code)]
    AlignToGrid,
    RefreshDesktop,
    LogOff,
    Standby,
    ShutDown,
    Restart,
    Separator,
}

struct Menu {
    window: Window,
    x: i32,
    y: i32,
    choices: Vec<Choice>,
    hover: Option<usize>,
    with_band: bool,
    width: i32,
    layout: bar::MenuLayout,
    #[allow(dead_code)]
    kind: Option<SubmenuKind>,
}

impl Menu {
    fn items(&self) -> Vec<Item<'_>> {
        self.choices
            .iter()
            .map(|c| match c {
                Choice::Submenu(SubmenuKind::Programs) => Item::entry("Programs", Some(icons::FOLDER), true),
                Choice::Submenu(SubmenuKind::Settings) => Item::entry("Settings", Some(icons::COMPUTER), true),
                Choice::Submenu(SubmenuKind::Category(cat)) => Item::entry(cat, Some(apps::category_icon(cat)), true),
                Choice::Submenu(SubmenuKind::View) => Item::entry("View", None, true),
                Choice::Submenu(SubmenuKind::SortBy) => Item::entry("Sort by", None, true),
                Choice::Submenu(SubmenuKind::New) => Item::entry("New", None, true),
                Choice::Run(app) => {
                    let icon = if app.terminal {
                        Some(icons::TERMINAL)
                    } else if app.exec.contains("file") || app.name.contains("File") {
                        Some(icons::FOLDER)
                    } else {
                        Some(icons::FILE)
                    };
                    Item::entry(&app.name, icon, false)
                }
                Choice::Action { label, icon, .. } => Item::entry(label, *icon, false),
                Choice::PinTaskbar { .. } => Item::entry("Pin to Taskbar", Some(icons::FERRO), false),
                Choice::CreateDesktopShortcut { .. } => Item::entry("Create Desktop Shortcut", Some(icons::COMPUTER), false),
                Choice::DeleteDesktopItem(_) => Item::entry("Delete", Some(icons::RECYCLE), false),
                Choice::RenameDesktopItem(_) => Item::entry("Rename...", None, false),
                Choice::ToggleAutoArrange(on) => {
                    let label = if *on { "[v] Auto arrange icons" } else { "[  ] Auto arrange icons" };
                    Item::entry(label, None, false)
                }
                Choice::ToggleAlignToGrid(on) => {
                    let label = if *on { "[v] Align icons to grid" } else { "[  ] Align icons to grid" };
                    Item::entry(label, None, false)
                }
                Choice::RefreshDesktop => Item::entry("Refresh", None, false),
                Choice::ArrangeIconsByName => Item::entry("Arrange by Name", None, false),
                Choice::ArrangeIconsByType => Item::entry("Arrange by Type", None, false),
                Choice::AlignToGrid => Item::entry("Align to Grid", None, false),
                Choice::LogOff => Item::entry("Log Off...", Some(icons::FERRO), false),
                Choice::Standby => Item::entry("Standby (Encrypted Sleep)...", Some(icons::COMPUTER), false),
                Choice::ShutDown => Item::entry("Shut Down...", Some(icons::COMPUTER), false),
                Choice::Restart => Item::entry("Restart...", Some(icons::COMPUTER), false),
                Choice::Separator => Item::Separator,
            })
            .collect()
    }
}

/// Where installed programs describe themselves.
fn app_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(format!("{home}/.local/share/applications"));
    }
    dirs.push("/usr/local/share/applications".to_string());
    dirs.push("/usr/share/applications".to_string());
    dirs
}

/// The time for the taskbar, as Windows 95 showed it: `10:14 PM`.
fn clock_text() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    #[cfg(unix)]
    let (hour, minute) = {
        // SAFETY: localtime_r fills the zeroed struct it's given.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = now.try_into().unwrap_or(0);
        unsafe { libc::localtime_r(&t, &mut tm) };
        (tm.tm_hour as u64, tm.tm_min as u64)
    };
    #[cfg(not(unix))]
    let (hour, minute) = (now / 3600 % 24, now / 60 % 60);
    clock_format(hour, minute)
}

fn clock_format(hour: u64, minute: u64) -> String {
    let h12 = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
    format!("{h12}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
}

/// The system tray text: true memory footprint (in MB) and clock, as on Lite.
fn tray_text() -> String {
    let clock = clock_text();
    match ferro_sys::MemInfo::read() {
        Some(m) => {
            let mb = (m.footprint_kb() + 512) / 1024;
            format!("{mb} MB  {clock}")
        }
        None => clock,
    }
}

/// Starts a program by its command line, detached from the window manager.
fn launch(command: &str) {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", command]).stdin(std::process::Stdio::null());
    if let Err(e) = cmd.spawn() {
        eprintln!("ferro-wm: {command}: {e}");
    }
}

fn send_switches(s: ferro_sandbox::store::Switches) {
    #[cfg(unix)]
    {
        use std::io::Write;
        let b = |v: bool| if v { 1 } else { 0 };
        let line = format!("SWITCHES {} {} {} {} {}", b(s.network), b(s.microphone), b(s.camera), b(s.vpn), b(s.tor));
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect("/run/ferro/system.sock") {
            let _ = writeln!(stream, "{line}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = s;
    }
}

/// Key codes on the standard PC keyboard map every X server here uses.
const KEY_ESCAPE: u8 = 9;
const KEY_F4: u8 = 70;
const KEY_E: u8 = 26;
const KEY_D: u8 = 40;
const KEY_L: u8 = 46;
const KEY_SUPER_L: u8 = 133;
const KEY_SUPER_R: u8 = 134;
const KEY_RETURN: u8 = 36;
const KEY_F2: u8 = 68;
const KEY_DELETE: u8 = 119;

/// The held-down keys as X11 numbers them.
const SHIFT: u16 = 1;
const CAPS_LOCK: u16 = 2;
const CONTROL: u16 = 4;
const ALT: u16 = 8;
const NUM_LOCK: u16 = 16;
const SUPER: u16 = 64; // Mod4

/// The keys that work whatever window has the typing: what they need held
/// down, the key, and what they do.
const SHORTCUTS: [(u16, u8, Shortcut); 8] = [
    (CONTROL | SHIFT, KEY_ESCAPE, Shortcut::TaskManager),
    (CONTROL, KEY_ESCAPE, Shortcut::StartMenu),
    (ALT, KEY_F4, Shortcut::CloseWindow),
    (0, KEY_SUPER_L, Shortcut::StartMenu),
    (0, KEY_SUPER_R, Shortcut::StartMenu),
    (SUPER, KEY_E, Shortcut::Files),
    (SUPER, KEY_D, Shortcut::ShowDesktop),
    (SUPER, KEY_L, Shortcut::LockScreen),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    TaskManager,
    StartMenu,
    CloseWindow,
    Files,
    ShowDesktop,
    LockScreen,
}

/// Which shortcut a key press is, if any. Caps Lock and Num Lock being on
/// (they count as held keys) changes nothing.
fn shortcut(state: u16, key: u8) -> Option<Shortcut> {
    let held = state & !(CAPS_LOCK | NUM_LOCK) & (SHIFT | CONTROL | ALT | SUPER);
    SHORTCUTS.iter().find(|(mods, k, _)| *k == key && *mods == held).map(|s| s.2)
}

const MIN_W: i32 = 112;
const MIN_H: i32 = frame::TOP + frame::BOTTOM + 1;

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-wm: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let root = screen.root;
    let appearance = Appearance::load();
    let cached_wallpaper = appearance.wallpaper.as_ref().and_then(ferro_gfx::DecodedImage::load_from_file);
    // Only one window manager can ask for this: it fails if another runs.
    let mask = EventMask::SUBSTRUCTURE_REDIRECT
        | EventMask::SUBSTRUCTURE_NOTIFY
        | EventMask::PROPERTY_CHANGE
        | EventMask::BUTTON_PRESS
        | EventMask::POINTER_MOTION
        | EventMask::EXPOSURE
        | EventMask::KEY_PRESS;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(mask).background_pixel(appearance.desktop_color))?
        .check()
        .map_err(|_| "another window manager is running")?;
    conn.clear_area(false, root, 0, 0, 0, 0)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, root, &CreateGCAux::new())?;
    let atoms = Atoms::new(&conn)?;
    // Finished programs we started are forgotten, not kept as zombies.
    #[cfg(unix)]
    // SAFETY: installing SIG_IGN has no preconditions.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }
    let (screen_w, screen_h) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let bar_win = conn.generate_id()?;
    conn.create_window(
        screen.root_depth,
        bar_win,
        root,
        0,
        (screen_h - bar::HEIGHT) as i16,
        screen_w as u16,
        bar::HEIGHT as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS),
    )?;
    conn.map_window(bar_win)?;
    // The clock: a second connection asks for the taskbar to be redrawn
    // now and then, which wakes the loop below without it having to poll.
    std::thread::spawn(move || {
        let Ok((tick, _)) = x11rb::connect(None) else { return };
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            if tick.clear_area(true, bar_win, 0, 0, 0, 0).is_err() || tick.flush().is_err() {
                return;
            }
        }
    });

    let pinned_apps = bar::load_pinned();
    let desktop_mgr = desktop::DesktopManager::new(screen_w, screen_h);
    let window_store = WindowStore::load();

    let mut desktop_bg = Surface::new(screen_w as usize, screen_h as usize);
    if let Some(ref img) = cached_wallpaper {
        desktop_bg.draw_image_scaled(Rect::new(0, 0, screen_w, screen_h), img);
    } else {
        desktop_bg.fill(Rect::new(0, 0, screen_w, screen_h), appearance.desktop_color);
    }
    let desktop_surface = Surface::new(screen_w as usize, screen_h as usize);

    let mut wm = Wm {
        root,
        screen_w,
        screen_h,
        bar: bar_win,
        tasks: Vec::new(),
        bar_stale: true,
        menu: None,
        submenu: None,
        nested_submenu: None,
        app_cache: None,
        app_cache_time: None,
        desktop: desktop_mgr,
        window_store,
        pinned_apps,
        last_title_click: None,
        last_desktop_click: None,
        last_clock_click: None,
        depth: screen.root_depth,
        visual: screen.root_visual,
        gc,
        atoms,
        windows: HashMap::new(),
        order: Vec::new(),
        active: None,
        drag: None,
        appearance,
        cached_wallpaper,
        desktop_bg,
        desktop_surface,
        show_desktop_minimized: None,
        store: ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH),
        conn,
    };
    wm.announce()?;
    // The shortcuts come to us first, with or without the lock keys on.
    for (mods, key, _) in SHORTCUTS {
        for locks in [0, CAPS_LOCK, NUM_LOCK, CAPS_LOCK | NUM_LOCK] {
            let _ = wm.conn.grab_key(true, root, ModMask::from(mods | locks), key, GrabMode::ASYNC, GrabMode::ASYNC);
        }
    }
    // Windows that were there before us.
    let existing = wm.conn.query_tree(root)?.reply()?.children;
    for win in existing {
        let attrs = wm.conn.get_window_attributes(win)?.reply()?;
        if attrs.map_state == MapState::VIEWABLE && !attrs.override_redirect {
            wm.manage(win, true)?;
        }
    }
    wm.paint_desktop()?;
    wm.conn.flush()?;
    loop {
        let mut event = wm.conn.wait_for_event()?;
        if let Event::MotionNotify(mut m) = event {
            while let Ok(Some(next)) = wm.conn.poll_for_event() {
                if let Event::MotionNotify(next_m) = next {
                    m = next_m;
                } else {
                    if let Err(e) = wm.handle(Event::MotionNotify(m)) {
                        eprintln!("ferro-wm: {e}");
                    }
                    if std::mem::take(&mut wm.bar_stale) {
                        wm.paint_bar()?;
                    }
                    wm.conn.flush()?;
                    event = next;
                    break;
                }
            }
            if let Event::MotionNotify(_) = event {
                event = Event::MotionNotify(m);
            }
        }
        if let Err(e) = wm.handle(event) {
            // A window that vanished mid-request is everyday life here.
            eprintln!("ferro-wm: {e}");
        }
        if std::mem::take(&mut wm.bar_stale) {
            wm.paint_bar()?;
        }
        wm.conn.flush()?;
    }
}

impl Wm {
    /// The screen's height above the taskbar: where windows live.
    fn work_h(&self) -> i32 {
        self.screen_h - bar::HEIGHT
    }

    /// Sends part of a drawn surface to a window, in pieces so no request
    /// outgrows what the server accepts.
    fn blit(&self, win: Window, s: &Surface, stride: i32, (x, y, w, h): (i32, i32, i32, i32)) -> Res<()> {
        self.blit_at(win, s, stride, (x, y), (x, y), (w, h))
    }

    /// Sends a portion `(src_x, src_y, w, h)` of surface `s` to position `(dst_x, dst_y)` on `win`.
    fn blit_at(&self, win: Window, s: &Surface, stride: i32, (src_x, src_y): (i32, i32), (dst_x, dst_y): (i32, i32), (w, h): (i32, i32)) -> Res<()> {
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let px = s.pixels();
        let rows_per = (48 * 1024 / (w as usize * 4)).max(1) as i32;
        let mut row = 0;
        let mut data = Vec::with_capacity((w * rows_per * 4) as usize);
        while row < h {
            let n = rows_per.min(h - row);
            data.clear();
            for j in 0..n {
                let start = ((src_y + row + j) * stride + src_x) as usize;
                for p in &px[start..start + w as usize] {
                    data.extend_from_slice(&p.to_le_bytes());
                }
            }
            self.conn.put_image(ImageFormat::Z_PIXMAP, win, self.gc, w as u16, n as u16, dst_x as i16, (dst_y + row) as i16, 0, self.depth, &data)?;
            row += n;
        }
        Ok(())
    }

    fn paint_bar(&self) -> Res<()> {
        let tasks: Vec<Task> =
            self.tasks.iter().filter_map(|f| self.windows.get(f)).map(|m| Task { title: &m.title, active: self.active == Some(m.frame) }).collect();
        let mut s = Surface::new(self.screen_w as usize, bar::HEIGHT as usize);
        bar::draw(&mut s, self.screen_w, &tasks, &self.pinned_apps, self.menu.is_some(), &tray_text(), Some(&self.store.switches));
        self.blit(self.bar, &s, self.screen_w, (0, 0, self.screen_w, bar::HEIGHT))
    }

    fn update_desktop_bg(&mut self) {
        if let Some(ref img) = self.cached_wallpaper {
            self.desktop_bg.draw_image_scaled(Rect::new(0, 0, self.screen_w, self.screen_h), img);
        } else {
            self.desktop_bg.fill(Rect::new(0, 0, self.screen_w, self.screen_h), self.appearance.desktop_color);
        }
    }

    fn reload_wallpaper(&mut self) {
        self.cached_wallpaper = self.appearance.wallpaper.as_ref().and_then(ferro_gfx::DecodedImage::load_from_file);
        self.update_desktop_bg();
    }

    fn paint_full_desktop(&mut self) -> Res<()> {
        let (w, h) = (self.screen_w, self.screen_h);
        self.desktop_surface.copy_from(&self.desktop_bg, Rect::new(0, 0, w, h));
        for (i, item) in self.desktop.items.iter().enumerate() {
            let selected = self.desktop.selected == Some(i);
            let hovered = self.desktop.hovered == Some(i);
            desktop::draw_item(&mut self.desktop_surface, item, selected, hovered);
        }
        self.blit(self.root, &self.desktop_surface, w, (0, 0, w, h))
    }

    fn paint_desktop_rect(&mut self, rect: Rect) -> Res<()> {
        let (w, h) = (self.screen_w, self.screen_h);
        let r = rect.intersect(&Rect::new(0, 0, w, h));
        if r.w <= 0 || r.h <= 0 {
            return Ok(());
        }
        self.desktop_surface.copy_from(&self.desktop_bg, r);
        for (i, item) in self.desktop.items.iter().enumerate() {
            if item.rect().intersect(&r).w > 0 && item.rect().intersect(&r).h > 0 {
                let selected = self.desktop.selected == Some(i);
                let hovered = self.desktop.hovered == Some(i);
                desktop::draw_item(&mut self.desktop_surface, item, selected, hovered);
            }
        }
        self.blit_at(self.root, &self.desktop_surface, w, (r.x, r.y), (r.x, r.y), (r.w, r.h))
    }

    fn paint_desktop(&mut self) -> Res<()> {
        let (w, h) = (self.screen_w, self.screen_h);
        self.desktop.refresh(w, h);
        self.paint_full_desktop()
    }

    fn paint_desktop_surface(&mut self) -> Res<()> {
        self.paint_full_desktop()
    }

    fn place_move(&mut self, frame_win: Window, x: i32, y: i32) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        (m.x, m.y) = (x, y);
        self.conn.configure_window(frame_win, &ConfigureWindowAux::new().x(x).y(y))?;
        Ok(())
    }

    fn get_apps(&mut self) -> &[apps::App] {
        let now = std::time::Instant::now();
        let stale = match self.app_cache_time {
            Some(t) => now.duration_since(t).as_secs() > 30,
            None => true,
        };
        if stale || self.app_cache.is_none() {
            self.app_cache = Some(apps::installed(&app_dirs()));
            self.app_cache_time = Some(now);
        }
        self.app_cache.as_deref().unwrap_or(&[])
    }

    fn paint_menu(&self) -> Res<()> {
        let Some(menu) = &self.menu else { return Ok(()) };
        let items = menu.items();
        let mut s = Surface::new(menu.layout.width as usize, menu.layout.height as usize);
        bar::draw_menu_with_layout(&mut s, &items, menu.hover, menu.with_band, &menu.layout);
        self.blit(menu.window, &s, menu.layout.width, (0, 0, menu.layout.width, menu.layout.height))
    }

    fn paint_submenu(&self) -> Res<()> {
        let Some(sub) = &self.submenu else { return Ok(()) };
        let items = sub.items();
        let mut s = Surface::new(sub.layout.width as usize, sub.layout.height as usize);
        bar::draw_menu_with_layout(&mut s, &items, sub.hover, false, &sub.layout);
        self.blit(sub.window, &s, sub.layout.width, (0, 0, sub.layout.width, sub.layout.height))
    }

    fn paint_nested_submenu(&self) -> Res<()> {
        let Some(nested) = &self.nested_submenu else { return Ok(()) };
        let items = nested.items();
        let mut s = Surface::new(nested.layout.width as usize, nested.layout.height as usize);
        bar::draw_menu_with_layout(&mut s, &items, nested.hover, false, &nested.layout);
        self.blit(nested.window, &s, nested.layout.width, (0, 0, nested.layout.width, nested.layout.height))
    }

    /// Opens the classic Windows 95 / Lite Start menu: Programs, Documents, Settings,
    /// Help, Run, Log Off, Shut Down.
    fn open_menu(&mut self) -> Res<()> {
        let choices = vec![
            Choice::Submenu(SubmenuKind::Programs),
            Choice::Action {
                label: "Documents",
                command: "sh -c 'mkdir -p \"$HOME/Documents\" && ferro-files \"$HOME/Documents\"'".into(),
                icon: Some(icons::FILE),
            },
            Choice::Submenu(SubmenuKind::Settings),
            Choice::Action {
                label: "Help",
                command: "sh -c 'xterm -T \"Help\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e sh -c \"ferro-heavy status; read -p \\\"Press Enter to close...\\\"\" || x-terminal-emulator -e sh -c \"ferro-heavy status; read -p \\\"Press Enter to close...\\\"\" || ferro-cmd'".into(),
                icon: Some(icons::HELP),
            },
            Choice::Action {
                label: "Run...",
                command: "sh -c 'xterm -T \"Command Prompt\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e ferro-cmd || x-terminal-emulator || ferro-cmd'".into(),
                icon: Some(icons::TERMINAL),
            },
            Choice::Separator,
            Choice::LogOff,
            Choice::Standby,
            Choice::Restart,
            Choice::ShutDown,
        ];
        self.spawn_menu(0, None, choices, true, bar::START_MENU_W, None)
    }

    fn open_submenu(&mut self, kind: SubmenuKind, parent_item_idx: usize) -> Res<()> {
        let (menu_x, menu_w, menu_y, top) = {
            let Some(menu) = &self.menu else { return Ok(()) };
            let items = menu.items();
            let mut top = 3;
            for item in items.iter().take(parent_item_idx) {
                top += bar::item_height(item, menu.with_band);
            }
            (menu.x, menu.width, menu.y, top)
        };
        let sub_choices = match kind {
            SubmenuKind::View => vec![
                Choice::ToggleAutoArrange(self.desktop.auto_arrange),
                Choice::ToggleAlignToGrid(self.desktop.align_to_grid),
            ],
            SubmenuKind::SortBy => vec![
                Choice::ArrangeIconsByName,
                Choice::ArrangeIconsByType,
            ],
            SubmenuKind::New => vec![
                Choice::Action {
                    label: "Folder",
                    command: "sh -c 'd=\"$HOME/Desktop/New Folder\"; i=1; while [ -e \"$d\" ]; do d=\"$HOME/Desktop/New Folder ($i)\"; i=$((i+1)); done; mkdir -p \"$d\"'".into(),
                    icon: Some(icons::FOLDER),
                },
                Choice::Action {
                    label: "Text Document",
                    command: "sh -c 'f=\"$HOME/Desktop/New Document.txt\"; i=1; while [ -e \"$f\" ]; do f=\"$HOME/Desktop/New Document ($i).txt\"; i=$((i+1)); done; touch \"$f\"'".into(),
                    icon: Some(icons::FILE),
                },
                Choice::Action {
                    label: "Terminal Shortcut",
                    command: "sh -c 's=\"$HOME/Desktop/Terminal.desktop\"; echo -e \"[Desktop Entry]\\nName=Terminal\\nExec=ferro-cmd\\nType=Application\" > \"$s\"'".into(),
                    icon: Some(icons::TERMINAL),
                },
            ],
            SubmenuKind::Programs => {
                let progs = self.get_apps().to_vec();
                let mut choices = Vec::new();
                for &cat in apps::CATEGORIES {
                    if progs.iter().any(|a| a.category == cat) {
                        choices.push(Choice::Submenu(SubmenuKind::Category(cat.to_string())));
                    }
                }
                if !choices.is_empty() {
                    choices.push(Choice::Separator);
                }
                choices.push(Choice::Action {
                    label: "Command Prompt",
                    command: "sh -c 'xterm -T \"Command Prompt\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e ferro-cmd || x-terminal-emulator -e ferro-cmd || ferro-cmd'".into(),
                    icon: Some(icons::TERMINAL),
                });
                choices.push(Choice::Action { label: "Files", command: "ferro-files".into(), icon: Some(icons::FOLDER) });
                choices.push(Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) });
                choices
            }
            SubmenuKind::Settings => vec![
                Choice::Action { label: "Network & Internet...", command: "ferro-network".into(), icon: Some(icons::NETWORK) },
                Choice::Action { label: "Appearance & Display...", command: "ferro-appearance".into(), icon: Some(icons::COMPUTER) },
                Choice::Action { label: "File Associations...", command: "ferro-associations".into(), icon: Some(icons::FILE) },
                Choice::Action { label: "Date & Time...", command: "ferro-datetime".into(), icon: Some(icons::CLOCK) },
                Choice::Action { label: "Volume Control...", command: "ferro-volume".into(), icon: Some(icons::SPEAKER) },
                Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) },
                Choice::Action {
                    label: "Add Programs & Drivers...",
                    command: "sh -c 'xterm -T \"Add Programs\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e ferro-heavy || x-terminal-emulator -e ferro-heavy || ferro-cmd'".into(),
                    icon: Some(icons::DRIVE),
                },
            ],
            SubmenuKind::Category(_) => return Ok(()),
        };
        let dummy = Menu {
            window: 0,
            x: 0,
            y: 0,
            choices: sub_choices.clone(),
            hover: None,
            with_band: false,
            width: 0,
            layout: bar::MenuLayout::default(),
            kind: Some(kind.clone()),
        };
        let items = dummy.items();
        let layout = bar::menu_layout(&items, false, Some(self.work_h() - 20));
        let sub_w = layout.width;
        let sub_h = layout.height;
        let sub_x = (menu_x + menu_w - 2).clamp(0, (self.screen_w - sub_w).max(0));
        let sub_y = (menu_y + top).clamp(10, (self.work_h() - sub_h).max(0));

        self.close_submenu()?;
        let window = self.conn.generate_id()?;
        self.conn.create_window(
            self.depth,
            window,
            self.root,
            sub_x as i16,
            sub_y as i16,
            sub_w as u16,
            sub_h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(EventMask::EXPOSURE),
        )?;
        self.conn.map_window(window)?;
        self.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;

        let submenu =
            Menu { window, x: sub_x, y: sub_y, choices: sub_choices, hover: None, with_band: false, width: sub_w, layout, kind: Some(kind) };
        self.submenu = Some(submenu);
        self.paint_submenu()?;
        Ok(())
    }

    fn open_nested_submenu(&mut self, kind: SubmenuKind, parent_item_idx: usize) -> Res<()> {
        let (parent_x, parent_w, parent_y, top) = {
            let Some(parent) = &self.submenu else { return Ok(()) };
            let items = parent.items();
            let top = if parent.layout.cols > 1 {
                let row = parent_item_idx % parent.layout.items_per_col;
                3 + row as i32 * bar::CONTEXT_ITEM_H
            } else {
                let mut t = 3;
                for item in items.iter().take(parent_item_idx) {
                    t += bar::item_height(item, false);
                }
                t
            };
            (parent.x, parent.width, parent.y, top)
        };

        let nested_choices = match &kind {
            SubmenuKind::Category(cat) => {
                let progs = self.get_apps().to_vec();
                let choices: Vec<Choice> = progs.into_iter().filter(|a| &a.category == cat).map(Choice::Run).collect();
                if choices.is_empty() {
                    return Ok(());
                }
                choices
            }
            _ => return Ok(()),
        };

        let nested_items: Vec<Item<'_>> = nested_choices
            .iter()
            .map(|c| match c {
                Choice::Run(app) => {
                    let icon = if app.terminal {
                        Some(icons::TERMINAL)
                    } else if app.exec.contains("file") || app.name.contains("File") {
                        Some(icons::FOLDER)
                    } else {
                        Some(icons::FILE)
                    };
                    Item::entry(&app.name, icon, false)
                }
                Choice::Action { label, icon, .. } => Item::entry(label, *icon, false),
                Choice::Separator => Item::Separator,
                _ => Item::Separator,
            })
            .collect();

        let layout = bar::menu_layout(&nested_items, false, Some(self.work_h() - 20));
        let nested_w = layout.width;
        let nested_h = layout.height;

        let right_x = parent_x + parent_w - 2;
        let nested_x = if right_x + nested_w <= self.screen_w { right_x } else { (parent_x - nested_w + 2).max(0) };
        let nested_y = (parent_y + top).clamp(10, (self.work_h() - nested_h).max(0));

        self.close_nested_submenu()?;
        let window = self.conn.generate_id()?;
        self.conn.create_window(
            self.depth,
            window,
            self.root,
            nested_x as i16,
            nested_y as i16,
            nested_w as u16,
            nested_h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(EventMask::EXPOSURE),
        )?;
        self.conn.map_window(window)?;
        self.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;

        let nested_menu = Menu {
            window,
            x: nested_x,
            y: nested_y,
            choices: nested_choices,
            hover: None,
            with_band: false,
            width: nested_w,
            layout,
            kind: Some(kind),
        };
        self.nested_submenu = Some(nested_menu);
        self.paint_nested_submenu()?;
        Ok(())
    }

    fn open_desktop_menu(&mut self, x: i32, y: i32, hit: Option<usize>) -> Res<()> {
        let choices = match hit {
            Some(idx) => {
                let item = &self.desktop.items[idx];
                match &item.kind {
                    desktop::ItemKind::Files => vec![
                        Choice::Action { label: "Open", command: "ferro-files".into(), icon: Some(icons::FOLDER) },
                        Choice::PinTaskbar { name: "Files".into(), exec: "ferro-files".into() },
                    ],
                    desktop::ItemKind::RecycleBin => vec![
                        Choice::Action {
                            label: "Open",
                            command: "sh -c 'mkdir -p \"$HOME/.local/share/Trash\" && ferro-files \"$HOME/.local/share/Trash\"'".into(),
                            icon: Some(icons::RECYCLE),
                        },
                        Choice::Action {
                            label: "Empty Recycle Bin",
                            command: "sh -c 'rm -rf \"$HOME/.local/share/Trash\"/* \"$HOME/.local/share/Trash\"/.* 2>/dev/null; true'".into(),
                            icon: None,
                        },
                    ],
                    desktop::ItemKind::Shortcut { exec, .. } => vec![
                        Choice::Action { label: "Open", command: exec.clone(), icon: Some(item.icon) },
                        Choice::PinTaskbar { name: item.label.clone(), exec: exec.clone() },
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                    ],
                    desktop::ItemKind::Folder(path) => vec![
                        Choice::Action {
                            label: "Open",
                            command: format!("ferro-files \"{}\"", path.display()),
                            icon: Some(icons::FOLDER),
                        },
                        Choice::PinTaskbar { name: item.label.clone(), exec: format!("ferro-files \"{}\"", path.display()) },
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                    ],
                    desktop::ItemKind::File(path) => vec![
                        Choice::Action {
                            label: "Open",
                            command: ferro_associations::resolve_command_for_path(path),
                            icon: Some(icons::FILE),
                        },
                        Choice::PinTaskbar { name: item.label.clone(), exec: format!("xdg-open \"{}\"", path.display()) },
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                    ],
                }
            }
            None => vec![
                Choice::Submenu(SubmenuKind::View),
                Choice::Submenu(SubmenuKind::SortBy),
                Choice::RefreshDesktop,
                Choice::Separator,
                Choice::Submenu(SubmenuKind::New),
                Choice::Separator,
                Choice::Action {
                    label: "Display settings",
                    command: "ferro-appearance".into(),
                    icon: Some(icons::COMPUTER),
                },
                Choice::Action {
                    label: "Open in Terminal",
                    command: "sh -c 'mkdir -p \"$HOME/Desktop\" && cd \"$HOME/Desktop\" && (xterm -T \"Command Prompt\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e ferro-cmd || x-terminal-emulator || ferro-cmd)'".into(),
                    icon: Some(icons::TERMINAL),
                },
                Choice::Action {
                    label: "Task Manager",
                    command: "ferro-taskmgr".into(),
                    icon: Some(icons::TASKMGR),
                },
            ],
        };
        self.spawn_menu(x, Some(y), choices, false, bar::CONTEXT_MENU_W, None)
    }

    fn spawn_menu(&mut self, x: i32, y: Option<i32>, choices: Vec<Choice>, with_band: bool, width: i32, kind: Option<SubmenuKind>) -> Res<()> {
        self.close_menu()?;
        let window = self.conn.generate_id()?;
        let dummy = Menu {
            window,
            x: 0,
            y: 0,
            choices: choices.clone(),
            hover: None,
            with_band,
            width,
            layout: bar::MenuLayout::default(),
            kind: kind.clone(),
        };
        let items = dummy.items();
        let layout = bar::menu_layout(&items, with_band, Some(self.work_h() - 20));
        let h = layout.height;
        let w = layout.width.max(width);
        let pos_y = y.unwrap_or((self.work_h() - h).max(0)).clamp(0, (self.work_h() - h).max(0));
        let pos_x = x.clamp(0, (self.screen_w - w).max(0));
        let menu = Menu { window, x: pos_x, y: pos_y, choices, hover: None, with_band, width: w, layout, kind };
        self.conn.create_window(
            self.depth,
            window,
            self.root,
            pos_x as i16,
            pos_y as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(EventMask::EXPOSURE),
        )?;
        self.conn.map_window(window)?;
        self.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        let mask = EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
        self.conn.grab_pointer(false, window, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?;
        self.menu = Some(menu);
        self.bar_stale = true;
        self.paint_menu()?;
        Ok(())
    }

    fn close_nested_submenu(&mut self) -> Res<()> {
        if let Some(nested) = self.nested_submenu.take() {
            let _ = self.conn.destroy_window(nested.window);
        }
        Ok(())
    }

    fn close_submenu(&mut self) -> Res<()> {
        self.close_nested_submenu()?;
        if let Some(sub) = self.submenu.take() {
            let _ = self.conn.destroy_window(sub.window);
        }
        Ok(())
    }

    fn close_menu(&mut self) -> Res<()> {
        self.close_submenu()?;
        if let Some(menu) = self.menu.take() {
            let _ = self.conn.ungrab_pointer(CURRENT_TIME);
            let _ = self.conn.destroy_window(menu.window);
            self.bar_stale = true;
        }
        Ok(())
    }

    /// A click while a menu is open.
    fn menu_click(&mut self, rx: i32, ry: i32, detail: u8) -> Res<()> {
        let Some(menu) = &self.menu else { return Ok(()) };

        // 1. Check click in nested submenu if open
        if let Some(nested) = &self.nested_submenu {
            if (nested.x..nested.x + nested.width).contains(&rx) && (nested.y..nested.y + nested.layout.height).contains(&ry) {
                let chosen = bar::menu_hit_with_layout(&nested.items(), rx - nested.x, ry - nested.y, false, &nested.layout);
                if let Some(i) = chosen {
                    if detail == 3 {
                        let ch = nested.choices[i].clone();
                        return self.open_app_context_menu(rx, ry, ch);
                    }
                    let command = match &nested.choices[i] {
                        Choice::Run(app) if app.terminal => Some(format!("x-terminal-emulator -e {}", app.exec)),
                        Choice::Run(app) => Some(app.exec.clone()),
                        Choice::Action { command, .. } => Some(command.clone()),
                        _ => None,
                    };
                    self.close_menu()?;
                    if let Some(command) = command {
                        launch(&command);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        self.paint_desktop()?;
                    }
                    return Ok(());
                }
            }
        }

        // 2. Check click in submenu if open
        if let Some(sub) = &self.submenu {
            if (sub.x..sub.x + sub.width).contains(&rx) && (sub.y..sub.y + sub.layout.height).contains(&ry) {
                let chosen = bar::menu_hit_with_layout(&sub.items(), rx - sub.x, ry - sub.y, false, &sub.layout);
                if let Some(i) = chosen {
                    if detail == 3 {
                        let ch = sub.choices[i].clone();
                        return self.open_app_context_menu(rx, ry, ch);
                    }
                    if let Choice::Submenu(kind) = &sub.choices[i] {
                        let k = kind.clone();
                        self.open_nested_submenu(k, i)?;
                        return Ok(());
                    }
                    match &sub.choices[i] {
                        Choice::ToggleAutoArrange(_) => {
                            self.desktop.auto_arrange = !self.desktop.auto_arrange;
                            if self.desktop.auto_arrange {
                                self.desktop.realign_grid(self.screen_h);
                            }
                            self.desktop.save_positions();
                            self.close_menu()?;
                            self.paint_full_desktop()?;
                            return Ok(());
                        }
                        Choice::ToggleAlignToGrid(_) => {
                            self.desktop.align_to_grid = !self.desktop.align_to_grid;
                            if self.desktop.align_to_grid {
                                self.desktop.realign_grid(self.screen_h);
                            }
                            self.desktop.save_positions();
                            self.close_menu()?;
                            self.paint_full_desktop()?;
                            return Ok(());
                        }
                        Choice::ArrangeIconsByName => {
                            self.desktop.arrange_by_name(self.screen_w, self.screen_h);
                            self.close_menu()?;
                            self.paint_full_desktop()?;
                            return Ok(());
                        }
                        Choice::ArrangeIconsByType => {
                            self.desktop.arrange_by_type(self.screen_w, self.screen_h);
                            self.close_menu()?;
                            self.paint_full_desktop()?;
                            return Ok(());
                        }
                        Choice::AlignToGrid => {
                            self.desktop.align_to_grid(self.screen_w, self.screen_h);
                            self.close_menu()?;
                            self.paint_full_desktop()?;
                            return Ok(());
                        }
                        Choice::RefreshDesktop => {
                            self.close_menu()?;
                            self.paint_desktop()?;
                            return Ok(());
                        }
                        _ => {}
                    }
                    let command = match &sub.choices[i] {
                        Choice::Run(app) if app.terminal => Some(format!("x-terminal-emulator -e {}", app.exec)),
                        Choice::Run(app) => Some(app.exec.clone()),
                        Choice::Action { command, .. } => Some(command.clone()),
                        _ => None,
                    };
                    self.close_menu()?;
                    if let Some(command) = command {
                        launch(&command);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        self.paint_desktop()?;
                    }
                    return Ok(());
                }
            }
        }

        // 3. Check click in main menu
        let menu_h = menu.layout.height;
        if (menu.x..menu.x + menu.width).contains(&rx) && (menu.y..menu.y + menu_h).contains(&ry) {
            let chosen = bar::menu_hit_with_layout(&menu.items(), rx - menu.x, ry - menu.y, menu.with_band, &menu.layout);
            if chosen.is_some_and(|i| matches!(menu.choices[i], Choice::LogOff)) {
                std::process::exit(0);
            }
            if let Some(i) = chosen {
                if detail == 3 {
                    match &menu.choices[i] {
                        Choice::Run(_) | Choice::Action { .. } => {
                            let ch = menu.choices[i].clone();
                            return self.open_app_context_menu(rx, ry, ch);
                        }
                        _ => {}
                    }
                }
                if let Choice::RefreshDesktop = menu.choices[i] {
                    self.close_menu()?;
                    self.paint_desktop()?;
                    return Ok(());
                }
                if let Choice::ToggleAutoArrange(_) = menu.choices[i] {
                    self.desktop.auto_arrange = !self.desktop.auto_arrange;
                    if self.desktop.auto_arrange {
                        self.desktop.realign_grid(self.screen_h);
                    }
                    self.desktop.save_positions();
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::ToggleAlignToGrid(_) = menu.choices[i] {
                    self.desktop.align_to_grid = !self.desktop.align_to_grid;
                    if self.desktop.align_to_grid {
                        self.desktop.realign_grid(self.screen_h);
                    }
                    self.desktop.save_positions();
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::CreateDesktopShortcut { name, exec } = &menu.choices[i] {
                    let desk_dir = std::env::var("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| std::path::PathBuf::from("/home/ferro"))
                        .join("Desktop");
                    let _ = std::fs::create_dir_all(&desk_dir);
                    let path = desk_dir.join(format!("{name}.desktop"));
                    let content = format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nIcon=system-run\n");
                    let _ = std::fs::write(path, content);
                    self.close_menu()?;
                    self.paint_desktop()?;
                    return Ok(());
                }
                if let Choice::PinTaskbar { name, exec } = &menu.choices[i] {
                    let icon = if exec.contains("cmd") || exec.contains("term") {
                        icons::TERMINAL
                    } else if exec.contains("file") {
                        icons::FOLDER
                    } else if exec.contains("network") {
                        icons::NETWORK
                    } else if exec.contains("taskmgr") {
                        icons::TASKMGR
                    } else {
                        icons::COMPUTER
                    };
                    self.pinned_apps.push(bar::PinnedApp { name: name.clone(), exec: exec.clone(), icon });
                    bar::save_pinned(&self.pinned_apps);
                    self.bar_stale = true;
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::Submenu(kind) = &menu.choices[i] {
                    let k = kind.clone();
                    self.open_submenu(k, i)?;
                    return Ok(());
                }
                if let Choice::DeleteDesktopItem(idx) = menu.choices[i] {
                    self.desktop.delete_item(idx);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::RenameDesktopItem(idx) = menu.choices[i] {
                    if let Some(item) = self.desktop.items.get(idx) {
                        let name = item.id.clone();
                        let cmd = format!(
                            "xterm -T \"Rename: {name}\" -bg \"#000000\" -fg \"#FFFFFF\" -geometry 50x4 -e sh -c '\
                            echo \"Enter new name for {name}:\"; \
                            read -r new_name; \
                            if [ -n \"$new_name\" ]; then \
                                mv \"$HOME/Desktop/{name}\" \"$HOME/Desktop/$new_name\" 2>/dev/null; \
                            fi'"
                        );
                        launch(&cmd);
                    }
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::ArrangeIconsByName = menu.choices[i] {
                    self.desktop.arrange_by_name(self.screen_w, self.screen_h);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::ArrangeIconsByType = menu.choices[i] {
                    self.desktop.arrange_by_type(self.screen_w, self.screen_h);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::AlignToGrid = menu.choices[i] {
                    self.desktop.align_to_grid(self.screen_w, self.screen_h);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
            }
            let command = chosen.and_then(|i| match &menu.choices[i] {
                Choice::Run(app) if app.terminal => Some(format!("x-terminal-emulator -e {}", app.exec)),
                Choice::Run(app) => Some(app.exec.clone()),
                Choice::Action { command, .. } => Some(command.clone()),
                Choice::Standby => Some("systemctl suspend || sudo -n systemctl suspend || sudo -n zzz".to_owned()),
                Choice::ShutDown => {
                    Some("systemctl poweroff || sudo -n systemctl poweroff || sudo -n shutdown -h now || sudo -n poweroff".to_owned())
                }
                Choice::Restart => Some("systemctl reboot || sudo -n systemctl reboot || sudo -n shutdown -r now || sudo -n reboot".to_owned()),
                _ => None,
            });
            self.close_menu()?;
            if let Some(command) = command {
                launch(&command);
            }
            self.paint_desktop()?;
            return Ok(());
        }

        // 4. Clicked outside all menus
        self.close_menu()?;
        Ok(())
    }

    fn open_app_context_menu(&mut self, x: i32, y: i32, choice: Choice) -> Res<()> {
        let (name, exec, is_app) = match &choice {
            Choice::Run(app) => (app.name.clone(), app.exec.clone(), true),
            Choice::Action { label, command, .. } => (label.to_string(), command.clone(), false),
            _ => return Ok(()),
        };

        let mut choices = vec![
            Choice::Action { label: "Open", command: exec.clone(), icon: Some(icons::FERRO) },
            Choice::PinTaskbar { name: name.clone(), exec: exec.clone() },
            Choice::CreateDesktopShortcut { name: name.clone(), exec: exec.clone() },
        ];

        if is_app {
            choices.push(Choice::Separator);
            let first_bin = exec.split_whitespace().next().unwrap_or(&exec).to_string();
            let uninst_cmd = format!(
                "sh -c 'xterm -T \"Uninstall: {name}\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e sh -c \"\
                echo \\\"=== Uninstalling {name} ===\\\"; \
                BIN=\\$(which {first_bin} 2>/dev/null); \
                PKG=\\$(dpkg-query -S \\\"\\$BIN\\\" 2>/dev/null | cut -d: -f1 | head -n1); \
                if [ -z \\\"\\$PKG\\\" ]; then PKG=\\\"{first_bin}\\\"; fi; \
                echo \\\"Package identified: \\$PKG\\\"; \
                echo \\\"Running sudo apt remove --purge \\$PKG...\\\"; \
                sudo apt remove --purge -y \\\"\\$PKG\\\" || sudo apt remove -y \\\"\\$PKG\\\"; \
                rm -f \\\"\\$HOME/.local/share/applications/{name}.desktop\\\" \\\"\\$HOME/Desktop/{name}.desktop\\\"; \
                echo \\\"Done.\\\"; echo \\\"Press Enter to close...\\\"; read -r _\
                \" || ferro-cmd'"
            );
            choices.push(Choice::Action { label: "Uninstall...", command: uninst_cmd, icon: Some(icons::RECYCLE) });
        }

        self.spawn_menu(x, Some(y), choices, false, bar::CONTEXT_MENU_W, None)
    }

    fn toggle_switch(&mut self, i: usize) -> Res<()> {
        match i {
            0 => self.store.switches.network = !self.store.switches.network,
            1 => self.store.switches.microphone = !self.store.switches.microphone,
            2 => self.store.switches.camera = !self.store.switches.camera,
            _ => return Ok(()),
        }
        let _ = self.store.save();
        send_switches(self.store.switches);
        self.paint_bar()?;
        Ok(())
    }

    fn bar_click(&mut self, x: i32, y: i32, detail: u8) -> Res<()> {
        match bar::hit(self.screen_w, self.tasks.len(), self.pinned_apps.len(), x, y) {
            BarPart::Start => self.open_menu()?,
            BarPart::Pinned(i) => {
                if let Some(app) = self.pinned_apps.get(i) {
                    launch(&app.exec);
                }
            }
            BarPart::TrayIcon(0) => launch("ferro-network"),
            BarPart::TrayIcon(1) => self.toggle_switch(1)?,
            BarPart::TrayIcon(2) => self.toggle_switch(2)?,
            BarPart::TrayIcon(3) => {
                if detail == 3 {
                    launch("amixer -q sset Master toggle || pactl set-sink-mute @DEFAULT_SINK@ toggle");
                } else {
                    launch("ferro-volume");
                }
            }
            BarPart::TrayIcon(_) => {}
            BarPart::Tray => {
                let now = std::time::Instant::now();
                if let Some(prev) = self.last_clock_click {
                    if now.duration_since(prev).as_millis() < 400 {
                        self.last_clock_click = None;
                        launch("ferro-datetime");
                        return Ok(());
                    }
                }
                self.last_clock_click = Some(now);
                launch("ferro-taskmgr");
            }
            BarPart::Task(i) => {
                let f = self.tasks[i];
                let Some(m) = self.windows.get(&f) else { return Ok(()) };
                // As in Windows: the button of the window in front puts it
                // away; any other button brings its window forward.
                if m.minimized {
                    self.restore(f)?;
                } else if self.active == Some(f) {
                    self.minimize(f)?;
                } else {
                    self.activate(f)?;
                }
            }
            BarPart::Nothing => {}
        }
        Ok(())
    }

    /// Tells programs what this window manager understands.
    fn announce(&self) -> Res<()> {
        let a = &self.atoms;
        let supported = [
            a.net_wm_name,
            a.net_wm_state,
            a.net_wm_state_fullscreen,
            a.net_active_window,
            a.net_client_list,
            a.net_wm_window_type,
            a.net_wm_window_type_dock,
            a.net_wm_window_type_desktop,
            a.net_wm_window_type_dialog,
            a.net_wm_window_type_menu,
            a.net_wm_window_type_popup_menu,
            a.net_wm_window_type_dropdown_menu,
            a.net_wm_window_type_tooltip,
            a.net_wm_window_type_notification,
            a.net_wm_window_type_combo,
            a.net_wm_window_type_dnd,
            a.net_wm_window_type_utility,
            a.net_wm_window_type_splash,
        ];
        self.conn.change_property32(PropMode::REPLACE, self.root, a.net_supported, AtomEnum::ATOM, &supported)?;
        Ok(())
    }

    fn by_client(&self, client: Window) -> Option<Window> {
        self.windows.values().find(|m| m.client == client).map(|m| m.frame)
    }

    fn title_of(&self, client: Window) -> String {
        let read = |prop: Atom, kind: Atom| -> Option<String> {
            let r = self.conn.get_property(false, client, prop, kind, 0, 256).ok()?.reply().ok()?;
            (!r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).into_owned())
        };
        read(self.atoms.net_wm_name, self.atoms.utf8_string).or_else(|| read(AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())).unwrap_or_default()
    }

    fn class_of(&self, client: Window) -> String {
        if let Ok(prop) = self.conn.get_property(false, client, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 128) {
            if let Ok(reply) = prop.reply() {
                if let Ok(s) = String::from_utf8(reply.value) {
                    if let Some(first) = s.split('\0').next() {
                        return first.to_string();
                    }
                }
            }
        }
        String::new()
    }

    fn atom_list(&self, win: Window, prop: Atom) -> Vec<Atom> {
        self.conn
            .get_property(false, win, prop, AtomEnum::ATOM, 0, 32)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    /// Puts a frame around `client`.
    fn manage(&mut self, client: Window, already_mapped: bool) -> Res<()> {
        if self.by_client(client).is_some() || self.windows.contains_key(&client) {
            return Ok(());
        }
        if let Ok(cookie) = self.conn.get_window_attributes(client) {
            if let Ok(attrs) = cookie.reply() {
                if attrs.override_redirect {
                    self.conn.map_window(client)?;
                    return Ok(());
                }
            }
        }
        // Taskbars, desktops, and popup menus / dropdowns / tooltips / notifications: no frame.
        let kinds = self.atom_list(client, self.atoms.net_wm_window_type);
        if kinds.contains(&self.atoms.net_wm_window_type_dock)
            || kinds.contains(&self.atoms.net_wm_window_type_desktop)
            || kinds.contains(&self.atoms.net_wm_window_type_popup_menu)
            || kinds.contains(&self.atoms.net_wm_window_type_dropdown_menu)
            || kinds.contains(&self.atoms.net_wm_window_type_menu)
            || kinds.contains(&self.atoms.net_wm_window_type_tooltip)
            || kinds.contains(&self.atoms.net_wm_window_type_notification)
            || kinds.contains(&self.atoms.net_wm_window_type_combo)
            || kinds.contains(&self.atoms.net_wm_window_type_dnd)
        {
            self.conn.map_window(client)?;
            return Ok(());
        }

        let transient_for = self
            .conn
            .get_property(false, client, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        let is_dialog = transient_for.is_some()
            || kinds.contains(&self.atoms.net_wm_window_type_dialog)
            || kinds.contains(&self.atoms.net_wm_window_type_utility)
            || kinds.contains(&self.atoms.net_wm_window_type_splash);

        let class = self.class_of(client);
        let geo = self.conn.get_geometry(client)?.reply()?;

        let ((w, h), (mut x, mut y)) = if !is_dialog {
            let (saved_x, saved_y, saved_w, saved_h) = self.window_store.get(&class).unwrap_or((0, 0, 0, 0));
            let (w, h) =
                if saved_w >= 100 && saved_h >= 100 { (saved_w, saved_h) } else { frame::outer(i32::from(geo.width), i32::from(geo.height)) };
            let (x, y) = if saved_w >= 100 && saved_h >= 100 {
                (saved_x, saved_y)
            } else {
                let n = self.windows.len() as i32 % 8;
                let (mut x, mut y) = (i32::from(geo.x), i32::from(geo.y));
                if x <= 0 && y <= 0 {
                    (x, y) = (32 + n * 24, 32 + n * 24);
                }
                (x, y)
            };
            ((w, h), (x, y))
        } else {
            let (w, h) = frame::outer(i32::from(geo.width).max(100), i32::from(geo.height).max(80));
            let (mut x, mut y) = (i32::from(geo.x), i32::from(geo.y));
            if let Some(parent_client) = transient_for {
                if let Some(parent_frame) = self.by_client(parent_client) {
                    if let Some(parent) = self.windows.get(&parent_frame) {
                        x = parent.x + (parent.w - w) / 2;
                        y = parent.y + (parent.h - h) / 2;
                    }
                }
            } else if x <= 0 && y <= 0 {
                x = (self.screen_w - w) / 2;
                y = (self.work_h() - h) / 2;
            }
            ((w, h), (x, y))
        };
        x = x.clamp(0, (self.screen_w - 64).max(0));
        y = y.clamp(0, (self.work_h() - 64).max(0));

        let frame_win = self.conn.generate_id()?;
        let events = EventMask::SUBSTRUCTURE_REDIRECT
            | EventMask::SUBSTRUCTURE_NOTIFY
            | EventMask::EXPOSURE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::BUTTON_MOTION;
        self.conn.create_window(
            self.depth,
            frame_win,
            self.root,
            x as i16,
            y as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(events),
        )?;
        // If we die, the program's window goes back to the screen, not with us.
        self.conn.change_save_set(SetMode::INSERT, client)?;
        let (cx, cy, cw, ch) = (frame::LEFT, frame::TOP, frame::inner(w, h).0, frame::inner(w, h).1);
        self.conn.configure_window(client, &ConfigureWindowAux::new().x(cx).y(cy).width(cw as u32).height(ch as u32).border_width(0))?;
        self.conn.reparent_window(client, frame_win, cx as i16, cy as i16)?;
        self.conn.change_window_attributes(
            client,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        // A click inside the program brings its window forward; the click
        // then goes on to the program. Only left-click: right-click and the
        // wheel pass straight through to the program.
        self.conn.grab_button(false, client, EventMask::BUTTON_PRESS, GrabMode::SYNC, GrabMode::ASYNC, NONE, NONE, ButtonIndex::M1, ModMask::ANY)?;
        self.conn.map_window(client)?;
        self.conn.map_window(frame_win)?;
        self.conn.change_property32(PropMode::REPLACE, client, self.atoms.wm_state, self.atoms.wm_state, &[1, NONE])?;

        // Inform the client of its root position per ICCCM 4.1.5
        let note = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: client,
            window: client,
            above_sibling: NONE,
            x: (x + cx) as i16,
            y: (y + cy) as i16,
            width: cw as u16,
            height: ch as u16,
            border_width: 0,
            override_redirect: false,
        };
        self.conn.send_event(false, client, EventMask::STRUCTURE_NOTIFY, note)?;

        let title = self.title_of(client);
        let managed = Managed {
            client,
            frame: frame_win,
            class,
            x,
            y,
            w,
            h,
            title,
            restore: None,
            maximized: false,
            fullscreen: false,
            minimized: false,
            is_dialog,
            // Reparenting a window that's already shown unmaps it once.
            ignore_unmaps: u32::from(already_mapped),
        };
        self.windows.insert(frame_win, managed);
        self.order.push(frame_win);
        self.tasks.push(frame_win);
        self.publish_clients()?;
        if self.atom_list(client, self.atoms.net_wm_state).contains(&self.atoms.net_wm_state_fullscreen) {
            self.set_fullscreen(frame_win, true)?;
        }
        self.activate(frame_win)
    }

    fn publish_clients(&self) -> Res<()> {
        let clients: Vec<Window> = self.order.iter().filter_map(|f| self.windows.get(f)).map(|m| m.client).collect();
        self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms.net_client_list, AtomEnum::WINDOW, &clients)?;
        Ok(())
    }

    /// Forgets a window whose program closed it (or went away).
    fn unmanage(&mut self, frame_win: Window, client_gone: bool) -> Res<()> {
        let Some(m) = self.windows.remove(&frame_win) else { return Ok(()) };
        self.order.retain(|f| *f != frame_win);
        self.tasks.retain(|f| *f != frame_win);
        self.bar_stale = true;
        if !client_gone {
            let _ = self.conn.reparent_window(m.client, self.root, m.x as i16, m.y as i16);
            let _ = self.conn.change_save_set(SetMode::DELETE, m.client);
        }
        self.conn.destroy_window(frame_win)?;
        self.publish_clients()?;
        if self.active == Some(frame_win) {
            self.active = None;
            let next = self.order.iter().rev().copied().find(|f| self.windows.get(f).is_some_and(|m| !m.minimized));
            match next {
                Some(next) => self.activate(next)?,
                None => {
                    self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME)?;
                    self.conn.delete_property(self.root, self.atoms.net_active_window)?;
                }
            }
        }
        Ok(())
    }

    /// Brings a window to the front and gives it the keyboard.
    fn activate(&mut self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get(&frame_win) else { return Ok(()) };
        let (client, fullscreen) = (m.client, m.fullscreen);
        self.order.retain(|f| *f != frame_win);
        self.order.push(frame_win);
        self.conn.configure_window(frame_win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        // The taskbar stays on top of everything but a full-screen program.
        if !fullscreen {
            self.conn.configure_window(self.bar, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        }
        self.bar_stale = true;
        self.conn.set_input_focus(InputFocus::POINTER_ROOT, client, CURRENT_TIME)?;
        self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms.net_active_window, AtomEnum::WINDOW, &[client])?;
        let before = self.active.replace(frame_win);
        if before != Some(frame_win) {
            if let Some(b) = before {
                self.paint(b, None)?;
            }
            self.paint(frame_win, None)?;
        }
        Ok(())
    }

    /// Draws a window's frame: the four strips around the program's area.
    fn paint(&self, frame_win: Window, pressed: Option<Part>) -> Res<()> {
        let Some(m) = self.windows.get(&frame_win) else { return Ok(()) };
        if m.fullscreen || m.minimized {
            return Ok(());
        }
        let (w, h) = (m.w, m.h);
        let mut s = Surface::new(w as usize, h as usize);
        frame::draw(
            &mut s,
            w,
            h,
            &Look { title: &m.title, active: self.active == Some(frame_win), maximized: m.maximized, pressed, dark_mode: self.appearance.dark_mode },
        );
        let middle = h - frame::TOP - frame::BOTTOM;
        for strip in [
            (0, 0, w, frame::TOP),
            (0, h - frame::BOTTOM, w, frame::BOTTOM),
            (0, frame::TOP, frame::LEFT, middle),
            (w - frame::RIGHT, frame::TOP, frame::RIGHT, middle),
        ] {
            self.blit(frame_win, &s, w, strip)?;
        }
        Ok(())
    }

    /// Moves and sizes a frame, and the program's window inside it.
    fn place(&mut self, frame_win: Window, x: i32, y: i32, w: i32, h: i32) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        let (w, h) = if m.fullscreen { (w, h) } else { (w.max(MIN_W), h.max(MIN_H)) };
        let resized = (w, h) != (m.w, m.h);
        (m.x, m.y, m.w, m.h) = (x, y, w, h);
        let (client, fullscreen) = (m.client, m.fullscreen);
        self.conn.configure_window(frame_win, &ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32))?;
        let (cx, cy, cw, ch) = if fullscreen { (0, 0, w, h) } else { (frame::LEFT, frame::TOP, frame::inner(w, h).0, frame::inner(w, h).1) };
        self.conn.configure_window(client, &ConfigureWindowAux::new().x(cx).y(cy).width(cw as u32).height(ch as u32))?;
        // Programs learn where they are on screen from this, since inside
        // the frame they never seem to move.
        let note = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: client,
            window: client,
            above_sibling: NONE,
            x: (x + cx) as i16,
            y: (y + cy) as i16,
            width: cw as u16,
            height: ch as u16,
            border_width: 0,
            override_redirect: false,
        };
        self.conn.send_event(false, client, EventMask::STRUCTURE_NOTIFY, note)?;
        if resized {
            self.paint(frame_win, None)?;
        }
        Ok(())
    }

    fn toggle_maximize(&mut self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        if m.fullscreen {
            return Ok(());
        }
        let to = if m.maximized {
            m.maximized = false;
            m.restore.take().unwrap_or((32, 32, 640, 480))
        } else {
            m.maximized = true;
            m.restore = Some((m.x, m.y, m.w, m.h));
            (0, 0, self.screen_w, self.screen_h - bar::HEIGHT)
        };
        self.place(frame_win, to.0, to.1, to.2, to.3)?;
        self.paint(frame_win, None)
    }

    /// The whole screen, no frame: what games ask for.
    fn set_fullscreen(&mut self, frame_win: Window, on: bool) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        if m.fullscreen == on {
            return Ok(());
        }
        let client = m.client;
        let to = if on {
            if !m.maximized {
                m.restore = Some((m.x, m.y, m.w, m.h));
            }
            m.fullscreen = true;
            (0, 0, self.screen_w, self.screen_h)
        } else {
            m.fullscreen = false;
            m.maximized = false;
            m.restore.take().unwrap_or((32, 32, 640, 480))
        };
        let state: &[Atom] = if on { &[self.atoms.net_wm_state_fullscreen] } else { &[] };
        self.conn.change_property32(PropMode::REPLACE, client, self.atoms.net_wm_state, AtomEnum::ATOM, state)?;
        self.place(frame_win, to.0, to.1, to.2, to.3)?;
        // Over the taskbar while it has the whole screen, under it after.
        let top = if on { frame_win } else { self.bar };
        self.conn.configure_window(top, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        self.paint(frame_win, None)
    }

    fn minimize(&mut self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        m.minimized = true;
        let client = m.client;
        self.bar_stale = true;
        self.conn.unmap_window(frame_win)?;
        self.conn.change_property32(PropMode::REPLACE, client, self.atoms.wm_state, self.atoms.wm_state, &[3, NONE])?;
        if self.active == Some(frame_win) {
            self.active = None;
            if let Some(next) = self.order.iter().rev().copied().find(|f| self.windows.get(f).is_some_and(|m| !m.minimized)) {
                self.activate(next)?;
            } else {
                self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME)?;
                self.conn.delete_property(self.root, self.atoms.net_active_window)?;
            }
        }
        Ok(())
    }

    fn restore(&mut self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        m.minimized = false;
        let client = m.client;
        self.conn.map_window(frame_win)?;
        self.conn.change_property32(PropMode::REPLACE, client, self.atoms.wm_state, self.atoms.wm_state, &[1, NONE])?;
        self.activate(frame_win)
    }

    /// Asks the program to close its window, as its own Close would; a
    /// program that doesn't know how is disconnected.
    fn close(&self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get(&frame_win) else { return Ok(()) };
        if self.atom_list(m.client, self.atoms.wm_protocols).contains(&self.atoms.wm_delete_window) {
            let msg = ClientMessageEvent::new(32, m.client, self.atoms.wm_protocols, [self.atoms.wm_delete_window, CURRENT_TIME, 0, 0, 0]);
            self.conn.send_event(false, m.client, EventMask::NO_EVENT, msg)?;
        } else {
            self.conn.kill_client(m.client)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn handle(&mut self, event: Event) -> Res<()> {
        match event {
            Event::MapRequest(e) => match self.by_client(e.window) {
                Some(f) => self.restore(f)?,
                None => self.manage(e.window, false)?,
            },
            Event::ConfigureRequest(e) => self.configure_request(e)?,
            Event::UnmapNotify(e) => {
                if let Some(f) = self.by_client(e.window) {
                    let m = self.windows.get_mut(&f).unwrap();
                    if m.ignore_unmaps > 0 {
                        m.ignore_unmaps -= 1;
                    } else if !m.minimized {
                        self.unmanage(f, false)?;
                    }
                }
            }
            Event::DestroyNotify(e) => {
                if let Some(f) = self.by_client(e.window) {
                    self.unmanage(f, true)?;
                }
            }
            Event::Expose(e) if e.count == 0 => {
                if e.window == self.bar {
                    self.bar_stale = true;
                } else if e.window == self.root {
                    let rect = Rect::new(e.x as i32, e.y as i32, e.width as i32, e.height as i32);
                    let _ = self.blit_at(self.root, &self.desktop_surface, self.screen_w, (rect.x, rect.y), (rect.x, rect.y), (rect.w, rect.h));
                } else if self.menu.as_ref().is_some_and(|m| m.window == e.window) {
                    self.paint_menu()?;
                } else if self.submenu.as_ref().is_some_and(|s| s.window == e.window) {
                    self.paint_submenu()?;
                } else if self.nested_submenu.as_ref().is_some_and(|s| s.window == e.window) {
                    self.paint_nested_submenu()?;
                } else {
                    self.paint(e.window, None)?;
                }
            }
            Event::PropertyNotify(e) => {
                if e.atom == self.atoms.net_wm_name || e.atom == Atom::from(AtomEnum::WM_NAME) {
                    if let Some(f) = self.by_client(e.window) {
                        let title = self.title_of(e.window);
                        self.windows.get_mut(&f).unwrap().title = title;
                        self.bar_stale = true;
                        self.paint(f, None)?;
                    }
                } else if e.atom == self.atoms.ferro_switches {
                    self.store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
                    self.bar_stale = true;
                } else if e.atom == self.atoms.ferro_appearance {
                    let mut got_prop = false;
                    if let Ok(cookie) = self.conn.get_property(false, self.root, self.atoms.ferro_appearance, AtomEnum::CARDINAL, 0, 2) {
                        if let Ok(reply) = cookie.reply() {
                            if let Some(mut iter) = reply.value32() {
                                if let Some(col) = iter.next() {
                                    self.appearance.desktop_color = col;
                                    got_prop = true;
                                }
                                if let Some(dark) = iter.next() {
                                    self.appearance.dark_mode = dark != 0;
                                    got_prop = true;
                                }
                            }
                        }
                    }
                    if !got_prop {
                        self.appearance = Appearance::load();
                    } else {
                        let app = Appearance::load();
                        self.appearance.wallpaper = app.wallpaper;
                        self.appearance.logon_wallpaper = app.logon_wallpaper;
                    }
                    self.reload_wallpaper();
                    let _ = self
                        .conn
                        .change_window_attributes(self.root, &ChangeWindowAttributesAux::new().background_pixel(self.appearance.desktop_color));
                    let _ = self.conn.clear_area(false, self.root, 0, 0, self.screen_w as u16, self.screen_h as u16);
                    self.paint_desktop()?;
                    let frames: Vec<Window> = self.windows.keys().copied().collect();
                    for f in frames {
                        let _ = self.paint(f, None);
                    }
                    self.bar_stale = true;
                }
            }
            Event::ClientMessage(e) => self.client_message(e)?,
            Event::KeyPress(e) => match shortcut(u16::from(e.state), e.detail) {
                Some(Shortcut::TaskManager) => launch("ferro-taskmgr"),
                Some(Shortcut::StartMenu) if self.menu.is_some() => self.close_menu()?,
                Some(Shortcut::StartMenu) => self.open_menu()?,
                Some(Shortcut::Files) => launch("ferro-files"),
                Some(Shortcut::ShowDesktop) => self.toggle_show_desktop()?,
                Some(Shortcut::LockScreen) => {
                    launch("ferro-screensaver --lock || ferro-logon /usr/local/lib/ferro-heavy/check-password /tmp/lock-unlock || true")
                }
                Some(Shortcut::CloseWindow) => {
                    if let Some(frame_win) = self.active {
                        self.close(frame_win)?;
                    }
                }
                None => {
                    if self.active.is_none() || e.event == self.root {
                        if e.detail == KEY_DELETE {
                            if let Some(sel) = self.desktop.selected {
                                self.desktop.delete_item(sel);
                                self.paint_desktop_surface()?;
                            }
                        } else if e.detail == KEY_RETURN {
                            if let Some(sel) = self.desktop.selected {
                                if let Some(item) = self.desktop.items.get(sel) {
                                    launch(&item.exec_command());
                                }
                            }
                        } else if e.detail == KEY_F2 {
                            if let Some(sel) = self.desktop.selected {
                                if let Some(item) = self.desktop.items.get(sel) {
                                    let name = item.id.clone();
                                    let cmd = format!(
                                        "xterm -T \"Rename: {name}\" -bg \"#000000\" -fg \"#FFFFFF\" -geometry 50x4 -e sh -c '\
                                        echo \"Enter new name for {name}:\"; \
                                        read -r new_name; \
                                        if [ -n \"$new_name\" ]; then \
                                            mv \"$HOME/Desktop/{name}\" \"$HOME/Desktop/$new_name\" 2>/dev/null; \
                                        fi'"
                                    );
                                    launch(&cmd);
                                }
                            }
                        }
                    }
                }
            },
            Event::ButtonPress(e) => self.button_press(e)?,
            Event::MotionNotify(e) => self.motion(e)?,
            Event::ButtonRelease(e) => self.button_release(e)?,
            _ => {}
        }
        Ok(())
    }

    fn configure_request(&mut self, e: ConfigureRequestEvent) -> Res<()> {
        let Some(f) = self.by_client(e.window) else {
            // Not ours (yet): let it have what it asks for.
            let aux = ConfigureWindowAux::from_configure_request(&e);
            self.conn.configure_window(e.window, &aux)?;
            return Ok(());
        };
        let m = &self.windows[&f];
        if m.fullscreen || m.maximized {
            // It stays where it is; tell it so.
            return self.place(f, m.x, m.y, m.w, m.h);
        }
        let (mut x, mut y, mut w, mut h) = (m.x, m.y, m.w, m.h);
        if e.value_mask.contains(ConfigWindow::WIDTH) {
            w = i32::from(e.width) + frame::LEFT + frame::RIGHT;
        }
        if e.value_mask.contains(ConfigWindow::HEIGHT) {
            h = i32::from(e.height) + frame::TOP + frame::BOTTOM;
        }
        if e.value_mask.contains(ConfigWindow::X) {
            x = i32::from(e.x);
        }
        if e.value_mask.contains(ConfigWindow::Y) {
            y = i32::from(e.y);
        }
        self.place(f, x, y, w, h)
    }

    fn client_message(&mut self, e: ClientMessageEvent) -> Res<()> {
        let Some(f) = self.by_client(e.window) else { return Ok(()) };
        let data = e.data.as_data32();
        if e.type_ == self.atoms.net_wm_state && (data[1] == self.atoms.net_wm_state_fullscreen || data[2] == self.atoms.net_wm_state_fullscreen) {
            // 0 = off, 1 = on, 2 = the other one.
            let now = self.windows[&f].fullscreen;
            let on = match data[0] {
                0 => false,
                1 => true,
                _ => !now,
            };
            self.set_fullscreen(f, on)?;
        } else if e.type_ == self.atoms.net_active_window {
            self.restore(f)?;
        }
        Ok(())
    }

    fn button_press(&mut self, e: ButtonPressEvent) -> Res<()> {
        if self.menu.is_some() {
            return self.menu_click(i32::from(e.root_x), i32::from(e.root_y), e.detail);
        }
        if e.event == self.bar {
            return self.bar_click(i32::from(e.event_x), i32::from(e.event_y), e.detail);
        }
        if e.event == self.root {
            return self.root_click(e);
        }
        // A click inside a program: forward it once its window is in front.
        if let Some(f) = self.by_client(e.event) {
            self.activate(f)?;
            self.conn.allow_events(Allow::REPLAY_POINTER, e.time)?;
            return Ok(());
        }
        let f = e.event;
        let (w, h, x, y, maximized, restore) = match self.windows.get(&f) {
            Some(m) => (m.w, m.h, m.x, m.y, m.maximized, m.restore),
            None => return Ok(()),
        };
        self.activate(f)?;
        if e.detail != 1 {
            return Ok(());
        }
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
        match frame::hit(w, h, px, py, !maximized) {
            Part::Close => return self.close(f),
            Part::Maximize => return self.toggle_maximize(f),
            Part::Minimize => return self.minimize(f),
            Part::Title => {
                let now = std::time::Instant::now();
                let is_double_click = match self.last_title_click {
                    Some((prev_time, prev_f)) => prev_f == f && now.duration_since(prev_time).as_millis() < 400,
                    None => false,
                };
                self.last_title_click = Some((now, f));
                if is_double_click {
                    self.toggle_maximize(f)?;
                    return Ok(());
                }

                let (mut cur_x, mut cur_y) = (x, y);
                if maximized {
                    if let Some((_, _, rw, rh)) = restore {
                        if let Some(m) = self.windows.get_mut(&f) {
                            m.maximized = false;
                            m.restore = None;
                        }
                        cur_x = (i32::from(e.root_x) - rw / 2).clamp(0, (self.screen_w - rw).max(0));
                        cur_y = 0;
                        self.place(f, cur_x, cur_y, rw, rh)?;
                        self.paint(f, None)?;
                    }
                }
                self.drag = Some(Drag::Move { frame: f, dx: i32::from(e.root_x) - cur_x, dy: i32::from(e.root_y) - cur_y });
                let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                let _ = self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
            }
            Part::Resize(l, t, r, b) => {
                self.drag =
                    Some(Drag::Resize { frame: f, sides: (l, t, r, b), start: (i32::from(e.root_x), i32::from(e.root_y)), origin: (x, y, w, h) });
                let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                let _ = self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
            }
            _ => {}
        }
        Ok(())
    }

    fn root_click(&mut self, e: ButtonPressEvent) -> Res<()> {
        let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
        self.desktop.refresh(self.screen_w, self.screen_h);
        let hit = self.desktop.hit(x, y);
        if let Some(prev) = self.active.take() {
            let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME);
            let _ = self.conn.delete_property(self.root, self.atoms.net_active_window);
            let _ = self.paint(prev, None);
            self.bar_stale = true;
        }
        if e.detail == 1 {
            let now = std::time::Instant::now();
            let is_double_click = match self.last_desktop_click {
                Some((prev_time, prev_hit)) => hit.is_some() && hit == Some(prev_hit) && now.duration_since(prev_time).as_millis() < 650,
                None => false,
            };
            self.last_desktop_click = hit.map(|idx| (now, idx));

            if is_double_click {
                if let Some(idx) = hit {
                    if let Some(item) = self.desktop.items.get(idx) {
                        launch(&item.exec_command());
                    }
                }
                self.desktop.selected = None;
                self.drag = None;
                self.paint_full_desktop()?;
            } else {
                let prev_sel = self.desktop.selected;
                self.desktop.selected = hit;
                if self.desktop.selected != prev_sel {
                    let mut dirty = Rect::default();
                    if let Some(idx) = prev_sel {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = it.rect();
                        }
                    }
                    if let Some(idx) = hit {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = if dirty.w > 0 { dirty.union(&it.rect()) } else { it.rect() };
                        }
                    }
                    if dirty.w > 0 && dirty.h > 0 {
                        self.paint_desktop_rect(dirty)?;
                    }
                }
                if let Some(idx) = hit {
                    let item = &self.desktop.items[idx];
                    self.drag = Some(Drag::Desktop { item: idx, dx: x - item.x, dy: y - item.y });
                    let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                    let _ = self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
                }
            }
        } else if e.detail == 3 {
            let prev_sel = self.desktop.selected;
            self.desktop.selected = hit;
            if self.desktop.selected != prev_sel {
                let mut dirty = Rect::default();
                if let Some(idx) = prev_sel {
                    if let Some(it) = self.desktop.items.get(idx) {
                        dirty = it.rect();
                    }
                }
                if let Some(idx) = hit {
                    if let Some(it) = self.desktop.items.get(idx) {
                        dirty = if dirty.w > 0 { dirty.union(&it.rect()) } else { it.rect() };
                    }
                }
                if dirty.w > 0 && dirty.h > 0 {
                    let _ = self.paint_desktop_rect(dirty);
                }
            }
            self.open_desktop_menu(x, y, hit)?;
        }
        Ok(())
    }

    fn motion(&mut self, e: MotionNotifyEvent) -> Res<()> {
        let (rx, ry) = (i32::from(e.root_x), i32::from(e.root_y));
        if self.menu.is_some() {
            // 1. Nested submenu hover (tertiary flyout)
            let (inside_nested, nested_changed) = if let Some(nested) = &mut self.nested_submenu {
                let inside = (nested.x..nested.x + nested.width).contains(&rx) && (nested.y..nested.y + nested.layout.height).contains(&ry);
                if inside {
                    let hover = bar::menu_hit_with_layout(&nested.items(), rx - nested.x, ry - nested.y, false, &nested.layout);
                    if hover != nested.hover {
                        nested.hover = hover;
                        (true, true)
                    } else {
                        (true, false)
                    }
                } else {
                    (false, false)
                }
            } else {
                (false, false)
            };
            if nested_changed {
                self.paint_nested_submenu()?;
                return Ok(());
            }
            if inside_nested {
                return Ok(());
            }

            // 2. Submenu hover (secondary flyout)
            let (inside_sub, sub_changed, open_nested) = if let Some(sub) = &mut self.submenu {
                let inside = (sub.x..sub.x + sub.width).contains(&rx) && (sub.y..sub.y + sub.layout.height).contains(&ry);
                if inside {
                    let hover = bar::menu_hit_with_layout(&sub.items(), rx - sub.x, ry - sub.y, false, &sub.layout);
                    if hover != sub.hover {
                        sub.hover = hover;
                        let open_nested = hover.and_then(|idx| match &sub.choices[idx] {
                            Choice::Submenu(kind) => Some((kind.clone(), idx)),
                            _ => None,
                        });
                        (true, true, Some(open_nested))
                    } else {
                        (true, false, None)
                    }
                } else {
                    (false, false, None)
                }
            } else {
                (false, false, None)
            };

            if inside_sub {
                if sub_changed {
                    self.paint_submenu()?;
                    if let Some(Some((k, idx))) = open_nested {
                        self.open_nested_submenu(k, idx)?;
                    } else if let Some(None) = open_nested {
                        self.close_nested_submenu()?;
                    }
                }
                return Ok(());
            }

            // 3. Main menu hover (primary menu)
            let (inside_menu, hover_changed, open_sub) = {
                let menu = self.menu.as_mut().unwrap();
                let menu_h = menu.layout.height;
                let inside_menu = (menu.x..menu.x + menu.width).contains(&rx) && (menu.y..menu.y + menu_h).contains(&ry);
                if inside_menu {
                    let hover = bar::menu_hit_with_layout(&menu.items(), rx - menu.x, ry - menu.y, menu.with_band, &menu.layout);
                    if hover != menu.hover {
                        menu.hover = hover;
                        let open_sub = hover.and_then(|idx| match &menu.choices[idx] {
                            Choice::Submenu(kind) => Some((kind.clone(), idx)),
                            _ => None,
                        });
                        (true, true, Some(open_sub))
                    } else {
                        (true, false, None)
                    }
                } else {
                    (false, false, None)
                }
            };

            if inside_menu {
                if hover_changed {
                    self.paint_menu()?;
                    if let Some(Some((k, idx))) = open_sub {
                        self.open_submenu(k, idx)?;
                    } else if let Some(None) = open_sub {
                        self.close_submenu()?;
                    }
                }
                return Ok(());
            }
            return Ok(());
        }
        match self.drag {
            Some(Drag::Desktop { item, dx, dy }) => {
                let new_x = rx - dx;
                let new_y = ry - dy;
                let old_rect = self.desktop.items.get(item).map(|it| it.rect()).unwrap_or_default();
                self.desktop.move_item(item, new_x, new_y, self.screen_w, self.screen_h);
                let new_rect = self.desktop.items.get(item).map(|it| it.rect()).unwrap_or_default();
                let dirty = old_rect.union(&new_rect);
                self.paint_desktop_rect(dirty)?;
            }
            Some(Drag::Move { frame: f, dx, dy }) => {
                let Some(m) = self.windows.get(&f) else { return Ok(()) };
                let w = m.w;
                // Never so far off that the title bar can't be reached.
                let x = (rx - dx).clamp(48 - w, self.screen_w - 48);
                let y = (ry - dy).clamp(0, self.work_h() - frame::TOP);
                self.place_move(f, x, y)?;
            }
            Some(Drag::Resize { frame: f, sides: (l, t, r, b), start, origin }) => {
                let (dx, dy) = (rx - start.0, ry - start.1);
                let (mut x, mut y, mut w, mut h) = origin;
                if r {
                    w = (origin.2 + dx).max(MIN_W);
                }
                if b {
                    h = (origin.3 + dy).max(MIN_H);
                }
                if l {
                    w = (origin.2 - dx).max(MIN_W);
                    x = origin.0 + origin.2 - w;
                }
                if t {
                    h = (origin.3 - dy).max(MIN_H);
                    y = origin.1 + origin.3 - h;
                }
                self.place(f, x, y, w, h)?;
            }
            None => {
                let new_hover = self.desktop.hit(rx, ry);
                if new_hover != self.desktop.hovered {
                    let old_hover = self.desktop.hovered;
                    self.desktop.hovered = new_hover;
                    let mut dirty = Rect::default();
                    if let Some(idx) = old_hover {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = it.rect();
                        }
                    }
                    if let Some(idx) = new_hover {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = if dirty.w > 0 { dirty.union(&it.rect()) } else { it.rect() };
                        }
                    }
                    if dirty.w > 0 && dirty.h > 0 {
                        self.paint_desktop_rect(dirty)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn toggle_show_desktop(&mut self) -> Res<()> {
        if let Some(minimized) = self.show_desktop_minimized.take() {
            for f in minimized {
                if let Some(m) = self.windows.get(&f) {
                    if m.minimized {
                        self.restore(f)?;
                    }
                }
            }
        } else {
            let mut to_minimize = Vec::new();
            for (&f, m) in &self.windows {
                if !m.minimized {
                    to_minimize.push(f);
                }
            }
            if !to_minimize.is_empty() {
                for &f in &to_minimize {
                    self.minimize(f)?;
                }
                self.show_desktop_minimized = Some(to_minimize);
            }
        }
        Ok(())
    }

    fn button_release(&mut self, e: ButtonReleaseEvent) -> Res<()> {
        let (rx, ry) = (i32::from(e.root_x), i32::from(e.root_y));
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);

        if let Some(Drag::Desktop { item, .. }) = self.drag.take() {
            let old_rect = self.desktop.items.get(item).map(|it| it.rect()).unwrap_or_default();
            self.desktop.finish_move_item(item, self.screen_h);
            let new_rect = self.desktop.items.get(item).map(|it| it.rect()).unwrap_or_default();
            let dirty = old_rect.union(&new_rect);
            self.paint_desktop_rect(dirty)?;
            return Ok(());
        }

        if let Some(Drag::Move { frame: f, .. }) = self.drag.take() {
            if ry <= 2 {
                // Dragged to top edge -> Snap to Maximize
                if let Some(m) = self.windows.get_mut(&f) {
                    if !m.maximized && !m.fullscreen {
                        m.maximized = true;
                        m.restore = Some((m.x, m.y, m.w, m.h));
                        let (w, h) = (self.screen_w, self.work_h());
                        self.place(f, 0, 0, w, h)?;
                        self.paint(f, None)?;
                    }
                }
            } else if rx <= 2 {
                // Dragged to left edge -> Snap to Left Half (50%)
                if let Some(m) = self.windows.get_mut(&f) {
                    if !m.fullscreen {
                        if m.restore.is_none() {
                            m.restore = Some((m.x, m.y, m.w, m.h));
                        }
                        m.maximized = false;
                        let half_w = self.screen_w / 2;
                        let h = self.work_h();
                        self.place(f, 0, 0, half_w, h)?;
                        self.paint(f, None)?;
                    }
                }
            } else if rx >= self.screen_w - 3 {
                // Dragged to right edge -> Snap to Right Half (50%)
                if let Some(m) = self.windows.get_mut(&f) {
                    if !m.fullscreen {
                        if m.restore.is_none() {
                            m.restore = Some((m.x, m.y, m.w, m.h));
                        }
                        m.maximized = false;
                        let half_w = self.screen_w / 2;
                        let rem_w = self.screen_w - half_w;
                        let h = self.work_h();
                        self.place(f, half_w, 0, rem_w, h)?;
                        self.paint(f, None)?;
                    }
                }
            } else if let Some(m) = self.windows.get(&f) {
                if !m.maximized && !m.fullscreen {
                    self.place(f, m.x, m.y, m.w, m.h)?;
                }
            }
            if let Some(m) = self.windows.get(&f) {
                if !m.is_dialog && !m.maximized && !m.fullscreen {
                    self.window_store.set(&m.class, m.x, m.y, m.w, m.h);
                }
            }
            return Ok(());
        }

        if let Some(Drag::Resize { frame: f, .. }) = self.drag.take() {
            if let Some(m) = self.windows.get(&f) {
                if !m.is_dialog && !m.maximized && !m.fullscreen {
                    self.window_store.set(&m.class, m.x, m.y, m.w, m.h);
                }
            }
            return Ok(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_ignore_the_lock_keys() {
        assert_eq!(shortcut(CONTROL | SHIFT, KEY_ESCAPE), Some(Shortcut::TaskManager));
        assert_eq!(shortcut(CONTROL | SHIFT | NUM_LOCK | CAPS_LOCK, KEY_ESCAPE), Some(Shortcut::TaskManager));
        assert_eq!(shortcut(CONTROL, KEY_ESCAPE), Some(Shortcut::StartMenu));
        assert_eq!(shortcut(ALT | NUM_LOCK, KEY_F4), Some(Shortcut::CloseWindow));
        assert_eq!(shortcut(0, KEY_SUPER_L), Some(Shortcut::StartMenu));
        assert_eq!(shortcut(NUM_LOCK, KEY_SUPER_R), Some(Shortcut::StartMenu));
        assert_eq!(shortcut(SUPER, KEY_E), Some(Shortcut::Files));
        assert_eq!(shortcut(SUPER | CAPS_LOCK, KEY_D), Some(Shortcut::ShowDesktop));
        // Escape or F4 by themselves belong to the program.
        assert_eq!(shortcut(0, KEY_ESCAPE), None);
        assert_eq!(shortcut(0, KEY_F4), None);
        assert_eq!(shortcut(ALT | CONTROL, KEY_F4), None);
    }

    #[test]
    fn clock_reads_like_windows_95() {
        assert_eq!(clock_format(0, 5), "12:05 AM");
        assert_eq!(clock_format(9, 30), "9:30 AM");
        assert_eq!(clock_format(12, 0), "12:00 PM");
        assert_eq!(clock_format(22, 14), "10:14 PM");
    }

    #[test]
    fn context_menu_items_display_toggles_and_submenus() {
        let menu = Menu {
            window: 0,
            x: 0,
            y: 0,
            choices: vec![
                Choice::Submenu(SubmenuKind::View),
                Choice::Submenu(SubmenuKind::SortBy),
                Choice::RefreshDesktop,
                Choice::ToggleAutoArrange(true),
                Choice::ToggleAlignToGrid(false),
            ],
            hover: None,
            with_band: false,
            width: 180,
            layout: bar::MenuLayout::default(),
            kind: None,
        };
        let items = menu.items();
        assert_eq!(items.len(), 5);
        match &items[0] {
            Item::Entry(e) => {
                assert_eq!(e.label, "View");
                assert!(e.submenu);
            }
            _ => panic!("Expected Entry for View"),
        }
        match &items[3] {
            Item::Entry(e) => assert_eq!(e.label, "[v] Auto arrange icons"),
            _ => panic!("Expected Entry for ToggleAutoArrange"),
        }
        match &items[4] {
            Item::Entry(e) => assert_eq!(e.label, "[  ] Align icons to grid"),
            _ => panic!("Expected Entry for ToggleAlignToGrid"),
        }
    }
}
