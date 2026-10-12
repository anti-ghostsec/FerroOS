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

mod apps;
mod bar;
mod frame;

use bar::{BarPart, Item, Task};
use ferro_gfx::{color, Surface};
use frame::{Look, Part};
use std::collections::HashMap;
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
    net_active_window: Atom,
    net_supported: Atom,
    net_client_list: Atom,
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
            net_active_window: get("_NET_ACTIVE_WINDOW")?,
            net_supported: get("_NET_SUPPORTED")?,
            net_client_list: get("_NET_CLIENT_LIST")?,
        })
    }
}

/// One program window and the frame around it.
struct Managed {
    client: Window,
    frame: Window,
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
    /// Unmaps we caused ourselves (reparenting, minimizing), not the program closing its window.
    ignore_unmaps: u32,
}

/// What the mouse is in the middle of doing.
enum Drag {
    Move { frame: Window, dx: i32, dy: i32 },
    Resize { frame: Window, sides: (bool, bool, bool, bool), start: (i32, i32), origin: (i32, i32, i32, i32) },
    Button { frame: Window, part: Part },
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
    /// The Start menu, while it's open.
    menu: Option<Menu>,
}

/// One line of the open Start menu and what choosing it does.
enum Choice {
    Run(apps::App),
    ShutDown,
    Restart,
    Separator,
}

struct Menu {
    window: Window,
    choices: Vec<Choice>,
    hover: Option<usize>,
}

impl Menu {
    fn items(&self) -> Vec<Item<'_>> {
        self.choices
            .iter()
            .map(|c| match c {
                Choice::Run(app) => Item::Entry(&app.name),
                Choice::ShutDown => Item::Entry("Shut Down"),
                Choice::Restart => Item::Entry("Restart"),
                Choice::Separator => Item::Separator,
            })
            .collect()
    }
}

/// Where installed programs describe themselves.
const APP_DIRS: [&str; 2] = ["/usr/local/share/applications", "/usr/share/applications"];

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

