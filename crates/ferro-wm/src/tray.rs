//! Other programs' icons in the tray (Sunshine, Steam, Discord...): the
//! freedesktop system tray, which puts each icon, a little window of its
//! program's, in the taskbar (XEmbed).
//!
//! The desktop owns the `_NET_SYSTEM_TRAY_S<screen>` selection and says so
//! to every program (a MANAGER message). A program then asks for its icon
//! window to be docked; it goes into the taskbar, in the box left of the
//! clock, and gets its own clicks and menus from there. It leaves when its
//! program unmaps or destroys it, or takes it back. With more than
//! `bar::APPS_FOLDED`, the rest wait behind a < that shows them all.

use crate::{bar, Res, Wm};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

/// SYSTEM_TRAY_REQUEST_DOCK, of `_NET_SYSTEM_TRAY_OPCODE`.
const REQUEST_DOCK: u32 = 0;
/// XEMBED_EMBEDDED_NOTIFY, of `_XEMBED`.
const EMBEDDED_NOTIFY: u32 = 0;
/// In `_XEMBED_INFO`'s flags: the program wants its icon shown.
const XEMBED_MAPPED: u32 = 1;

/// The tray's own names (atoms).
pub struct TrayAtoms {
    selection: Atom,
    pub opcode: Atom,
    orientation: Atom,
    visual: Atom,
    manager: Atom,
    xembed: Atom,
    pub xembed_info: Atom,
}

/// One program's icon in the tray.
pub struct TrayIcon {
    pub win: Window,
    /// Hidden by its program (`_XEMBED_INFO`): kept, but takes no room.
    pub shown: bool,
    /// Whether it's on the screen now (we mapped it, or it came that way).
    mapped: bool,
    /// Unmaps we made ourselves, not the program: not a sign it's gone.
    ignore_unmaps: u32,
}

pub struct Tray {
    /// The window holding the selection; NONE when another tray has it.
    pub win: Window,
    pub atoms: TrayAtoms,
    pub icons: Vec<TrayIcon>,
    /// All the icons shown, not just the first few (the < was clicked).
    pub open: bool,
}

impl Tray {
    /// Becomes the screen's system tray, if no other program is.
    pub fn new(conn: &RustConnection, root: Window, screen_num: usize, visual: Visualid) -> Res<Self> {
        let get = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let atoms = TrayAtoms {
            selection: get(&format!("_NET_SYSTEM_TRAY_S{screen_num}"))?,
            opcode: get("_NET_SYSTEM_TRAY_OPCODE")?,
            orientation: get("_NET_SYSTEM_TRAY_ORIENTATION")?,
            visual: get("_NET_SYSTEM_TRAY_VISUAL")?,
            manager: get("MANAGER")?,
            xembed: get("_XEMBED")?,
            xembed_info: get("_XEMBED_INFO")?,
        };
        let mut tray = Tray { win: NONE, atoms, icons: Vec::new(), open: false };
        if conn.get_selection_owner(tray.atoms.selection)?.reply()?.owner != NONE {
            eprintln!("ferro-wm: another program is the system tray");
            return Ok(tray);
        }
        let win = conn.generate_id()?;
        conn.create_window(0, win, root, -1, -1, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
        // Icons lie flat in a row, and draw with the screen's own colours.
        conn.change_property32(PropMode::REPLACE, win, tray.atoms.orientation, AtomEnum::CARDINAL, &[0])?;
        conn.change_property32(PropMode::REPLACE, win, tray.atoms.visual, AtomEnum::VISUALID, &[visual])?;
        conn.set_selection_owner(win, tray.atoms.selection, CURRENT_TIME)?;
        if conn.get_selection_owner(tray.atoms.selection)?.reply()?.owner != win {
            conn.destroy_window(win)?;
            return Ok(tray);
        }
        tray.win = win;
        // Programs already running hear there's a tray now, and dock.
        let msg = ClientMessageEvent::new(32, root, tray.atoms.manager, [CURRENT_TIME, tray.atoms.selection, win, 0, 0]);
        conn.send_event(false, root, EventMask::STRUCTURE_NOTIFY, msg)?;
        Ok(tray)
    }

    pub fn has(&self, win: Window) -> bool {
        self.icons.iter().any(|i| i.win == win)
    }

    /// The icons that take room in the taskbar, folded or all.
    pub fn apps(&self) -> bar::Apps {
        bar::Apps::new(self.icons.iter().filter(|i| i.shown).count(), self.open)
    }
}

impl Wm {
    /// Whether `win` is a tray icon (it has `_XEMBED_INFO`): one left on the
    /// screen by a desktop that reloaded, to be docked again, not framed.
    pub(crate) fn is_tray_icon(&self, win: Window) -> bool {
        self.tray.win != NONE
            && self
                .conn
                .get_property(false, win, self.tray.atoms.xembed_info, AtomEnum::ANY, 0, 2)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.format == 32)
    }

