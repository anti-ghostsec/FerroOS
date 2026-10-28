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
    net_wm_state_demands_attention: Atom,
    wm_hints: Atom,
    ferro_appearance: Atom,
    ferro_switches: Atom,
    xrootpmap_id: Atom,
    esetroot_pmap_id: Atom,
    ferro_tick: Atom,
    /// Where the Recycle Bin's icon is, for the file manager's drops.
    ferro_recycle_bin: Atom,
    ferro_refresh: Atom,
    /// Files dropped on the desktop from the file manager: where they go.
    ferro_place: Atom,
    net_wm_pid: Atom,
    /// The folder a file manager window shows, for pinning that folder.
    ferro_current_dir: Atom,
    /// A sandboxed program asking for the Info window (ferro-info).
    ferro_info: Atom,
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
            net_wm_state_demands_attention: get("_NET_WM_STATE_DEMANDS_ATTENTION")?,
            wm_hints: get("WM_HINTS")?,
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
            xrootpmap_id: get("_XROOTPMAP_ID")?,
            esetroot_pmap_id: get("ESETROOT_PMAP_ID")?,
            ferro_tick: get("_FERRO_TICK")?,
            ferro_recycle_bin: get("_FERRO_RECYCLE_BIN")?,
            ferro_refresh: get("_FERRO_REFRESH")?,
            ferro_place: get("_FERRO_PLACE")?,
            net_wm_pid: get("_NET_WM_PID")?,
            ferro_current_dir: get("_FERRO_CURRENT_DIR")?,
            ferro_info: get("_FERRO_INFO")?,
        })
    }
}

/// One program window and the frame around it.
struct Managed {
    client: Window,
    frame: Window,
    pid: Option<u32>,
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
    urgent: bool,
}

/// What the mouse is in the middle of doing.
enum Drag {
    Move { frame: Window, dx: i32, dy: i32 },
    Resize { frame: Window, sides: (bool, bool, bool, bool), start: (i32, i32), origin: (i32, i32, i32, i32) },
    Desktop { item: usize, start: (i32, i32), origin: (i32, i32), moved: bool, others: Vec<(usize, i32, i32)> },
    DesktopSelect { start_x: i32, start_y: i32, cur_x: i32, cur_y: i32 },
    TaskBar { task: usize, start_x: i32, moved: bool, was_active: bool },
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
    /// Desktop icons dragged over a window: drawn beside the pointer there
    /// (the desktop itself is under the window).
    drag_icon: Option<ferro_ui::DragIcon>,
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
    /// Where the pointer last was while a menu was open: which way it's
    /// heading decides whether passing over items may close a flyout.
    menu_pointer: (i32, i32),
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
    /// Desktop appearance and theme settings.
    appearance: Appearance,
    /// Cached decoded wallpaper image.
    cached_wallpaper: Option<ferro_gfx::DecodedImage>,
    /// Pre-rendered desktop background (wallpaper or solid color).
    desktop_bg: Surface<'static>,
    /// Full desktop buffer with background and items drawn.
    desktop_surface: Surface<'static>,
    /// Dedicated desktop window beneath all client windows for fast hardware-composited desktop.
    desktop_win: Window,
    /// Root window background pixmap for flicker-free desktop and hardware caching.
    root_pixmap: Pixmap,
    /// Windows minimized by Show Desktop shortcut (Win+D).
    show_desktop_minimized: Option<Vec<Window>>,
    /// Privacy and kill switch choices.
    store: ferro_sandbox::store::Store,
    keymap: ferro_ui::Keymap,
    /// The keys that start the Snipping Tool: Print Screen's and Insert's.
    print_keys: Vec<u8>,
    flash_tick: u32,
    tray_tooltip: Option<(String, Rect)>,
    /// The Recycle Bin's place as last published, and how many it holds.
    bin_published: Option<Rect>,
    bin_count: usize,
    last_tray_text: String,
    last_clock_text: String,
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
    UnpinTaskbar(usize),
    CreateDesktopShortcut {
        name: String,
        exec: String,
    },
    DeleteDesktopItem(usize),
    RenameDesktopItem(usize),
    DesktopProperties(usize),
    CutDesktopSelection,
    CopyDesktopSelection,
    PasteToDesktop,
    ToggleAutoArrange(bool),
    ToggleAlignToGrid(bool),
    ArrangeIconsByName,
    ArrangeIconsByType,
    #[allow(dead_code)]
    AlignToGrid,
    RefreshDesktop,
    WindowRestore(Window),
    WindowMinimize(Window),
    WindowMaximize(Window),
    WindowClose(Window),
    ToggleShowDesktop,
    LogOff,
    ShutDown,
    Restart,
    RestartDesktop,
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
                Choice::PinTaskbar { .. } => Item::entry("Pin to taskbar", Some(icons::FERRO), false),
                Choice::UnpinTaskbar(_) => Item::entry("Unpin from taskbar", Some(icons::RECYCLE), false),
                Choice::CreateDesktopShortcut { .. } => Item::entry("Create Desktop Shortcut", Some(icons::COMPUTER), false),
                Choice::DeleteDesktopItem(_) => Item::entry("Delete", Some(icons::RECYCLE), false),
                Choice::RenameDesktopItem(_) => Item::entry("Rename", None, false),
                Choice::DesktopProperties(_) => Item::entry("Properties", None, false),
                Choice::CutDesktopSelection => Item::entry("Cut", None, false),
                Choice::CopyDesktopSelection => Item::entry("Copy", None, false),
                Choice::PasteToDesktop => Item::entry("Paste", None, false),
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
                Choice::WindowRestore(_) => Item::entry("Restore", None, false),
                Choice::WindowMinimize(_) => Item::entry("Minimize", None, false),
                Choice::WindowMaximize(_) => Item::entry("Maximize", None, false),
                Choice::WindowClose(_) => Item::entry("Close", None, false),
                Choice::ToggleShowDesktop => Item::entry("Show the desktop", Some(icons::COMPUTER), false),
                Choice::LogOff => Item::entry("Log Off...", Some(icons::FERRO), false),
                Choice::ShutDown => Item::entry("Shut Down...", Some(icons::COMPUTER), false),
                Choice::Restart => Item::entry("Restart...", Some(icons::COMPUTER), false),
                Choice::RestartDesktop => Item::entry("Restart Desktop", Some(icons::FERRO), false),
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

fn full_date_text() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    #[cfg(unix)]
    {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = now.try_into().unwrap_or(0);
        unsafe { libc::localtime_r(&t, &mut tm) };
        let weekdays = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let wday = weekdays.get(tm.tm_wday as usize).copied().unwrap_or("");
        let mon = months.get(tm.tm_mon as usize).copied().unwrap_or("");
        let day = tm.tm_mday;
        let year = 1900 + tm.tm_year;
        format!("{wday}, {mon} {day}, {year}")
    }
    #[cfg(not(unix))]
    {
        "Today".into()
    }
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

fn restart_wm() -> ! {
    let bin = std::ffi::CString::new("/usr/local/bin/ferro-wm").unwrap();
    let args = [bin.as_ptr(), std::ptr::null()];
    unsafe {
        libc::execv(bin.as_ptr(), args.as_ptr());
    }
    std::process::exit(1);
}

extern "C" fn handle_reload(_: libc::c_int) {
    let bin = std::ffi::CString::new("/usr/local/bin/ferro-wm").unwrap();
    let args = [bin.as_ptr(), std::ptr::null()];
    unsafe {
        libc::execv(bin.as_ptr(), args.as_ptr());
    }
    std::process::exit(1);
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
const KEY_F5: u8 = 71;
const KEY_DELETE: u8 = 119;
const KEY_END: u8 = 115;
const KEY_C: u8 = 54;
const KEY_X: u8 = 53;
const KEY_V: u8 = 55;
const KEY_A: u8 = 38;
const KEY_S: u8 = 39;
const KEY_PRINT: u8 = 107;

/// The held-down keys as X11 numbers them.
const SHIFT: u16 = 1;
const CAPS_LOCK: u16 = 2;
const CONTROL: u16 = 4;
const ALT: u16 = 8;
const NUM_LOCK: u16 = 16;
const SUPER: u16 = 64; // Mod4

/// The keys that work whatever window has the typing: what they need held
/// down, the key, and what they do.
const SHORTCUTS: [(u16, u8, Shortcut); 11] = [
    (CONTROL | SHIFT, KEY_ESCAPE, Shortcut::TaskManager),
    (CONTROL, KEY_ESCAPE, Shortcut::StartMenu),
    (ALT, KEY_F4, Shortcut::CloseWindow),
    (0, KEY_SUPER_L, Shortcut::StartMenu),
    (0, KEY_SUPER_R, Shortcut::StartMenu),
    (SUPER, KEY_E, Shortcut::Files),
    (SUPER, KEY_D, Shortcut::ShowDesktop),
    (SUPER, KEY_L, Shortcut::LockScreen),
    (CONTROL | ALT, KEY_END, Shortcut::Panic),
    (0, KEY_PRINT, Shortcut::Snip),
    (SUPER | SHIFT, KEY_S, Shortcut::Snip),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    TaskManager,
    StartMenu,
    CloseWindow,
    Files,
    ShowDesktop,
    LockScreen,
    /// The Snipping Tool: a box dragged over the screen, saved.
    Snip,
    /// Power off this instant: no saving, no clean shutdown. With the power
    /// gone the disk's keys go from memory and only the passphrase opens it.
    Panic,
}

/// Which shortcut a key press is, if any. Caps Lock and Num Lock being on
/// (they count as held keys) changes nothing.
/// The Print Screen and Insert keys' symbols.
const XK_PRINT: u32 = 0xff61;
const XK_INSERT: u32 = 0xff63;

/// Every key this keyboard has `keysym` on (keyboards differ in which).
fn keycodes_for(conn: &RustConnection, keysym: u32) -> Vec<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let Ok(Ok(map)) = conn.get_keyboard_mapping(min, max - min + 1).map(|c| c.reply()) else { return Vec::new() };
    let per = usize::from(map.keysyms_per_keycode).max(1);
    map.keysyms.chunks(per).enumerate().filter(|(_, syms)| syms.contains(&keysym)).map(|(i, _)| min + i as u8).collect()
}

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
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
        | EventMask::KEY_PRESS;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(mask).background_pixel(appearance.desktop_color))?
        .check()
        .map_err(|_| "another window manager is running")?;
    conn.clear_area(false, root, 0, 0, 0, 0)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, root, &CreateGCAux::new().graphics_exposures(0))?;
    let _ = conn.change_pointer_control(1, 1, 0, true, true);
    let atoms = Atoms::new(&conn)?;
    // Finished programs we started are forgotten, not kept as zombies.
    #[cfg(unix)]
    // SAFETY: installing signal handlers.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, handle_reload as *const () as libc::sighandler_t);
        libc::signal(libc::SIGUSR1, handle_reload as *const () as libc::sighandler_t);
        // Reloaded from inside that handler, this copy starts with the
        // signal still blocked: unblocked, or a second reload never comes.
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::sigaddset(&mut set, libc::SIGUSR1);
        libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
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
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(color::FACE)
            .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION | EventMask::LEAVE_WINDOW),
    )?;
    conn.map_window(bar_win)?;
    let tick_atom = atoms.ferro_tick;
    std::thread::spawn(move || {
        let Ok((tick, _)) = x11rb::connect(None) else { return };
        let event = ClientMessageEvent::new(32, bar_win, tick_atom, [0; 5]);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if tick.send_event(false, bar_win, EventMask::NO_EVENT, event).is_err() || tick.flush().is_err() {
                return;
            }
        }
    });

    let keymap = ferro_ui::Keymap::load(&conn)?;
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
    let root_pixmap = conn.generate_id()?;
    conn.create_pixmap(screen.root_depth, root_pixmap, root, screen_w as u16, screen_h as u16)?;

    let desktop_win = conn.generate_id()?;
    conn.create_window(
        screen.root_depth,
        desktop_win,
        root,
        0,
        0,
        screen_w as u16,
        (screen_h - bar::HEIGHT) as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(appearance.desktop_color)
            .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION),
    )?;
    conn.change_property32(PropMode::REPLACE, desktop_win, atoms.net_wm_window_type, AtomEnum::ATOM, &[atoms.net_wm_window_type_desktop])?;
    conn.map_window(desktop_win)?;
    conn.configure_window(desktop_win, &ConfigureWindowAux::new().stack_mode(StackMode::BELOW))?;

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
        menu_pointer: (0, 0),
        app_cache: None,
        app_cache_time: None,
        desktop: desktop_mgr,
        window_store,
        pinned_apps,
        last_title_click: None,
        last_desktop_click: None,
        depth: screen.root_depth,
        visual: screen.root_visual,
        gc,
        atoms,
        windows: HashMap::new(),
        order: Vec::new(),
        active: None,
        drag: None,
        drag_icon: None,
        appearance,
        cached_wallpaper,
        desktop_bg,
        desktop_surface,
        desktop_win,
        root_pixmap,
        show_desktop_minimized: None,
        store: ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH),
        keymap,
        print_keys: Vec::new(),
        flash_tick: 0,
        tray_tooltip: None,
        bin_published: None,
        bin_count: desktop::count_trash_items(),
        last_tray_text: tray_text(),
        last_clock_text: clock_text(),
        conn,
    };
    wm.announce()?;
    // The shortcuts come to us first, with or without the lock keys on.
    // Print Screen is wherever this keyboard has it (laptops differ).
    // And on Insert alone: many keyboards share one key for Insert and Print
    // Screen, sending Insert unless Fn is held. Shift+Insert and Ctrl+Insert
    // (paste and copy) still reach programs; only plain Insert snips.
    wm.print_keys = keycodes_for(&wm.conn, XK_PRINT);
    wm.print_keys.extend(keycodes_for(&wm.conn, XK_INSERT));
    let print = wm.print_keys.iter().map(|&k| (0, k));
    for (mods, key) in SHORTCUTS.iter().map(|(m, k, _)| (*m, *k)).chain(print) {
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
        let event = wm.conn.wait_for_event()?;
        let mut events = Vec::with_capacity(16);
        events.push(event);
        while let Ok(Some(next)) = wm.conn.poll_for_event() {
            events.push(next);
        }

        let mut pending_motion: Option<MotionNotifyEvent> = None;
        for ev in events {
            if let Event::MotionNotify(m) = ev {
                pending_motion = Some(m);
            } else {
                if let Some(m) = pending_motion.take() {
                    if let Err(e) = wm.handle(Event::MotionNotify(m)) {
                        eprintln!("ferro-wm: {e}");
                    }
                }
                if let Err(e) = wm.handle(ev) {
                    eprintln!("ferro-wm: {e}");
                }
            }
        }
        if let Some(m) = pending_motion.take() {
            if let Err(e) = wm.handle(Event::MotionNotify(m)) {
                eprintln!("ferro-wm: {e}");
            }
        }

        if std::mem::take(&mut wm.bar_stale) {
            wm.paint_bar()?;
        }
        wm.conn.flush()?;
    }
}