/// Starts a program by its command line, detached from the window manager.
fn launch(command: &str) {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", command]).stdin(std::process::Stdio::null());
    if let Err(e) = cmd.spawn() {
        eprintln!("ferro-wm: {command}: {e}");
    }
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
    // Only one window manager can ask for this: it fails if another runs.
    let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(mask).background_pixel(color::DESKTOP))?
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
    let mut wm = Wm {
        root,
        screen_w,
        screen_h,
        bar: bar_win,
        tasks: Vec::new(),
        bar_stale: true,
        menu: None,
        depth: screen.root_depth,
        visual: screen.root_visual,
        gc,
        atoms,
        windows: HashMap::new(),
        order: Vec::new(),
        active: None,
        drag: None,
        conn,
    };
    wm.announce()?;
    // Windows that were there before us.
    let existing = wm.conn.query_tree(root)?.reply()?.children;
    for win in existing {
        let attrs = wm.conn.get_window_attributes(win)?.reply()?;
        if attrs.map_state == MapState::VIEWABLE && !attrs.override_redirect {
            wm.manage(win, true)?;
        }
    }
    wm.conn.flush()?;
    loop {
        let event = wm.conn.wait_for_event()?;
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
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let px = s.pixels();
        let rows_per = (48 * 1024 / (w as usize * 4)).max(1) as i32;
        let mut row = 0;
        while row < h {
            let n = rows_per.min(h - row);
            let mut data = Vec::with_capacity((w * n * 4) as usize);
            for j in 0..n {
                let start = ((y + row + j) * stride + x) as usize;
                for p in &px[start..start + w as usize] {
                    data.extend_from_slice(&p.to_le_bytes());
                }
            }
            self.conn.put_image(ImageFormat::Z_PIXMAP, win, self.gc, w as u16, n as u16, x as i16, (y + row) as i16, 0, self.depth, &data)?;
            row += n;
        }
        Ok(())
    }

    fn paint_bar(&self) -> Res<()> {
        let tasks: Vec<Task> =
            self.tasks.iter().filter_map(|f| self.windows.get(f)).map(|m| Task { title: &m.title, active: self.active == Some(m.frame) }).collect();
        let mut s = Surface::new(self.screen_w as usize, bar::HEIGHT as usize);
        bar::draw(&mut s, self.screen_w, &tasks, self.menu.is_some(), &clock_text());
        self.blit(self.bar, &s, self.screen_w, (0, 0, self.screen_w, bar::HEIGHT))
    }

    fn paint_menu(&self) -> Res<()> {
        let Some(menu) = &self.menu else { return Ok(()) };
        let items = menu.items();
        let h = bar::menu_height(&items);
        let mut s = Surface::new(bar::MENU_W as usize, h as usize);
        bar::draw_menu(&mut s, &items, menu.hover, "FerroOS");
        self.blit(menu.window, &s, bar::MENU_W, (0, 0, bar::MENU_W, h))
    }

    /// Opens the Start menu above the Start button: every installed
    /// program, then Restart and Shut Down.
    fn open_menu(&mut self) -> Res<()> {
        let programs = apps::installed(&APP_DIRS);
        let mut choices: Vec<Choice> = programs.into_iter().take(bar::MAX_ITEMS).map(Choice::Run).collect();
        if !choices.is_empty() {
            choices.push(Choice::Separator);
        }
        choices.extend([Choice::Restart, Choice::ShutDown]);
        let window = self.conn.generate_id()?;
        let menu = Menu { window, choices, hover: None };
        let h = bar::menu_height(&menu.items());
        self.conn.create_window(
            self.depth,
            window,
            self.root,
            0,
            (self.work_h() - h).max(0) as i16,
            bar::MENU_W as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new().override_redirect(1).background_pixel(color::FACE).event_mask(EventMask::EXPOSURE),
        )?;
        self.conn.map_window(window)?;
        self.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        // Every click and movement comes to the menu while it's open, so a
        // click anywhere else closes it.
        let mask = EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
        self.conn.grab_pointer(false, window, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?;
        self.menu = Some(menu);
        self.bar_stale = true;
        Ok(())
    }

    fn close_menu(&mut self) -> Res<()> {
        if let Some(menu) = self.menu.take() {
            self.conn.ungrab_pointer(CURRENT_TIME)?;
            self.conn.destroy_window(menu.window)?;
            self.bar_stale = true;
        }
        Ok(())
    }

    /// A click while the Start menu is open: on an entry, do it; anywhere
    /// else, just close the menu.
    fn menu_click(&mut self, x: i32, y: i32) -> Res<()> {
        let Some(menu) = &self.menu else { return Ok(()) };
        let chosen = bar::menu_hit(&menu.items(), x, y);
        let command = chosen.and_then(|i| match &menu.choices[i] {
            // Programs that need a terminal get one.
            Choice::Run(app) if app.terminal => Some(format!("x-terminal-emulator -e {}", app.exec)),
            Choice::Run(app) => Some(app.exec.clone()),
            Choice::ShutDown => Some("systemctl poweroff".to_owned()),
            Choice::Restart => Some("systemctl reboot".to_owned()),
            Choice::Separator => None,
        });
        self.close_menu()?;
        if let Some(command) = command {
            launch(&command);
        }
        Ok(())
    }

    fn bar_click(&mut self, x: i32, y: i32) -> Res<()> {
        match bar::hit(self.screen_w, self.tasks.len(), x, y) {
            BarPart::Start => self.open_menu()?,
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
        let supported = [a.net_wm_name, a.net_wm_state, a.net_wm_state_fullscreen, a.net_active_window, a.net_client_list, a.net_wm_window_type];
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
        // Taskbars and desktops (ours, later) draw themselves: no frame.
        let kinds = self.atom_list(client, self.atoms.net_wm_window_type);
        if kinds.contains(&self.atoms.net_wm_window_type_dock) || kinds.contains(&self.atoms.net_wm_window_type_desktop) {
            self.conn.map_window(client)?;
            return Ok(());
        }
        let geo = self.conn.get_geometry(client)?.reply()?;
        let (w, h) = frame::outer(i32::from(geo.width), i32::from(geo.height));
        // Programs that don't say where they want to be are stepped down
        // the screen, like Windows does.
        let n = self.windows.len() as i32 % 8;
        let (mut x, mut y) = (i32::from(geo.x), i32::from(geo.y));
        if x <= 0 && y <= 0 {
            (x, y) = (32 + n * 24, 32 + n * 24);
        }
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
        self.conn.configure_window(client, &ConfigureWindowAux::new().border_width(0))?;
        self.conn.reparent_window(client, frame_win, frame::LEFT as i16, frame::TOP as i16)?;
        self.conn.change_window_attributes(
            client,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        // A click inside the program brings its window forward; the click
        // then goes on to the program.
        self.conn.grab_button(false, client, EventMask::BUTTON_PRESS, GrabMode::SYNC, GrabMode::ASYNC, NONE, NONE, ButtonIndex::ANY, ModMask::ANY)?;
        self.conn.map_window(client)?;
        self.conn.map_window(frame_win)?;
        self.conn.change_property32(PropMode::REPLACE, client, self.atoms.wm_state, self.atoms.wm_state, &[1, NONE])?;

        let title = self.title_of(client);
        let managed = Managed {
            client,
            frame: frame_win,
            x,
            y,
            w,
            h,
            title,
            restore: None,
            maximized: false,
            fullscreen: false,
            minimized: false,
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
        frame::draw(&mut s, w, h, &Look { title: &m.title, active: self.active == Some(frame_win), maximized: m.maximized, pressed });
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
                    } else if e.event == f {
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
                } else if self.menu.as_ref().is_some_and(|m| m.window == e.window) {
                    self.paint_menu()?;
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
                }
            }
            Event::ClientMessage(e) => self.client_message(e)?,
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
            return self.menu_click(i32::from(e.event_x), i32::from(e.event_y));
        }
        if e.event == self.bar {
            return self.bar_click(i32::from(e.event_x), i32::from(e.event_y));
        }
        // A click inside a program: forward it once its window is in front.
        if let Some(f) = self.by_client(e.event) {
            self.activate(f)?;
            self.conn.allow_events(Allow::REPLAY_POINTER, e.time)?;
            return Ok(());
        }
        let f = e.event;
        let Some(m) = self.windows.get(&f) else { return Ok(()) };
        let (w, h, x, y, maximized) = (m.w, m.h, m.x, m.y, m.maximized);
        self.activate(f)?;
        if e.detail != 1 {
            return Ok(());
        }
        let (px, py) = (i32::from(e.event_x), i32::from(e.event_y));
        match frame::hit(w, h, px, py, !maximized) {
            part @ (Part::Close | Part::Maximize | Part::Minimize) => {
                self.paint(f, Some(part))?;
                self.drag = Some(Drag::Button { frame: f, part });
            }
            Part::Title if !maximized => self.drag = Some(Drag::Move { frame: f, dx: i32::from(e.root_x) - x, dy: i32::from(e.root_y) - y }),
            Part::Resize(l, t, r, b) => {
                self.drag =
                    Some(Drag::Resize { frame: f, sides: (l, t, r, b), start: (i32::from(e.root_x), i32::from(e.root_y)), origin: (x, y, w, h) });
            }
            _ => {}
        }
        Ok(())
    }

    fn motion(&mut self, e: MotionNotifyEvent) -> Res<()> {
        if let Some(menu) = &mut self.menu {
            let hover = bar::menu_hit(&menu.items(), i32::from(e.event_x), i32::from(e.event_y));
            if hover != menu.hover {
                menu.hover = hover;
                self.paint_menu()?;
            }
            return Ok(());
        }
        let (rx, ry) = (i32::from(e.root_x), i32::from(e.root_y));
        match self.drag {
            Some(Drag::Move { frame: f, dx, dy }) => {
                let Some(m) = self.windows.get(&f) else { return Ok(()) };
                let (w, h) = (m.w, m.h);
                // Never so far off that the title bar can't be reached.
                let x = (rx - dx).clamp(48 - w, self.screen_w - 48);
                let y = (ry - dy).clamp(0, self.work_h() - frame::TOP);
                self.place(f, x, y, w, h)?;
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
            _ => {}
        }
        Ok(())
    }

    fn button_release(&mut self, e: ButtonReleaseEvent) -> Res<()> {
        if let Some(Drag::Button { frame: f, part }) = self.drag.take() {
            let Some(m) = self.windows.get(&f) else { return Ok(()) };
            self.paint(f, None)?;
            // Only if the mouse is still on the button it went down on.
            if frame::hit(m.w, m.h, i32::from(e.event_x), i32::from(e.event_y), !m.maximized) == part {
                match part {
                    Part::Close => self.close(f)?,
                    Part::Maximize => self.toggle_maximize(f)?,
                    Part::Minimize => self.minimize(f)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_like_windows_95() {
        assert_eq!(clock_format(0, 5), "12:05 AM");
        assert_eq!(clock_format(9, 30), "9:30 AM");
        assert_eq!(clock_format(12, 0), "12:00 PM");
        assert_eq!(clock_format(22, 14), "10:14 PM");
    }
}