    /// A program's message to the tray: dock this icon.
    pub(crate) fn tray_message(&mut self, e: &ClientMessageEvent) -> Res<()> {
        let d = e.data.as_data32();
        if d[1] == REQUEST_DOCK && d[2] != NONE {
            self.dock(d[2])?;
        }
        Ok(())
    }

    /// Whether the program wants `win` shown (`_XEMBED_INFO`; shown if it
    /// doesn't say).
    fn wants_shown(&self, win: Window) -> bool {
        let reply = self.conn.get_property(false, win, self.tray.atoms.xembed_info, AtomEnum::ANY, 0, 2).ok().and_then(|c| c.reply().ok());
        match reply.as_ref().and_then(|r| r.value32()).map(|v| v.collect::<Vec<u32>>()) {
            Some(v) if v.len() >= 2 => v[1] & XEMBED_MAPPED != 0,
            _ => true,
        }
    }

    pub(crate) fn dock(&mut self, win: Window) -> Res<()> {
        if self.tray.has(win) || win == self.bar {
            return Ok(());
        }
        // Kept if the desktop ends (put back on the screen, not destroyed
        // with the taskbar), so its program never loses it.
        self.conn.change_save_set(SetMode::INSERT, win)?;
        let was_mapped = self.conn.get_window_attributes(win)?.reply()?.map_state != MapState::UNMAPPED;
        self.conn.reparent_window(win, self.bar, 0, 0)?;
        // Its own unmaps, and what it says about being shown, from now on
        // (after the move, whose unmap isn't one).
        self.conn
            .change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE))?;
        let msg = ClientMessageEvent::new(32, win, self.tray.atoms.xembed, [CURRENT_TIME, EMBEDDED_NOTIFY, 0, self.bar, 0]);
        self.conn.send_event(false, win, EventMask::NO_EVENT, msg)?;
        let shown = self.wants_shown(win);
        self.tray.icons.push(TrayIcon { win, shown, mapped: was_mapped, ignore_unmaps: 0 });
        self.tray_layout()
    }

    /// The icons in their places, the shown ones mapped; the taskbar redrawn
    /// round them.
    pub(crate) fn tray_layout(&mut self) -> Res<()> {
        let apps = self.tray.apps();
        let size = bar::APP_ICON as u32;
        let mut k = 0;
        for icon in self.tray.icons.iter_mut() {
            // Hidden by its program, or folded behind the <.
            let on_bar = icon.shown && k < apps.shown();
            if icon.shown {
                k += 1;
            }
            if on_bar {
                let (x, y) = bar::app_icon_at(self.screen_w, apps, k - 1);
                self.conn.configure_window(icon.win, &ConfigureWindowAux::new().x(x).y(y).width(size).height(size).stack_mode(StackMode::ABOVE))?;
                self.conn.map_window(icon.win)?;
                icon.mapped = true;
            } else if icon.mapped {
                icon.ignore_unmaps += 1;
                icon.mapped = false;
                self.conn.unmap_window(icon.win)?;
            }
        }
        // Back to few enough: nothing left to unfold.
        if !apps.folds() {
            self.tray.open = false;
        }
        self.bar_stale = true;
        Ok(())
    }

    /// The < or > beside the programs' icons: all of them, or the first few.
    pub(crate) fn tray_more(&mut self) -> Res<()> {
        self.tray.open = !self.tray.open;
        self.tray_layout()
    }

    /// Its program unmapped, destroyed or took back an icon: it leaves.
    /// One merely unmapped is handed back, out of the taskbar.
    pub(crate) fn undock(&mut self, win: Window, give_back: bool) -> Res<()> {
        let Some(at) = self.tray.icons.iter().position(|i| i.win == win) else {
            return Ok(());
        };
        self.tray.icons.remove(at);
        if give_back {
            let _ = self.conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT));
            let _ = self.conn.reparent_window(win, self.root, 0, 0);
            let _ = self.conn.change_save_set(SetMode::DELETE, win);
        }
        self.tray_layout()
    }

    /// An unmap of a tray icon: ours (hiding it), or its program's (it goes).
    pub(crate) fn tray_unmapped(&mut self, win: Window) -> Res<()> {
        if let Some(icon) = self.tray.icons.iter_mut().find(|i| i.win == win) {
            if icon.ignore_unmaps > 0 {
                icon.ignore_unmaps -= 1;
                return Ok(());
            }
        }
        self.undock(win, true)
    }

    /// The program changed `_XEMBED_INFO`: shows or hides its icon.
    pub(crate) fn tray_info_changed(&mut self, win: Window) -> Res<()> {
        let shown = self.wants_shown(win);
        if let Some(icon) = self.tray.icons.iter_mut().find(|i| i.win == win) {
            if icon.shown != shown {
                icon.shown = shown;
                return self.tray_layout();
            }
        }
        Ok(())
    }
}