/// Fits a window (its frame) inside the screen above the taskbar: moved in
/// first, then made smaller if it still doesn't fit, so no part of it (an
/// app's sign-in button, say) ends up under the taskbar or off the screen.
fn fit_rect(x: i32, y: i32, w: i32, h: i32, screen_w: i32, work_h: i32) -> (i32, i32, i32, i32) {
    let w = w.min(screen_w).max(1);
    let h = h.min(work_h).max(1);
    let x = x.min(screen_w - w).max(0);
    let y = y.min(work_h - h).max(0);
    (x, y, w, h)
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
        let is_flash_phase = (self.flash_tick % 2) == 1;
        let tasks: Vec<Task> = self
            .tasks
            .iter()
            .filter_map(|f| self.windows.get(f))
            .map(|m| Task { title: &m.title, active: self.active == Some(m.frame), flashing: m.urgent && is_flash_phase })
            .collect();
        let tooltip = self.tray_tooltip.as_ref().map(|(tip, r)| (tip.as_str(), r.x + r.w / 2));
        let mut s = Surface::new(self.screen_w as usize, bar::HEIGHT as usize);
        bar::draw(&mut s, self.screen_w, &tasks, &self.pinned_apps, self.menu.is_some(), &self.last_tray_text, Some(&self.store.switches), tooltip);
        self.blit(self.bar, &s, self.screen_w, (0, 0, self.screen_w, bar::HEIGHT))
    }

    /// Every half second (and straight after a delete): the Recycle Bin's count, whoever put
    /// things in it (the file manager, the image viewer, a program), and
    /// where its icon is, for the file manager to drop things on.
    fn keep_bin_current(&mut self) -> Res<()> {
        let Some((idx, rect)) = self.desktop.bin() else { return Ok(()) };
        if self.bin_published != Some(rect) {
            self.bin_published = Some(rect);
            let v = [rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32];
            self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms.ferro_recycle_bin, AtomEnum::CARDINAL, &v)?;
        }
        // Every tick (half a second): a folder listing, cheap enough that
        // the count follows a delete or Empty without a visible wait.
        let count = desktop::count_trash_items();
        if count != self.bin_count {
            self.bin_count = count;
            let before = self.desktop.items[idx].visual_rect();
            self.desktop.items[idx].label = desktop::bin_label(count);
            let after = self.desktop.items[idx].visual_rect();
            // Only the icon and its label are drawn again.
            let (x, y) = (before.x.min(after.x), before.y.min(after.y));
            let r = Rect::new(x, y, before.right().max(after.right()) - x, before.bottom().max(after.bottom()) - y);
            self.paint_desktop_rect(r)?;
        }
        Ok(())
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
            let selected = self.desktop.is_selected(i);
            let ren = self.desktop.renaming.as_ref().filter(|r| r.item_idx == i);
            let thumb = self.desktop.thumbnails.get(&item.id);
            desktop::draw_item(&mut self.desktop_surface, item, selected, false, ren, thumb);
        }
        self.blit(self.root_pixmap, &self.desktop_surface, w, (0, 0, w, h))?;
        let _ = self.conn.change_window_attributes(self.root, &ChangeWindowAttributesAux::new().background_pixmap(self.root_pixmap));
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms.xrootpmap_id, AtomEnum::PIXMAP, &[self.root_pixmap]);
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms.esetroot_pmap_id, AtomEnum::PIXMAP, &[self.root_pixmap]);

        if let Some(h_idx) = self.desktop.hovered {
            if self.drag.is_none() {
                if let Some(item) = self.desktop.items.get(h_idx) {
                    let selected = self.desktop.is_selected(h_idx);
                    let ren = self.desktop.renaming.as_ref().filter(|r| r.item_idx == h_idx);
                    let thumb = self.desktop.thumbnails.get(&item.id);
                    desktop::draw_item(&mut self.desktop_surface, item, selected, true, ren, thumb);
                }
            }
        }
        if let Some(rb) = self.desktop.rubberband {
            desktop::focus_rect(&mut self.desktop_surface, rb);
        }
        self.blit(self.desktop_win, &self.desktop_surface, w, (0, 0, w, self.work_h()))?;
        Ok(())
    }

    fn paint_desktop_rect(&mut self, rect: Rect) -> Res<()> {
        let w = self.screen_w;
        let r = rect.intersect(&Rect::new(0, 0, w, self.work_h()));
        if r.w <= 0 || r.h <= 0 {
            return Ok(());
        }
        self.desktop_surface.copy_from(&self.desktop_bg, r);
        for (i, item) in self.desktop.items.iter().enumerate() {
            if item.visual_rect().intersect(&r).w > 0 && item.visual_rect().intersect(&r).h > 0 {
                let selected = self.desktop.is_selected(i);
                let hovered = self.drag.is_none() && self.desktop.hovered == Some(i);
                let ren = self.desktop.renaming.as_ref().filter(|r| r.item_idx == i);
                let thumb = self.desktop.thumbnails.get(&item.id);
                desktop::draw_item(&mut self.desktop_surface, item, selected, hovered, ren, thumb);
            }
        }
        if let Some(rb) = self.desktop.rubberband {
            if rb.intersect(&r).w > 0 && rb.intersect(&r).h > 0 {
                desktop::focus_rect(&mut self.desktop_surface, rb);
            }
        }
        self.blit_at(self.desktop_win, &self.desktop_surface, w, (r.x, r.y), (r.x, r.y), (r.w, r.h))?;
        Ok(())
    }

    fn border_strips(&self, r: Rect) -> [Rect; 4] {
        [
            Rect::new(r.x - 2, r.y - 2, r.w + 4, 4),
            Rect::new(r.x - 2, r.bottom() - 2, r.w + 4, 4),
            Rect::new(r.x - 2, r.y - 2, 4, r.h + 4),
            Rect::new(r.right() - 2, r.y - 2, 4, r.h + 4),
        ]
    }

    fn paint_desktop(&mut self) -> Res<()> {
        let (w, h) = (self.screen_w, self.screen_h);
        self.desktop.refresh(w, h);
        self.paint_full_desktop()
    }

    fn paint_desktop_surface(&mut self) -> Res<()> {
        self.paint_full_desktop()
    }

    fn send_configure_notify(&self, client: Window, x: i32, y: i32, w: i32, h: i32) -> Res<()> {
        let note = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: client,
            window: client,
            above_sibling: NONE,
            x: x as i16,
            y: y as i16,
            width: w as u16,
            height: h as u16,
            border_width: 0,
            override_redirect: false,
        };
        let _ = self.conn.send_event(false, client, EventMask::STRUCTURE_NOTIFY, note);
        let mut note_root = note;
        note_root.event = self.root;
        let _ = self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, note_root);
        Ok(())
    }

    fn place_move(&mut self, frame_win: Window, x: i32, y: i32) -> Res<()> {
        let (client, fullscreen, w, h) = {
            let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
            (m.x, m.y) = (x, y);
            (m.client, m.fullscreen, m.w, m.h)
        };
        self.conn.configure_window(frame_win, &ConfigureWindowAux::new().x(x).y(y))?;
        let (cx, cy, cw, ch) = if fullscreen { (0, 0, w, h) } else { (frame::LEFT, frame::TOP, frame::inner(w, h).0, frame::inner(w, h).1) };
        self.send_configure_notify(client, x + cx, y + cy, cw, ch)?;
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
            Choice::Action {
                label: "My Computer",
                command: "ferro-files \"My Computer\"".into(),
                icon: Some(icons::COMPUTER),
            },
            Choice::Submenu(SubmenuKind::Settings),
            Choice::Action {
                label: "Help",
                // FerroOS's help text, in a window it can be scrolled in.
                command: "xterm -T Help -geometry 80x40 -bg \"#FFFFFF\" -fg \"#000000\" -fa \"DejaVu Sans Mono\" -fs 10 -e less -P \"Help  -  arrows, Page Up/Down to scroll, Q to close\" /usr/local/share/ferro/help.txt".into(),
                icon: Some(icons::HELP),
            },
            Choice::Action {
                label: "Run...",
                command: "sh -c 'ferro-terminal || lxterminal || xterm'".into(),
                icon: Some(icons::TERMINAL),
            },
            Choice::Separator,
            Choice::LogOff,
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
                    command: "sh -c 's=\"$HOME/Desktop/Terminal.desktop\"; printf \"[Desktop Entry]\\nName=Terminal\\nExec=ferro-terminal\\nType=Application\\n\" > \"$s\" && chmod 700 \"$s\"'".into(),
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
                    command: "sh -c 'ferro-terminal || lxterminal || xterm'".into(),
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
                    command: "sh -c 'xterm -T \"Add Programs\" -bg \"#000000\" -fg \"#33FF33\" -cr \"#33FF33\" -fa \"DejaVu Sans Mono\" -fs 10 -e ferro-heavy menu || x-terminal-emulator -e ferro-heavy menu || ferro-cmd'".into(),
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
                        Choice::Separator,
                        Choice::DesktopProperties(idx),
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
                        Choice::Separator,
                        Choice::DesktopProperties(idx),
                    ],
                    desktop::ItemKind::Shortcut { exec, .. } => vec![
                        Choice::Action { label: "Open", command: exec.clone(), icon: Some(item.icon) },
                        Choice::PinTaskbar { name: item.label.clone(), exec: exec.clone() },
                        Choice::CutDesktopSelection,
                        Choice::CopyDesktopSelection,
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                        Choice::Separator,
                        Choice::DesktopProperties(idx),
                    ],
                    desktop::ItemKind::Folder(path) => vec![
                        Choice::Action {
                            label: "Open",
                            command: format!("ferro-files {}", ferro_associations::shell_quote(&path.to_string_lossy())),
                            icon: Some(icons::FOLDER),
                        },
                        Choice::PinTaskbar {
                            name: item.label.clone(),
                            exec: format!("ferro-files {}", ferro_associations::shell_quote(&path.to_string_lossy())),
                        },
                        Choice::CutDesktopSelection,
                        Choice::CopyDesktopSelection,
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                        Choice::Separator,
                        Choice::DesktopProperties(idx),
                    ],
                    desktop::ItemKind::File(path) => vec![
                        Choice::Action { label: "Open", command: ferro_associations::resolve_command_for_path(path), icon: Some(icons::FILE) },
                        Choice::PinTaskbar {
                            name: item.label.clone(),
                            exec: format!("xdg-open {}", ferro_associations::shell_quote(&path.to_string_lossy())),
                        },
                        Choice::CutDesktopSelection,
                        Choice::CopyDesktopSelection,
                        Choice::RenameDesktopItem(idx),
                        Choice::Separator,
                        Choice::DeleteDesktopItem(idx),
                        Choice::Separator,
                        Choice::DesktopProperties(idx),
                    ],
                }
            }
            None => {
                let mut items = vec![Choice::Submenu(SubmenuKind::View), Choice::Submenu(SubmenuKind::SortBy), Choice::RefreshDesktop];
                if desktop::has_clipboard() {
                    items.push(Choice::PasteToDesktop);
                }
                items.extend([
                    Choice::Separator,
                    Choice::Submenu(SubmenuKind::New),
                    Choice::Separator,
                    Choice::Action {
                        label: "Open in Terminal",
                        command: "sh -c 'mkdir -p \"$HOME/Desktop\" && cd \"$HOME/Desktop\" && (ferro-terminal || lxterminal || xterm)'".into(),
                        icon: Some(icons::TERMINAL),
                    },
                    Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) },
                    Choice::RestartDesktop,
                    Choice::Separator,
                    Choice::Action { label: "Properties", command: "ferro-appearance".into(), icon: Some(icons::COMPUTER) },
                ]);
                items
            }
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
        // The keyboard too, so the arrow keys, Enter and Esc work the menu.
        let _ = self.conn.grab_keyboard(false, self.root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
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

    /// Whether the pointer, moving from where it last was to (rx, ry), is
    /// heading for the open `flyout` of `parent`: inside the triangle from
    /// its last position to the flyout's near edge. Menus everywhere do this
    /// so a diagonal move across other items doesn't close the flyout.
    fn heading_into(&self, flyout: Option<&Menu>, parent: Option<&Menu>, rx: i32, ry: i32) -> bool {
        let (Some(f), Some(p)) = (flyout, parent) else { return false };
        let (px, py) = self.menu_pointer;
        // The flyout's edge nearest its parent.
        let edge = if f.x >= p.x + p.width / 2 { f.x } else { f.x + f.width };
        if (rx - px).abs() + (ry - py).abs() == 0 || (edge - px).signum() != (rx - px).signum() {
            return false;
        }
        let (ax, ay, bx, by) = (edge, f.y - 4, edge, f.y + f.layout.height + 4);
        let side = |x1: i32, y1: i32, x2: i32, y2: i32| i64::from(x2 - x1) * i64::from(ry - y1) - i64::from(y2 - y1) * i64::from(rx - x1);
        let (d1, d2, d3) = (side(px, py, ax, ay), side(ax, ay, bx, by), side(bx, by, px, py));
        let neg = d1 < 0 || d2 < 0 || d3 < 0;
        let pos = d1 > 0 || d2 > 0 || d3 > 0;
        !(neg && pos)
    }

    /// A key while a menu is open: Up and Down move, Right or Enter opens
    /// a flyout or starts what's chosen, Left goes back out of a flyout,
    /// Escape closes. `false` for keys it leaves to the shortcuts.
    fn menu_key(&mut self, keycode: u8) -> Res<bool> {
        const UP: u8 = 111;
        const DOWN: u8 = 116;
        const LEFT: u8 = 113;
        const RIGHT: u8 = 114;
        const RETURN: u8 = 36;
        const KP_ENTER: u8 = 104;
        const ESCAPE: u8 = 9;
        const SPACE: u8 = 65;
        if !matches!(keycode, UP | DOWN | LEFT | RIGHT | RETURN | KP_ENTER | ESCAPE | SPACE) {
            return Ok(false);
        }
        // The level being worked: the deepest open menu with a selection.
        let level = if self.nested_submenu.as_ref().is_some_and(|m| m.hover.is_some()) {
            2
        } else if self.submenu.as_ref().is_some_and(|m| m.hover.is_some()) {
            1
        } else {
            0
        };
        match keycode {
            UP | DOWN => {
                let Some(m) = self.menu_level(level) else { return Ok(true) };
                m.hover = bar::menu_step(&m.items(), m.hover, keycode == DOWN);
                match level {
                    2 => self.paint_nested_submenu()?,
                    1 => {
                        self.close_nested_submenu()?;
                        self.paint_submenu()?;
                    }
                    _ => {
                        self.close_submenu()?;
                        self.paint_menu()?;
                    }
                }
            }
            ESCAPE | LEFT => match level {
                2 => {
                    self.close_nested_submenu()?;
                    self.paint_submenu()?;
                }
                1 => {
                    self.close_submenu()?;
                    self.paint_menu()?;
                }
                _ if keycode == ESCAPE => self.close_menu()?,
                _ => {}
            },
            RIGHT | RETURN | KP_ENTER | SPACE => {
                let Some(m) = self.menu_level(level) else { return Ok(true) };
                let Some(idx) = m.hover else {
                    // Nothing chosen yet: start at the top.
                    m.hover = bar::menu_step(&m.items(), None, true);
                    return self.repaint_level(level).map(|()| true);
                };
                let is_flyout = matches!(m.choices[idx], Choice::Submenu(_));
                if keycode == RIGHT && !is_flyout {
                    return Ok(true);
                }
                let Some((cx, cy)) = bar::menu_item_center(&m.items(), idx, m.with_band, &m.layout) else { return Ok(true) };
                let (ax, ay) = (m.x + cx, m.y + cy);
                if is_flyout {
                    // Open it, and go into it.
                    if let Choice::Submenu(kind) = m.choices[idx].clone() {
                        if level == 0 {
                            self.open_submenu(kind, idx)?;
                        } else {
                            self.open_nested_submenu(kind, idx)?;
                        }
                    }
                    if let Some(child) = self.menu_level(level + 1) {
                        child.hover = bar::menu_step(&child.items(), None, true);
                    }
                    self.repaint_level(level + 1)?;
                } else {
                    // What a click on it does.
                    self.menu_click(ax, ay, 1)?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// The open menu at `level`: 0 the menu, 1 its flyout, 2 the flyout's.
    fn menu_level(&mut self, level: usize) -> Option<&mut Menu> {
        match level {
            2 => self.nested_submenu.as_mut(),
            1 => self.submenu.as_mut(),
            _ => self.menu.as_mut(),
        }
    }

    fn repaint_level(&mut self, level: usize) -> Res<()> {
        match level {
            2 => self.paint_nested_submenu(),
            1 => self.paint_submenu(),
            _ => self.paint_menu(),
        }
    }

    fn close_menu(&mut self) -> Res<()> {
        self.close_submenu()?;
        if let Some(menu) = self.menu.take() {
            let _ = self.conn.ungrab_pointer(CURRENT_TIME);
            let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
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
                    // Marked as a program, or the desktop shows it as a plain
                    // file (as it does any .desktop that came from elsewhere).
                    if std::fs::write(&path, content).is_ok() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
                        }
                    }
                    self.close_menu()?;
                    self.paint_desktop()?;
                    return Ok(());
                }
                if let Choice::PinTaskbar { name, exec } = &menu.choices[i] {
                    let icon = bar::pinned_icon(exec);
                    self.pinned_apps.push(bar::PinnedApp { name: name.clone(), exec: exec.clone(), icon });
                    bar::save_pinned(&self.pinned_apps);
                    self.bar_stale = true;
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::UnpinTaskbar(idx) = menu.choices[i] {
                    if idx < self.pinned_apps.len() {
                        self.pinned_apps.remove(idx);
                        bar::save_pinned(&self.pinned_apps);
                        self.bar_stale = true;
                    }
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::WindowRestore(f) = menu.choices[i] {
                    self.close_menu()?;
                    self.restore(f)?;
                    return Ok(());
                }
                if let Choice::WindowMinimize(f) = menu.choices[i] {
                    self.close_menu()?;
                    self.minimize(f)?;
                    return Ok(());
                }
                if let Choice::WindowMaximize(f) = menu.choices[i] {
                    self.close_menu()?;
                    self.toggle_maximize(f)?;
                    return Ok(());
                }
                if let Choice::WindowClose(f) = menu.choices[i] {
                    self.close_menu()?;
                    self.close(f)?;
                    return Ok(());
                }
                if let Choice::ToggleShowDesktop = menu.choices[i] {
                    self.close_menu()?;
                    self.toggle_show_desktop()?;
                    return Ok(());
                }
                if let Choice::Submenu(kind) = &menu.choices[i] {
                    let k = kind.clone();
                    self.open_submenu(k, i)?;
                    return Ok(());
                }
                if let Choice::CutDesktopSelection = menu.choices[i] {
                    self.desktop.cut_selected();
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::CopyDesktopSelection = menu.choices[i] {
                    self.desktop.copy_selected();
                    self.close_menu()?;
                    return Ok(());
                }
                if let Choice::PasteToDesktop = menu.choices[i] {
                    desktop::paste_clipboard_to_desktop();
                    self.desktop.refresh(self.screen_w, self.screen_h);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::DeleteDesktopItem(idx) = menu.choices[i] {
                    if self.desktop.selected_items.contains(&idx) {
                        self.desktop.delete_selected();
                    } else {
                        self.desktop.delete_item(idx);
                    }
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    self.keep_bin_current()?;
                    return Ok(());
                }
                if let Choice::RestartDesktop = menu.choices[i] {
                    self.close_menu()?;
                    restart_wm();
                }
                if let Choice::RenameDesktopItem(idx) = menu.choices[i] {
                    self.desktop.start_rename(idx);
                    self.close_menu()?;
                    self.paint_full_desktop()?;
                    return Ok(());
                }
                if let Choice::DesktopProperties(idx) = menu.choices[i] {
                    if let Some(item) = self.desktop.items.get(idx) {
                        let home = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("/home/ferro"));
                        let path = match &item.kind {
                            desktop::ItemKind::Folder(p) | desktop::ItemKind::File(p) => Some(p.clone()),
                            desktop::ItemKind::RecycleBin => Some(home.join(".local/share/Trash")),
                            desktop::ItemKind::Files => Some(home.clone()),
                            desktop::ItemKind::Shortcut { .. } => Some(home.join("Desktop").join(&item.id)),
                        };
                        if let Some(p) = path {
                            launch(&format!("ferro-properties {}", ferro_associations::shell_quote(&p.to_string_lossy())));
                        }
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
        let (name, exec, file) = match &choice {
            Choice::Run(app) => (app.name.clone(), app.exec.clone(), Some(app.file.clone())),
            Choice::Action { label, command, .. } => (label.to_string(), command.clone(), None),
            _ => return Ok(()),
        };

        let mut choices = vec![
            Choice::Action { label: "Open", command: exec.clone(), icon: Some(icons::FERRO) },
            Choice::PinTaskbar { name: name.clone(), exec: exec.clone() },
            Choice::CreateDesktopShortcut { name: name.clone(), exec: exec.clone() },
        ];

        if let Some(file) = file.filter(|f| !f.is_empty()) {
            choices.push(Choice::Separator);
            // By the program's own .desktop file, so what's removed is what
            // was clicked, not whatever package has a similar name.
            let q_file = ferro_associations::shell_quote(&file);
            let uninst_cmd = format!("ferro-terminal -e /usr/local/bin/ferro-uninstall {q_file}");
            choices.push(Choice::Action { label: "Uninstall...", command: uninst_cmd, icon: Some(icons::RECYCLE) });
            choices.push(Choice::Action { label: "Properties", command: format!("ferro-program-props {q_file}"), icon: Some(icons::FILE) });
        }

        self.spawn_menu(x, Some(y), choices, false, bar::CONTEXT_MENU_W, None)
    }

    #[allow(dead_code)]
    fn toggle_switch(&mut self, i: usize) -> Res<()> {
        match i {
            0 => {
                self.store.switches.network = !self.store.switches.network;
                let state = if self.store.switches.network { "on" } else { "off" };
                let _ = std::process::Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-switch-hardware", "net", state]).status();
            }
            1 => {
                self.store.switches.microphone = !self.store.switches.microphone;
                let state = if self.store.switches.microphone { "on" } else { "off" };
                let _ = std::process::Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-switch-hardware", "mic", state]).status();
            }
            2 => {
                self.store.switches.camera = !self.store.switches.camera;
                let state = if self.store.switches.camera { "on" } else { "off" };
                let _ = std::process::Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-switch-hardware", "cam", state]).status();
            }
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
                if detail == 3 {
                    if let Some(app) = self.pinned_apps.get(i) {
                        let choices = vec![
                            Choice::Action { label: "Open", command: app.exec.clone(), icon: Some(app.icon) },
                            Choice::Separator,
                            Choice::UnpinTaskbar(i),
                        ];
                        let menu_x = bar::pinned_rect(i).x.clamp(0, (self.screen_w - 180).max(0));
                        self.spawn_menu(menu_x, None, choices, false, 180, None)?;
                    }
                } else if let Some(exec) = self.pinned_apps.get(i).map(|a| a.exec.clone()) {
                    // Like a running program's button: its window comes up, or
                    // goes down if it's already in front; opened only if it isn't.
                    match self.pinned_window(&exec) {
                        Some(f) if self.windows.get(&f).is_some_and(|m| m.minimized) => self.restore(f)?,
                        Some(f) if self.active == Some(f) => self.minimize(f)?,
                        Some(f) => self.activate(f)?,
                        None => launch(&exec),
                    }
                }
            }
            BarPart::TrayIcon(0) => launch("ferro-network"),
            BarPart::TrayIcon(1) => match detail {
                3 => launch("amixer -q sset Master toggle || pactl set-sink-mute @DEFAULT_SINK@ toggle"),
                4 => launch("amixer -q sset Master 5%+ || pactl set-sink-volume @DEFAULT_SINK@ +5%"),
                5 => launch("amixer -q sset Master 5%- || pactl set-sink-volume @DEFAULT_SINK@ -5%"),
                _ => launch("ferro-volume"),
            },
            BarPart::TrayIcon(_) => {}
            BarPart::TrayClock => {
                if detail == 3 {
                    let choices = vec![
                        Choice::Action { label: "Date and Time...", command: "ferro-datetime".into(), icon: Some(icons::CLOCK) },
                        Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) },
                    ];
                    let menu_x = x.clamp(0, (self.screen_w - 180).max(0));
                    self.spawn_menu(menu_x, None, choices, false, 180, None)?;
                } else {
                    launch("ferro-datetime");
                }
            }
            BarPart::TrayRam | BarPart::Tray => {
                if detail == 3 {
                    let choices = vec![
                        Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) },
                        Choice::Action { label: "Date and Time...", command: "ferro-datetime".into(), icon: Some(icons::CLOCK) },
                    ];
                    let menu_x = x.clamp(0, (self.screen_w - 180).max(0));
                    self.spawn_menu(menu_x, None, choices, false, 180, None)?;
                } else {
                    launch("ferro-taskmgr");
                }
            }
            BarPart::Task(i) => {
                let f = self.tasks[i];
                if detail == 3 {
                    let Some(m) = self.windows.get(&f) else { return Ok(()) };
                    let class = m.class.clone();
                    let title = m.title.clone();
                    let minimized = m.minimized;
                    let class_lower = class.to_lowercase();
                    let (client, pid) = (m.client, m.pid);
                    // A file manager window pins the folder it shows, not a program.
                    let (app_name, app_exec) = if let Some(dir) = self.window_folder(client) {
                        let name = dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                        (name, format!("ferro-files {}", ferro_associations::shell_quote(&dir.to_string_lossy())))
                    } else if let Some(app) = self.get_apps().iter().find(|a| {
                        !class_lower.is_empty()
                            && (a.name.to_lowercase() == class_lower
                                || a.exec.to_lowercase().starts_with(&class_lower)
                                || a.exec.to_lowercase().contains(&class_lower))
                    }) {
                        (app.name.clone(), app.exec.clone())
                    } else {
                        let clean_name = if class.is_empty() { title } else { class.clone() };
                        // No class to go by: the program the window's process runs, if
                        // that can be told; never some other program in its place.
                        let bin = if class.is_empty() {
                            pid.and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok())
                                .map(|exe| ferro_associations::shell_quote(&exe.to_string_lossy()))
                                .unwrap_or_default()
                        } else {
                            class.to_lowercase()
                        };
                        (clean_name, bin)
                    };

                    let pinned_idx = self.pinned_apps.iter().position(|p| p.exec == app_exec || (!app_name.is_empty() && p.name == app_name));

                    let mut choices = vec![
                        if minimized { Choice::WindowRestore(f) } else { Choice::WindowMinimize(f) },
                        Choice::WindowMaximize(f),
                        Choice::Separator,
                    ];

                    if let Some(pidx) = pinned_idx {
                        choices.push(Choice::UnpinTaskbar(pidx));
                    } else if !app_exec.is_empty() {
                        choices.push(Choice::PinTaskbar { name: app_name, exec: app_exec });
                    }

                    choices.push(Choice::Separator);
                    choices.push(Choice::WindowClose(f));

                    let menu_x = x.clamp(0, (self.screen_w - 180).max(0));
                    self.spawn_menu(menu_x, None, choices, false, 180, None)?;
                } else if detail == 1 {
                    let Some(m) = self.windows.get(&f) else { return Ok(()) };
                    let was_active = self.active == Some(f);
                    if m.minimized {
                        self.restore(f)?;
                    } else if !was_active {
                        self.activate(f)?;
                    }
                    self.drag = Some(Drag::TaskBar { task: i, start_x: x, moved: false, was_active });
                    let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                    let _ = self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
                }
            }
            BarPart::Nothing => {
                if detail == 3 {
                    let choices = vec![
                        Choice::Action { label: "Task Manager", command: "ferro-taskmgr".into(), icon: Some(icons::TASKMGR) },
                        Choice::ToggleShowDesktop,
                        Choice::Separator,
                        Choice::Action { label: "Volume Control...", command: "ferro-volume".into(), icon: Some(icons::SPEAKER) },
                        Choice::Action { label: "Display Settings...", command: "ferro-appearance".into(), icon: Some(icons::COMPUTER) },
                        Choice::Action { label: "Network Connections...", command: "ferro-network".into(), icon: Some(icons::NETWORK) },
                        Choice::Action { label: "Date and Time...", command: "ferro-datetime".into(), icon: Some(icons::CLOCK) },
                    ];
                    let menu_x = x.clamp(0, (self.screen_w - 180).max(0));
                    self.spawn_menu(menu_x, None, choices, false, 180, None)?;
                }
            }
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
            a.net_wm_state_demands_attention,
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

        let ((w, h), (x, y)) = if !is_dialog {
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
        let (x, y, w, h) = fit_rect(x, y, w, h, self.screen_w, self.work_h());

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
        self.send_configure_notify(client, x + cx, y + cy, cw, ch)?;

        let title = self.title_of(client);
        let pid = self
            .conn
            .get_property(false, client, self.atoms.net_wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        let managed = Managed {
            client,
            frame: frame_win,
            pid,
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
            urgent: false,
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
        if m.class.contains("ferro-rename") || m.title == "Rename" {
            self.desktop.refresh(self.screen_w, self.screen_h);
            self.paint_full_desktop()?;
        }
        Ok(())
    }

    /// Brings a window to the front and gives it the keyboard.
    fn activate(&mut self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get_mut(&frame_win) else { return Ok(()) };
        m.urgent = false;
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
        self.send_configure_notify(client, x + cx, y + cy, cw, ch)?;
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

    /// The folder a file manager window is showing (it says so in a property).
    /// The topmost window (its frame) at `(x, y)` on the screen, if any.
    fn frame_at(&self, x: i32, y: i32) -> Option<Window> {
        self.order
            .iter()
            .rev()
            .copied()
            .find(|f| self.windows.get(f).is_some_and(|m| !m.minimized && x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h))
    }

    fn window_folder(&self, client: Window) -> Option<std::path::PathBuf> {
        let reply = self.conn.get_property(false, client, self.atoms.ferro_current_dir, AtomEnum::ANY, 0, 1024).ok()?.reply().ok()?;
        let dir = std::path::PathBuf::from(String::from_utf8(reply.value).ok()?);
        dir.is_dir().then_some(dir)
    }

    /// A window already open for a pin: the folder's own file manager window
    /// for a pinned folder, else one of the pinned program's (by its class).
    fn pinned_window(&self, exec: &str) -> Option<Window> {
        let newest_first = self.order.iter().rev().copied();
        if let Some(folder) = bar::pinned_folder(exec) {
            return newest_first
                .into_iter()
                .find(|f| self.windows.get(f).is_some_and(|m| self.window_folder(m.client).as_deref() == Some(folder.as_path())));
        }
        let program = exec.split_whitespace().next().map(|p| p.trim_matches('\'').rsplit('/').next().unwrap_or(p).to_lowercase())?;
        newest_first.into_iter().find(|f| self.windows.get(f).is_some_and(|m| !m.class.is_empty() && m.class.to_lowercase() == program))
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
    /// program that doesn't know how is disconnected. Also monitors the process
    /// tree to ensure background worker processes (like Electron/Claude) do not linger,
    /// and triggers cache/memory compaction to wipe unmapped RAM buffers.
    fn close(&self, frame_win: Window) -> Res<()> {
        let Some(m) = self.windows.get(&frame_win) else { return Ok(()) };
        let pid = m.pid;
        if self.atom_list(m.client, self.atoms.wm_protocols).contains(&self.atoms.wm_delete_window) {
            let msg = ClientMessageEvent::new(32, m.client, self.atoms.wm_protocols, [self.atoms.wm_delete_window, CURRENT_TIME, 0, 0, 0]);
            self.conn.send_event(false, m.client, EventMask::NO_EVENT, msg)?;
        } else {
            self.conn.kill_client(m.client)?;
        }
        self.conn.flush()?;

        if let Some(p) = pid {
            let other_has_pid = self.windows.iter().any(|(&f, win)| f != frame_win && win.pid == Some(p));
            if !other_has_pid {
                std::thread::spawn(move || {
                    // Give the application 1.2s to cleanly exit on its own
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    let proc_path = format!("/proc/{p}");
                    if !std::path::Path::new(&proc_path).exists() {
                        let _ = std::process::Command::new("sudo")
                            .args([
                                "-n",
                                "sh",
                                "-c",
                                "sync; echo 3 > /proc/sys/vm/drop_caches 2>/dev/null; echo 1 > /proc/sys/vm/compact_memory 2>/dev/null",
                            ])
                            .status();
                        return;
                    }

                    // Recursively collect all descendant process IDs
                    let mut to_kill = vec![p];
                    if let Ok(entries) = std::fs::read_dir("/proc") {
                        let mut ppid_map: Vec<(u32, u32)> = Vec::new();
                        for entry in entries.flatten() {
                            if let Ok(name) = entry.file_name().into_string() {
                                if let Ok(parsed_pid) = name.parse::<u32>() {
                                    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{parsed_pid}/stat")) {
                                        if let Some(idx) = stat.rfind(')') {
                                            let rest = &stat[idx + 1..];
                                            let parts: Vec<&str> = rest.split_whitespace().collect();
                                            if parts.len() >= 2 {
                                                if let Ok(ppid) = parts[1].parse::<u32>() {
                                                    ppid_map.push((parsed_pid, ppid));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let mut i = 0;
                        while i < to_kill.len() {
                            let parent = to_kill[i];
                            for &(child, parent_id) in &ppid_map {
                                if parent_id == parent && !to_kill.contains(&child) {
                                    to_kill.push(child);
                                }
                            }
                            i += 1;
                        }
                    }

                    // Polite SIGTERM from leaf processes to root
                    for &target_pid in to_kill.iter().rev() {
                        unsafe {
                            libc::kill(target_pid as i32, libc::SIGTERM);
                        }
                    }

                    std::thread::sleep(std::time::Duration::from_millis(600));

                    // Force SIGKILL on anything still lingering
                    for &target_pid in to_kill.iter().rev() {
                        if std::path::Path::new(&format!("/proc/{target_pid}")).exists() {
                            unsafe {
                                libc::kill(target_pid as i32, libc::SIGKILL);
                            }
                        }
                    }

                    // Reclaim and wipe memory buffers for privacy & security
                    let _ = std::process::Command::new("sudo")
                        .args([
                            "-n",
                            "sh",
                            "-c",
                            "sync; echo 3 > /proc/sys/vm/drop_caches 2>/dev/null; echo 1 > /proc/sys/vm/compact_memory 2>/dev/null",
                        ])
                        .status();
                });
            }
        }
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
            Event::LeaveNotify(e) => {
                if e.event == self.bar && self.tray_tooltip.is_some() {
                    self.tray_tooltip = None;
                    self.bar_stale = true;
                }
            }
            Event::Expose(e) => {
                if e.window == self.bar {
                    if e.count == 0 {
                        self.bar_stale = true;
                    }
                } else if self.menu.as_ref().is_some_and(|m| m.window == e.window) {
                    if e.count == 0 {
                        self.paint_menu()?;
                    }
                } else if self.submenu.as_ref().is_some_and(|s| s.window == e.window) {
                    if e.count == 0 {
                        self.paint_submenu()?;
                    }
                } else if self.nested_submenu.as_ref().is_some_and(|s| s.window == e.window) {
                    if e.count == 0 {
                        self.paint_nested_submenu()?;
                    }
                } else if e.window == self.desktop_win {
                    if e.count == 0 {
                        let r = Rect::new(i32::from(e.x), i32::from(e.y), i32::from(e.width), i32::from(e.height));
                        self.paint_desktop_rect(r)?;
                    }
                } else if e.count == 0 {
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
                    if let Ok(cookie) = self.conn.get_property(false, self.root, self.atoms.ferro_switches, AtomEnum::CARDINAL, 0, 5) {
                        if let Ok(reply) = cookie.reply() {
                            if let Some(mut iter) = reply.value32() {
                                if let (Some(net), Some(mic), Some(cam), Some(vpn), Some(tor)) =
                                    (iter.next(), iter.next(), iter.next(), iter.next(), iter.next())
                                {
                                    self.store.switches.network = net != 0;
                                    self.store.switches.microphone = mic != 0;
                                    self.store.switches.camera = cam != 0;
                                    self.store.switches.vpn = vpn != 0;
                                    self.store.switches.tor = tor != 0;
                                }
                            }
                        }
                    }
                    if let Ok(home) = std::env::var("HOME") {
                        let conf = std::path::PathBuf::from(home).join(".config").join("ferro").join("switches.conf");
                        if let Ok(text) = std::fs::read_to_string(conf) {
                            for line in text.lines() {
                                if let Some((k, v)) = line.split_once('=') {
                                    let b = v.trim() == "true" || v.trim() == "1" || v.trim() == "on";
                                    match k.trim() {
                                        "network" => self.store.switches.network = b,
                                        "microphone" => self.store.switches.microphone = b,
                                        "camera" => self.store.switches.camera = b,
                                        "vpn" => self.store.switches.vpn = b,
                                        "tor" => self.store.switches.tor = b,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
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
                    self.paint_desktop()?;
                    let frames: Vec<Window> = self.windows.keys().copied().collect();
                    for f in frames {
                        let _ = self.paint(f, None);
                    }
                    self.bar_stale = true;
                } else if e.atom == self.atoms.wm_hints {
                    if let Some(f) = self.by_client(e.window) {
                        if let Ok(reply) = self.conn.get_property(false, e.window, self.atoms.wm_hints, AtomEnum::WM_HINTS, 0, 9)?.reply() {
                            if let Some(mut data) = reply.value32() {
                                if let Some(flags) = data.next() {
                                    let urgent = (flags & (1 << 8)) != 0;
                                    if let Some(m) = self.windows.get_mut(&f) {
                                        if self.active != Some(f) && m.urgent != urgent {
                                            m.urgent = urgent;
                                            self.bar_stale = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Event::ClientMessage(e) => self.client_message(e)?,
            Event::KeyPress(e) if self.menu.is_some() && self.menu_key(e.detail)? => {}
            Event::KeyPress(e) => match shortcut(u16::from(e.state), e.detail).or_else(|| {
                let held = u16::from(e.state) & (SHIFT | CONTROL | ALT | SUPER);
                (held == 0 && self.print_keys.contains(&e.detail)).then_some(Shortcut::Snip)
            }) {
                Some(Shortcut::TaskManager) => launch("ferro-taskmgr"),
                Some(Shortcut::StartMenu) if self.menu.is_some() => self.close_menu()?,
                Some(Shortcut::StartMenu) => self.open_menu()?,
                Some(Shortcut::Files) => launch("ferro-files"),
                Some(Shortcut::Snip) => launch("ferro-snip"),
                Some(Shortcut::ShowDesktop) => self.toggle_show_desktop()?,
                Some(Shortcut::Panic) => launch("sudo -n /sbin/poweroff -ff || systemctl poweroff -i"),
                Some(Shortcut::LockScreen) => {
                    launch("ferro-screensaver --lock || until ferro-logon /usr/local/lib/ferro-heavy/check-password \"${XDG_RUNTIME_DIR:-/tmp}/ferro-lock-name\"; do sleep 1; done")
                }
                Some(Shortcut::CloseWindow) => {
                    if let Some(frame_win) = self.active {
                        self.close(frame_win)?;
                    }
                }
                None => {
                    if self.desktop.renaming.is_some() {
                        let key = self.keymap.typed(&e);
                        match key {
                            ferro_ui::Key::Enter => {
                                self.desktop.commit_rename();
                                self.paint_full_desktop()?;
                            }
                            ferro_ui::Key::Escape => {
                                self.desktop.cancel_rename();
                                self.paint_full_desktop()?;
                            }
                            ferro_ui::Key::Backspace => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    if ren.sel_start < ren.sel_end && ren.sel_end <= ren.text.len() {
                                        ren.text.drain(ren.sel_start..ren.sel_end);
                                        ren.cursor = ren.sel_start;
                                        ren.sel_end = ren.sel_start;
                                        self.paint_full_desktop()?;
                                    } else if ren.cursor > 0 {
                                        ren.cursor -= 1;
                                        ren.text.remove(ren.cursor);
                                        self.paint_full_desktop()?;
                                    }
                                }
                            }
                            ferro_ui::Key::Delete => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    if ren.sel_start < ren.sel_end && ren.sel_end <= ren.text.len() {
                                        ren.text.drain(ren.sel_start..ren.sel_end);
                                        ren.cursor = ren.sel_start;
                                        ren.sel_end = ren.sel_start;
                                        self.paint_full_desktop()?;
                                    } else if ren.cursor < ren.text.len() {
                                        ren.text.remove(ren.cursor);
                                        self.paint_full_desktop()?;
                                    }
                                }
                            }
                            ferro_ui::Key::Left => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    if ren.sel_start < ren.sel_end {
                                        ren.cursor = ren.sel_start;
                                        ren.sel_end = ren.sel_start;
                                    } else if ren.cursor > 0 {
                                        ren.cursor -= 1;
                                    }
                                    self.paint_full_desktop()?;
                                }
                            }
                            ferro_ui::Key::Right => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    if ren.sel_start < ren.sel_end {
                                        ren.cursor = ren.sel_end;
                                        ren.sel_start = ren.sel_end;
                                    } else if ren.cursor < ren.text.len() {
                                        ren.cursor += 1;
                                    }
                                    self.paint_full_desktop()?;
                                }
                            }
                            ferro_ui::Key::Home => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    ren.cursor = 0;
                                    ren.sel_start = 0;
                                    ren.sel_end = 0;
                                    self.paint_full_desktop()?;
                                }
                            }
                            ferro_ui::Key::End => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    ren.cursor = ren.text.len();
                                    ren.sel_start = ren.cursor;
                                    ren.sel_end = ren.cursor;
                                    self.paint_full_desktop()?;
                                }
                            }
                            ferro_ui::Key::Char(c) if !c.is_control() => {
                                if let Some(ren) = &mut self.desktop.renaming {
                                    if ren.sel_start < ren.sel_end && ren.sel_end <= ren.text.len() {
                                        ren.text.drain(ren.sel_start..ren.sel_end);
                                        ren.cursor = ren.sel_start;
                                        ren.sel_end = ren.sel_start;
                                    }
                                    ren.text.insert(ren.cursor, c);
                                    ren.cursor += 1;
                                    ren.sel_start = ren.cursor;
                                    ren.sel_end = ren.cursor;
                                    self.paint_full_desktop()?;
                                }
                            }
                            _ => {}
                        }
                    } else if self.active.is_none() || e.event == self.root {
                        let ctrl = u16::from(e.state) & KeyButMask::CONTROL.bits() != 0;
                        if e.detail == KEY_DELETE {
                            if self.desktop.delete_selected() {
                                self.desktop.refresh(self.screen_w, self.screen_h);
                                self.paint_desktop_surface()?;
                                self.keep_bin_current()?;
                            }
                        } else if ctrl && e.detail == KEY_C {
                            self.desktop.copy_selected();
                        } else if ctrl && e.detail == KEY_X {
                            self.desktop.cut_selected();
                        } else if ctrl && e.detail == KEY_V {
                            if desktop::paste_clipboard_to_desktop().is_some() {
                                self.desktop.refresh(self.screen_w, self.screen_h);
                                self.paint_desktop_surface()?;
                            }
                        } else if ctrl && e.detail == KEY_A {
                            self.desktop.select_all();
                            self.paint_desktop_surface()?;
                        } else if e.detail == KEY_F5 {
                            self.desktop.refresh(self.screen_w, self.screen_h);
                            self.paint_desktop_surface()?;
                        } else if e.detail == KEY_RETURN {
                            if let Some(sel) = self.desktop.selected {
                                if let Some(item) = self.desktop.items.get(sel) {
                                    launch(&item.exec_command());
                                }
                            }
                        } else if e.detail == KEY_F2 {
                            if let Some(sel) = self.desktop.selected {
                                self.desktop.start_rename(sel);
                                self.paint_full_desktop()?;
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
        if e.value_mask.contains(ConfigWindow::STACK_MODE) && e.stack_mode == StackMode::ABOVE {
            let _ = self.restore(f);
        }
        let m = &self.windows[&f];
        if m.fullscreen || m.maximized {
            // It stays where it is; tell it so.
            return self.place(f, m.x, m.y, m.w, m.h);
        }
        let (mut w, mut h) = (m.w, m.h);
        if e.value_mask.contains(ConfigWindow::WIDTH) {
            w = i32::from(e.width) + frame::LEFT + frame::RIGHT;
        }
        if e.value_mask.contains(ConfigWindow::HEIGHT) {
            h = i32::from(e.height) + frame::TOP + frame::BOTTOM;
        }
        // Retain m.x and m.y for managed windows: do not allow client to reset window manager position
        let (x, y, w, h) = fit_rect(m.x, m.y, w, h, self.screen_w, self.work_h());
        self.place(f, x, y, w, h)
    }

    fn client_message(&mut self, e: ClientMessageEvent) -> Res<()> {
        if e.type_ == self.atoms.ferro_place {
            // The file manager dropped these on the desktop at (x, y): they go
            // there once they've arrived (copying may take a moment).
            let reply = self.conn.get_property(true, self.root, self.atoms.ferro_place, AtomEnum::ANY, 0, 65536)?.reply()?;
            let names: Vec<String> = String::from_utf8_lossy(&reply.value).lines().filter(|l| !l.is_empty()).map(str::to_owned).collect();
            let d = e.data.as_data32();
            self.desktop.place_at(&names, d[0] as i32, d[1] as i32);
            if self.desktop.refresh_pending(self.screen_w, self.screen_h) {
                self.paint_full_desktop()?;
            }
            return Ok(());
        }
        if e.type_ == self.atoms.ferro_info {
            // The Image Viewer and the Video Player may start nothing: they
            // ask, and only the Info window, only for a file, is opened.
            let reply = self.conn.get_property(true, self.root, self.atoms.ferro_info, AtomEnum::ANY, 0, 4096)?.reply()?;
            let path = std::path::PathBuf::from(String::from_utf8_lossy(&reply.value).into_owned());
            if path.is_file() {
                launch(&format!("ferro-info {}", ferro_associations::shell_quote(&path.to_string_lossy())));
            }
            return Ok(());
        }
        if e.type_ == self.atoms.ferro_refresh {
            // Programs may have come or gone, or moved Start menu folder.
            self.app_cache = None;
            self.desktop.refresh(self.screen_w, self.screen_h);
            self.paint_full_desktop()?;
            return Ok(());
        }
        if e.type_ == self.atoms.ferro_tick {
            self.flash_tick = self.flash_tick.wrapping_add(1);
            self.keep_bin_current()?;
            // Files dropped from the file manager that have arrived since.
            if self.desktop.refresh_pending(self.screen_w, self.screen_h) {
                self.paint_full_desktop()?;
            }
            // Thumbnails made aside since the last tick (pictures, videos).
            if self.desktop.take_thumbnails() {
                self.paint_full_desktop()?;
            }
            let has_urgent = self.windows.values().any(|m| m.urgent && self.active != Some(m.frame));

            // Only check memory footprint every 20 ticks (10 seconds) or when clock minute changes
            let cur_clock = clock_text();
            let clock_changed = cur_clock != self.last_clock_text;
            let check_mem = self.flash_tick.is_multiple_of(20);

            if has_urgent || clock_changed || check_mem {
                if clock_changed {
                    self.last_clock_text = cur_clock;
                }
                let cur_tray = tray_text();
                if cur_tray != self.last_tray_text || has_urgent {
                    self.last_tray_text = cur_tray;
                    self.bar_stale = true;
                }
            }
            return Ok(());
        }
        if e.type_ == self.atoms.net_active_window {
            let data = e.data.as_data32();
            let target = self.by_client(e.window).or_else(|| self.by_client(data[0]));
            if let Some(f) = target {
                self.restore(f)?;
            }
            return Ok(());
        }
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
        } else if e.type_ == self.atoms.net_wm_state
            && (data[1] == self.atoms.net_wm_state_demands_attention || data[2] == self.atoms.net_wm_state_demands_attention)
        {
            let on = match data[0] {
                0 => false,
                1 => true,
                _ => !self.windows.get(&f).map(|m| m.urgent).unwrap_or(false),
            };
            if let Some(m) = self.windows.get_mut(&f) {
                if self.active != Some(f) && m.urgent != on {
                    m.urgent = on;
                    self.bar_stale = true;
                }
            }
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
        if e.event == self.root || e.event == self.desktop_win {
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
        if self.desktop.renaming.is_some() {
            self.desktop.commit_rename();
            self.paint_full_desktop()?;
        }
        let (x, y) = (i32::from(e.root_x), i32::from(e.root_y));
        let hit = self.desktop.hit(x, y);
        if let Some(prev) = self.active.take() {
            let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME);
            let _ = self.conn.delete_property(self.root, self.atoms.net_active_window);
            let _ = self.paint(prev, None);
            self.bar_stale = true;
        }
        let ctrl = u16::from(e.state) & KeyButMask::CONTROL.bits() != 0;
        if let Some(old_h) = self.desktop.hovered.take() {
            if let Some(it) = self.desktop.items.get(old_h) {
                let _ = self.paint_desktop_rect(it.visual_rect());
            }
        }
        if e.detail == 1 {
            let now = std::time::Instant::now();
            let is_double_click = match self.last_desktop_click {
                Some((prev_time, prev_hit)) => hit.is_some() && hit == Some(prev_hit) && now.duration_since(prev_time).as_millis() < 650,
                None => false,
            };

            if is_double_click {
                self.last_desktop_click = None;
                if let Some(idx) = hit {
                    if let Some(item) = self.desktop.items.get(idx) {
                        let item_title = item.label.to_lowercase();
                        let cmd = item.exec_command();
                        let existing = self
                            .windows
                            .iter()
                            .find(|(_, m)| {
                                let t = m.title.to_lowercase();
                                t == item_title
                                    || t.starts_with(&format!("{item_title} -"))
                                    || t.ends_with(&format!("- {item_title}"))
                                    || t.contains(&format!(" {item_title} "))
                            })
                            .map(|(&f, _)| f);

                        if let Some(f) = existing {
                            let _ = self.restore(f);
                        }
                        launch(&cmd);
                    }
                }
                self.desktop.clear_selection();
                self.drag = None;
                self.paint_full_desktop()?;
                return Ok(());
            }
            self.last_desktop_click = hit.map(|idx| (now, idx));
            let old_sel = self.desktop.selected_items.clone();
            if let Some(idx) = hit {
                if ctrl {
                    if self.desktop.selected_items.contains(&idx) {
                        self.desktop.selected_items.remove(&idx);
                        if self.desktop.selected == Some(idx) {
                            self.desktop.selected = self.desktop.selected_items.iter().copied().next();
                        }
                    } else {
                        self.desktop.selected_items.insert(idx);
                        self.desktop.selected = Some(idx);
                    }
                } else if !self.desktop.is_selected(idx) {
                    self.desktop.set_single_selected(Some(idx));
                } else {
                    self.desktop.selected = Some(idx);
                }
                let item = &self.desktop.items[idx];
                let mut others = Vec::new();
                if self.desktop.selected_items.contains(&idx) {
                    for &other_idx in &self.desktop.selected_items {
                        if other_idx != idx {
                            if let (Some(cur), Some(oth)) = (self.desktop.items.get(idx), self.desktop.items.get(other_idx)) {
                                others.push((other_idx, oth.x - cur.x, oth.y - cur.y));
                            }
                        }
                    }
                }
                self.drag = Some(Drag::Desktop { item: idx, start: (x, y), origin: (item.x, item.y), moved: false, others });
                let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                let _ =
                    self.conn.grab_pointer(false, self.desktop_win, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
                let to_paint: Vec<Rect> = self
                    .desktop
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| old_sel.contains(i) != self.desktop.selected_items.contains(i))
                    .map(|(_, it)| it.visual_rect())
                    .collect();
                for r in to_paint {
                    self.paint_desktop_rect(r)?;
                }
            } else {
                self.desktop.clear_selection();
                self.drag = Some(Drag::DesktopSelect { start_x: x, start_y: y, cur_x: x, cur_y: y });
                let mask = EventMask::BUTTON_RELEASE | EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
                let _ =
                    self.conn.grab_pointer(false, self.desktop_win, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
                let to_paint: Vec<Rect> =
                    self.desktop.items.iter().enumerate().filter(|(i, _)| old_sel.contains(i)).map(|(_, it)| it.visual_rect()).collect();
                for r in to_paint {
                    self.paint_desktop_rect(r)?;
                }
            }
        } else if e.detail == 3 {
            let old_sel = self.desktop.selected_items.clone();
            if let Some(idx) = hit {
                if !self.desktop.is_selected(idx) {
                    self.desktop.set_single_selected(Some(idx));
                }
            } else {
                self.desktop.clear_selection();
            }
            let to_paint: Vec<Rect> = self
                .desktop
                .items
                .iter()
                .enumerate()
                .filter(|(i, _)| old_sel.contains(i) != self.desktop.selected_items.contains(i))
                .map(|(_, it)| it.visual_rect())
                .collect();
            for r in to_paint {
                self.paint_desktop_rect(r)?;
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
            let prev_sub_hover = self.submenu.as_ref().and_then(|m| m.hover);
            let (inside_sub, sub_changed, open_nested) = if let Some(sub) = &mut self.submenu {
                let inside = (sub.x..sub.x + sub.width).contains(&rx) && (sub.y..sub.y + sub.layout.height).contains(&ry);
                if inside {
                    let hover = bar::menu_hit_with_layout(&sub.items(), rx - sub.x, ry - sub.y, false, &sub.layout);
                    // A gap or separator changes nothing.
                    if hover.is_some() && hover != sub.hover {
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

            if inside_sub && sub_changed && self.heading_into(self.nested_submenu.as_ref(), self.submenu.as_ref(), rx, ry) {
                // On the way to the open flyout: what's passed over doesn't
                // count, so the flyout stays open.
                if let Some(sub) = &mut self.submenu {
                    sub.hover = prev_sub_hover;
                }
                self.menu_pointer = (rx, ry);
                return Ok(());
            }
            if inside_sub {
                self.menu_pointer = (rx, ry);
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
            let prev_menu_hover = self.menu.as_ref().and_then(|m| m.hover);
            let (inside_menu, hover_changed, open_sub) = {
                let menu = self.menu.as_mut().unwrap();
                let menu_h = menu.layout.height;
                let inside_menu = (menu.x..menu.x + menu.width).contains(&rx) && (menu.y..menu.y + menu_h).contains(&ry);
                if inside_menu {
                    let hover = bar::menu_hit_with_layout(&menu.items(), rx - menu.x, ry - menu.y, menu.with_band, &menu.layout);
                    if hover.is_some() && hover != menu.hover {
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

            if inside_menu && hover_changed && self.heading_into(self.submenu.as_ref(), self.menu.as_ref(), rx, ry) {
                if let Some(menu) = &mut self.menu {
                    menu.hover = prev_menu_hover;
                }
                self.menu_pointer = (rx, ry);
                return Ok(());
            }
            self.menu_pointer = (rx, ry);
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
            Some(Drag::TaskBar { task, start_x, ref mut moved, was_active }) => {
                if (rx - start_x).abs() > 4 {
                    *moved = true;
                }
                let hit = bar::hit(self.screen_w, self.tasks.len(), self.pinned_apps.len(), rx, bar::HEIGHT / 2);
                if let BarPart::Task(target_i) = hit {
                    if target_i != task && target_i < self.tasks.len() {
                        self.tasks.swap(task, target_i);
                        self.drag = Some(Drag::TaskBar { task: target_i, start_x: rx, moved: true, was_active });
                        self.paint_bar()?;
                    }
                }
            }
            Some(Drag::DesktopSelect { start_x, start_y, ref mut cur_x, ref mut cur_y }) => {
                let old_rx = start_x.min(*cur_x);
                let old_ry = start_y.min(*cur_y);
                let old_rw = (start_x.max(*cur_x) - old_rx).max(1);
                let old_rh = (start_y.max(*cur_y) - old_ry).max(1);
                let old_rb = Rect::new(old_rx, old_ry, old_rw, old_rh);

                *cur_x = rx;
                *cur_y = ry;

                let new_rx = start_x.min(rx);
                let new_ry = start_y.min(ry);
                let new_rw = (start_x.max(rx) - new_rx).max(1);
                let new_rh = (start_y.max(ry) - new_ry).max(1);
                let new_rb = Rect::new(new_rx, new_ry, new_rw, new_rh);

                if old_rb != new_rb {
                    let old_sel = self.desktop.selected_items.clone();
                    self.desktop.update_rubberband(new_rb);

                    let to_paint: Vec<Rect> = self
                        .desktop
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| old_sel.contains(i) != self.desktop.selected_items.contains(i))
                        .map(|(_, it)| it.visual_rect())
                        .collect();
                    for r in to_paint {
                        self.paint_desktop_rect(r)?;
                    }

                    for strip in self.border_strips(old_rb) {
                        self.paint_desktop_rect(strip)?;
                    }

                    for strip in self.border_strips(new_rb) {
                        self.paint_desktop_rect(strip)?;
                    }
                }
            }
            Some(Drag::Desktop { item, start, origin, ref mut moved, ref others }) => {
                if !*moved {
                    if (rx - start.0).abs() < 4 && (ry - start.1).abs() < 4 {
                        return Ok(());
                    }
                    *moved = true;
                }
                // Over a window the icons (on the desktop, under it) can't be
                // seen: what's dragged is drawn beside the pointer instead.
                let over_window = self.frame_at(rx, ry).is_some();
                match (&self.drag_icon, over_window) {
                    (Some(d), true) => d.move_to(&self.conn, (rx, ry))?,
                    (None, true) => {
                        if let Some(it) = self.desktop.items.get(item) {
                            let label = if others.is_empty() { it.label.clone() } else { format!("{} items", others.len() + 1) };
                            let thumb = if others.is_empty() { self.desktop.thumbnails.get(&it.id) } else { None };
                            let screen = self.conn.setup().roots[0].clone();
                            // The icon sits in the middle of the item, at the top.
                            let icon_x = origin.0 + (desktop::ICON_W - desktop::ICON_SIZE) / 2;
                            let grab = (start.0 - icon_x, start.1 - (origin.1 + 2));
                            self.drag_icon = Some(ferro_ui::DragIcon::new(&self.conn, &screen, it.icon, thumb, &label, (rx, ry), grab)?);
                        }
                    }
                    (Some(_), false) => {
                        if let Some(d) = self.drag_icon.take() {
                            d.close(&self.conn)?;
                        }
                    }
                    (None, false) => {}
                }
                let (dx, dy) = (rx - start.0, ry - start.1);
                let new_x = origin.0 + dx;
                let new_y = origin.1 + dy;

                let mut dirty = Rect::default();
                if let Some(it) = self.desktop.items.get(item) {
                    dirty = it.visual_rect();
                }
                self.desktop.move_item(item, new_x, new_y, self.screen_w, self.screen_h);
                if let Some(it) = self.desktop.items.get(item) {
                    dirty = dirty.union(&it.visual_rect());
                }
                for &(other, ox, oy) in others {
                    if let Some(it) = self.desktop.items.get(other) {
                        dirty = dirty.union(&it.visual_rect());
                    }
                    self.desktop.move_item(other, new_x + ox, new_y + oy, self.screen_w, self.screen_h);
                    if let Some(it) = self.desktop.items.get(other) {
                        dirty = dirty.union(&it.visual_rect());
                    }
                }

                if dirty.w > 0 && dirty.h > 0 {
                    self.paint_desktop_rect(dirty)?;
                }
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
                if ry >= self.work_h() {
                    let hit = bar::hit(self.screen_w, self.tasks.len(), self.pinned_apps.len(), rx, ry - self.work_h());
                    let new_tip = match hit {
                        BarPart::TrayClock => Some((full_date_text(), bar::tray_clock_rect(self.screen_w))),
                        BarPart::TrayIcon(0) => Some(("Network: Connected".into(), bar::tray_icon_rect(self.screen_w, 0))),
                        BarPart::TrayIcon(1) => Some(("Volume".into(), bar::tray_icon_rect(self.screen_w, 1))),
                        BarPart::TrayRam => Some((format!("Memory: {}", tray_text()), bar::tray_ram_rect(self.screen_w))),
                        _ => None,
                    };
                    if new_tip != self.tray_tooltip {
                        self.tray_tooltip = new_tip;
                        self.paint_bar()?;
                    }
                } else if self.tray_tooltip.is_some() {
                    self.tray_tooltip = None;
                    self.paint_bar()?;
                }

                let new_hover = self.desktop.hit(rx, ry);
                if new_hover != self.desktop.hovered {
                    let old_hover = self.desktop.hovered;
                    self.desktop.hovered = new_hover;
                    let mut dirty = Rect::default();
                    if let Some(idx) = old_hover {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = it.visual_rect();
                        }
                    }
                    if let Some(idx) = new_hover {
                        if let Some(it) = self.desktop.items.get(idx) {
                            dirty = if dirty.w > 0 { dirty.union(&it.visual_rect()) } else { it.visual_rect() };
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
        self.desktop.hovered = None;

        match self.drag.take() {
            Some(Drag::DesktopSelect { .. }) => {
                if let Some(rb) = self.desktop.rubberband.take() {
                    for strip in self.border_strips(rb) {
                        self.paint_desktop_rect(strip)?;
                    }
                }
                return Ok(());
            }
            Some(Drag::TaskBar { task, moved, was_active, .. }) => {
                if !moved && was_active {
                    if let Some(&f) = self.tasks.get(task) {
                        self.minimize(f)?;
                    }
                }
                self.paint_bar()?;
                return Ok(());
            }
            Some(Drag::Desktop { item, moved, ref others, start, origin }) => {
                if let Some(d) = self.drag_icon.take() {
                    d.close(&self.conn)?;
                }
                if !moved {
                    return Ok(());
                }
                // Let go over a file manager window: into the folder it shows.
                let folder =
                    self.frame_at(rx, ry).and_then(|f| self.windows.get(&f).map(|m| m.client)).and_then(|c| self.window_folder(c).map(|d| (c, d)));
                if let Some((client, dir)) = folder {
                    let mut idxs: Vec<usize> = others.iter().map(|&(o, _, _)| o).collect();
                    idxs.push(item);
                    let paths: Vec<std::path::PathBuf> = idxs
                        .iter()
                        .filter_map(|&i| self.desktop.items.get(i))
                        .filter_map(|it| match &it.kind {
                            desktop::ItemKind::File(p) | desktop::ItemKind::Folder(p) => Some(p.clone()),
                            desktop::ItemKind::Shortcut { .. } => Some(desktop::desktop_dir().join(&it.id)),
                            _ => None,
                        })
                        .filter(|p| !dir.starts_with(p) && p.parent() != Some(dir.as_path()))
                        .collect();
                    if !paths.is_empty() {
                        // mv: across drives too; -n: never over something there.
                        let _ = std::process::Command::new("mv").arg("-n").arg("--").args(&paths).arg(&dir).status();
                        let msg = ClientMessageEvent::new(32, client, self.atoms.ferro_refresh, [0; 5]);
                        self.conn.send_event(false, client, EventMask::NO_EVENT, msg)?;
                    }
                    self.desktop.clear_selection();
                    self.desktop.refresh(self.screen_w, self.screen_h);
                    self.paint_full_desktop()?;
                    return Ok(());
                }

                // Check if released over Recycle Bin or Folder: what's under
                // the icons being dragged, not those icons themselves.
                let mut moving: Vec<usize> = others.iter().map(|&(o, _, _)| o).collect();
                moving.push(item);
                let hit_target = self.desktop.hit_except(rx, ry, &moving);
                if let Some(target) = hit_target {
                    let is_moving_item = target == item || others.iter().any(|&(o, _, _)| o == target);
                    if !is_moving_item {
                        if let Some(target_item) = self.desktop.items.get(target).cloned() {
                            match target_item.kind {
                                desktop::ItemKind::RecycleBin => {
                                    let mut to_del = vec![item];
                                    for &(o, _, _) in others {
                                        to_del.push(o);
                                    }
                                    to_del.sort_unstable_by(|a, b| b.cmp(a));
                                    to_del.dedup();
                                    for idx in to_del {
                                        self.desktop.delete_item(idx);
                                    }
                                    self.desktop.clear_selection();
                                    self.desktop.refresh(self.screen_w, self.screen_h);
                                    self.paint_full_desktop()?;
                                    self.keep_bin_current()?;
                                    return Ok(());
                                }
                                desktop::ItemKind::Folder(ref dest_dir) => {
                                    let mut to_move = vec![item];
                                    for &(o, _, _) in others {
                                        to_move.push(o);
                                    }
                                    to_move.sort_unstable_by(|a, b| b.cmp(a));
                                    to_move.dedup();
                                    for idx in to_move {
                                        if let Some(it) = self.desktop.items.get(idx) {
                                            match &it.kind {
                                                desktop::ItemKind::File(p) | desktop::ItemKind::Folder(p) => {
                                                    if !dest_dir.starts_with(p) && dest_dir != p {
                                                        let target_file = dest_dir.join(p.file_name().unwrap_or_default());
                                                        let _ = std::fs::rename(p, target_file);
                                                    }
                                                }
                                                desktop::ItemKind::Shortcut { .. } => {
                                                    let desk_file = desktop::desktop_dir().join(&it.id);
                                                    let _ = std::fs::rename(&desk_file, dest_dir.join(&it.id));
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    self.desktop.clear_selection();
                                    self.desktop.refresh(self.screen_w, self.screen_h);
                                    self.paint_full_desktop()?;
                                    return Ok(());
                                }
                                _ => {}
                            }
                        }
                    }
                }

                let (dx, dy) = (rx - start.0, ry - start.1);
                let new_x = origin.0 + dx;
                let new_y = origin.1 + dy;
                self.desktop.move_item(item, new_x, new_y, self.screen_w, self.screen_h);
                for &(other, ox, oy) in others {
                    self.desktop.move_item(other, new_x + ox, new_y + oy, self.screen_w, self.screen_h);
                }
                self.desktop.finish_move_group(item, others, self.screen_w, self.screen_h);
                self.paint_full_desktop()?;
                return Ok(());
            }
            Some(Drag::Move { frame: f, .. }) => {
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
            Some(Drag::Resize { frame: f, .. }) => {
                if let Some(m) = self.windows.get(&f) {
                    if !m.is_dialog && !m.maximized && !m.fullscreen {
                        self.window_store.set(&m.class, m.x, m.y, m.w, m.h);
                    }
                }
                return Ok(());
            }
            None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod fit_tests {
    use super::fit_rect;

    #[test]
    fn windows_fit_above_the_taskbar() {
        // A 1200x900 window on a 1366x768 screen (728 above the taskbar).
        assert_eq!(fit_rect(86, 2, 1206, 906, 1366, 728), (86, 0, 1206, 728));
        // Pushed back in from the right and bottom before being shrunk.
        assert_eq!(fit_rect(900, 600, 600, 400, 1366, 728), (766, 328, 600, 400));
        // Small windows stay put.
        assert_eq!(fit_rect(32, 32, 400, 300, 1366, 728), (32, 32, 400, 300));
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
        assert_eq!(shortcut(0, KEY_PRINT), Some(Shortcut::Snip));
        assert_eq!(shortcut(SUPER | SHIFT, KEY_S), Some(Shortcut::Snip));
        assert_eq!(shortcut(SUPER, KEY_E), Some(Shortcut::Files));
        assert_eq!(shortcut(SUPER | CAPS_LOCK, KEY_D), Some(Shortcut::ShowDesktop));
        // Escape or F4 by themselves belong to the program.
        assert_eq!(shortcut(CONTROL | ALT, KEY_END), Some(Shortcut::Panic));
        assert_eq!(shortcut(CONTROL, KEY_END), None);
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
