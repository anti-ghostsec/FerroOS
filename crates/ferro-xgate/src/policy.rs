//! What a program may ask the display for, request by request.
//!
//! A program draws, reads its own pictures and talks to the window manager
//! as before. What it may not do unasked is see the screen: any request
//! that would copy what's on screen, or what other programs drew, to
//! somewhere it can read the pixels back. Those wait for the person's yes
//! (`Action::Ask`). What it may never do: press keys or move the mouse as
//! if it were the person (XTEST), record or snoop on what they type (RECORD,
//! raw key events, key events from other programs' windows, grabbing every
//! key), read what other programs keep on their windows beyond what kind
//! of window each is, or push other programs' windows around. Those are
//! turned into requests that do nothing.

use crate::wire::Req;
use std::collections::{HashMap, HashSet};

/// The core requests looked at.
mod core {
    pub const CREATE_WINDOW: u8 = 1;
    pub const CHANGE_WINDOW_ATTRIBUTES: u8 = 2;
    pub const DESTROY_WINDOW: u8 = 4;
    pub const SEND_EVENT: u8 = 25;
    pub const CHANGE_PROPERTY: u8 = 18;
    pub const GET_PROPERTY: u8 = 20;
    pub const CONVERT_SELECTION: u8 = 24;
    pub const DELETE_PROPERTY: u8 = 19;
    pub const GRAB_KEY: u8 = 33;
    pub const GRAB_SERVER: u8 = 36;
    pub const QUERY_KEYMAP: u8 = 44;
    pub const CREATE_PIXMAP: u8 = 53;
    pub const FREE_PIXMAP: u8 = 54;
    pub const CREATE_GC: u8 = 55;
    pub const CHANGE_GC: u8 = 56;
    pub const COPY_AREA: u8 = 62;
    pub const COPY_PLANE: u8 = 63;
    pub const GET_IMAGE: u8 = 73;
    pub const QUERY_EXTENSION: u8 = 98;
    pub const LIST_EXTENSIONS: u8 = 99;
    pub const CHANGE_HOSTS: u8 = 109;
    pub const SET_ACCESS_CONTROL: u8 = 111;
    pub const KILL_CLIENT: u8 = 113;
    pub const ROTATE_PROPERTIES: u8 = 114;
    pub const NO_OPERATION: u8 = 127;
}
pub use core::NO_OPERATION;

/// Extensions programs never learn about: faking input, recording it,
/// reading the screen's memory directly, and making cookies for the display
/// itself (SECURITY), which would let a program past the gate.
pub const HIDDEN: [&[u8]; 5] = [b"XTEST", b"RECORD", b"XFree86-DGA", b"XEVIE", b"SECURITY"];

/// Each extension's request number on this display (QueryExtension's
/// major opcode), for the ones looked at.
#[derive(Clone, Debug, Default)]
pub struct Opcodes {
    pub shm: Option<u8>,
    pub composite: Option<u8>,
    pub render: Option<u8>,
    pub xinput: Option<u8>,
    /// The hidden ones' (HIDDEN): nothing of theirs gets through. The
    /// display takes every program for root, the gate being root, so even
    /// the framebuffer itself (DGA) would be had for the asking.
    pub hidden: Vec<u8>,
    pub dri2: Option<u8>,
    pub dri3: Option<u8>,
    pub glx: Option<u8>,
    /// BIG-REQUESTS: its one request turns the longer header on (wire.rs).
    pub big_requests: Option<u8>,
}

/// What other programs' windows say about themselves that any program may
/// read: what kind of window, whose, how it wants to be placed, its icon;
/// what drag and drop, the tray and the settings daemon publish for others.
/// Not what's in them. (Titles are `Atoms::private`.)
pub const OPEN: [&str; 49] = [
    "WM_CLASS",
    "WM_HINTS",
    "WM_NORMAL_HINTS",
    "WM_TRANSIENT_FOR",
    "WM_CLIENT_MACHINE",
    "WM_ICON_SIZE",
    "WM_PROTOCOLS",
    "WM_STATE",
    "WM_CLIENT_LEADER",
    "WM_WINDOW_ROLE",
    "WM_COLORMAP_WINDOWS",
    "WM_LOCALE_NAME",
    "_NET_WM_PID",
    "_NET_WM_WINDOW_TYPE",
    "_NET_WM_STATE",
    "_NET_WM_ICON",
    "_NET_WM_DESKTOP",
    "_NET_FRAME_EXTENTS",
    "_NET_WM_ALLOWED_ACTIONS",
    "_NET_WM_STRUT",
    "_NET_WM_STRUT_PARTIAL",
    "_NET_WM_USER_TIME",
    "_NET_WM_USER_TIME_WINDOW",
    "_NET_WM_WINDOW_OPACITY",
    "_NET_WM_BYPASS_COMPOSITOR",
    "_NET_WM_OPAQUE_REGION",
    "_NET_WM_SYNC_REQUEST_COUNTER",
    "_NET_WM_ICON_GEOMETRY",
    "_NET_WM_HANDLED_ICONS",
    "_NET_WM_FULLSCREEN_MONITORS",
    "_NET_SUPPORTING_WM_CHECK",
    "_NET_STARTUP_ID",
    "_MOTIF_WM_HINTS",
    "_GTK_FRAME_EXTENTS",
    "_GTK_THEME_VARIANT",
    "_GTK_APPLICATION_ID",
    "_KDE_NET_WM_FRAME_STRUT",
    "XdndAware",
    "XdndTypeList",
    "XdndProxy",
    "XdndActionList",
    "XdndActionDescription",
    "_XEMBED_INFO",
    "_XSETTINGS_SETTINGS",
    "_NET_SYSTEM_TRAY_ORIENTATION",
    "_NET_SYSTEM_TRAY_VISUAL",
    "_NET_SYSTEM_TRAY_COLORS",
    "_NET_SYSTEM_TRAY_PADDING",
    "_NET_SYSTEM_TRAY_ICON_SIZE",
];

/// Kinds of property that are numbers and names, never text or pictures:
/// readable on any window whatever the property is called. (The built-in
/// ones: ATOM, CARDINAL, DRAWABLE, INTEGER, PIXMAP, POINT, RECTANGLE,
/// VISUALID, WINDOW, WM_HINTS, WM_SIZE_HINTS.)
pub const PLAIN_TYPES: [u32; 11] = [4, 6, 17, 19, 20, 21, 22, 32, 33, 35, 41];
/// And those without a built-in number.
pub const PLAIN_TYPE_NAMES: [&str; 4] = ["ATOM_PAIR", "WM_STATE", "_MOTIF_WM_HINTS", "_XEMBED_INFO"];

/// What only the window manager and the gate say about the desktop, which
/// the gate goes by: which picture is the background (anyone may read it
/// back), which window is in use (it may paste), who may see the screen.
pub const GUARDED: [&str; 4] = ["_FERRO_SCREEN", "_XROOTPMAP_ID", "ESETROOT_PMAP_ID", "_NET_ACTIVE_WINDOW"];

/// The names (atoms) the rules go by.
#[derive(Clone, Debug, Default)]
pub struct Atoms {
    /// Who may see the screen now, for the taskbar's light; only the gate writes it.
    pub screen_in_use: u32,
    /// The desktop's properties (GUARDED) programs may read but not write.
    pub guarded: Vec<u32>,
    /// Messages any program may send to another's window: the tray, XEMBED,
    /// FerroOS's own between its programs (refresh, the clipboard history).
    pub messages: Vec<u32>,
    /// Drag and drop's, which carry the sender's window first.
    pub xdnd: Vec<u32>,
    /// What other programs' windows are called (titles, which often name
    /// the file that's open) and the command that started them: not for
    /// others to read, unless they may see the screen.
    pub private: Vec<u32>,
    /// What the File Manager says each of its windows shows: only the File
    /// Manager may read another's (dragging files from window to window).
    pub dirs: Vec<u32>,
    /// What any program may read on another's window (OPEN).
    pub open: Vec<u32>,
    /// The kinds of property any program may read there (PLAIN_TYPES).
    pub plain: Vec<u32>,
    /// What's copied (CLIPBOARD) and what's selected (PRIMARY, SECONDARY):
    /// only for the program being used, the one with the keyboard.
    pub selections: Vec<u32>,
    /// Asking what kinds of thing were copied (TARGETS, TIMESTAMP), not
    /// what: any program may. GTK 4 asks the moment anything is copied and
    /// remembers the answer, while the person is still in the program they
    /// copied from; told "nothing", its Paste stayed greyed out.
    pub selection_info: Vec<u32>,
}

/// What the gate knows about the display around one program.
pub trait World {
    /// The id is one of this program's windows, pixmaps... (any of its connections').
    fn mine(&self, id: u32) -> bool;
    fn root(&self) -> u32;
    /// The desktop's background pixmap: anyone may draw with it (see-through terminals).
    fn background(&self) -> u32;
    /// The program may use this shared memory segment (it's its own).
    fn may_use_shm(&self, shmid: u32) -> bool;
    /// The keyboard is on one of its windows: it's the program being used.
    fn focused(&self) -> bool;
    /// One more key grabbed on a window not its own (a global shortcut),
    /// counted for the whole program, however many connections it has;
    /// false once it has too many.
    fn key_grab(&self) -> bool;
}

/// What a request that would see the screen wants to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// GetImage (core, or MIT-SHM's into shared memory): the pixels of a
    /// window, the screen or another program's pixmap.
    Image { shm: bool, drawable: u32, format: u8, w: u16, h: u16, plane_mask: u32 },
    /// Copied into something of its own to read back later.
    Copy,
    /// Another window's contents as a pixmap (Composite).
    WindowPixmap,
    /// Another program's graphics buffer (DRI3).
    Buffer,
}

/// What's done instead when a request may not go through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deny {
    /// It becomes NoOperation: nothing happens, nothing comes back.
    NoOp,
    /// The id at this offset becomes 0: the display answers with an error,
    /// for requests whose program waits for an answer.
    Clear(usize),
    /// A black picture is handed back in its place.
    Black,
    /// Pasting: the program is told there's nothing to paste (a
    /// SelectionNotify without a property, as when nothing was copied).
    NothingCopied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// QueryExtension for this name: hidden ones are said not to exist.
    Extension(Vec<u8>),
    /// ListExtensions: the hidden ones left out.
    Extensions,
    /// QueryKeymap: which keys are down, only while the program has the keyboard.
    Keymap,
    /// XInput's QueryDeviceState: the same, of one keyboard.
    DeviceState,
    /// GetProperty of another program's window's title: none there, unless
    /// the program may see the screen now.
    Private,
    /// GetProperty of anything else on another program's window: none
    /// there, unless it's a plain kind (`Atoms::plain`). What's being
    /// pasted into that window passes through such a property.
    Others,
    /// GetProperty of what was selected last (the cut buffers), by a
    /// program not being used: none there.
    Absent,
    /// BIG-REQUESTS' Enable: from its answer on, requests may be long.
    BigRequests,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Pass,
    /// The person decides.
    Ask(Read, Deny),
    Refuse(Deny),
    /// Goes through; its answer is looked at. (A GetProperty watched goes
    /// through without its "and delete it".)
    Watch(Expect),
    /// Goes through without the key events it asked for on others' windows
    /// (XInput 2, `strip_keys`).
    StripKeys,
    /// The same of the core protocol's (`strip_events`).
    StripEvents,
    /// GetProperty that would delete what it read: goes through, without deleting.
    NoDelete,
}

/// What a RENDER picture is a picture of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    Drawable(u32),
    /// A colour or a gradient: nothing of anyone's.
    Fill,
    /// Its id was given twice, one of which the display refused: which, it
    /// didn't say here.
    Unknown,
}

/// What the rules remember about one connection.
#[derive(Debug, Default)]
pub struct Client {
    /// The connection's own ids are `base | n`, n within `mask`: only
    /// those can it give to what it makes.
    base: u32,
    mask: u32,
    /// Its own pixmaps: what it may read back freely.
    pixmaps: HashSet<u32>,
    /// Its own windows. A pixmap can't be made with the id of one, though
    /// a program may ask; and drawing into one is drawing on the screen,
    /// not somewhere to read back from.
    windows: HashSet<u32>,
    /// Its RENDER pictures and what each is a picture of.
    pictures: HashMap<u32, Shows>,
    /// Its pixmaps freed while a picture still shows them: the display
    /// keeps them till the picture goes, so they're still its own (the
    /// mouse pointer library frees the pixmap before making the cursor).
    freed: HashSet<u32>,
    /// It's the File Manager (see `Atoms::dirs`).
    files: bool,
}

/// More global shortcuts than this is grabbing the keyboard key by key.
pub const MOST_KEY_GRABS: usize = 64;

/// Key and button presses, and which keys are down (KeymapState), in the
/// core protocol's event mask.
const SNOOPING: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x4000;
/// ChangeWindowAttributes' event mask, and the values before it.
const EVENT_MASK: u32 = 0x800;

/// Where ChangeWindowAttributes' event mask is, if it sets one.
fn event_mask_at(r: &Req) -> Option<usize> {
    let mask = r.u32(8)?;
    (mask & EVENT_MASK != 0).then(|| 12 + 4 * (mask & (EVENT_MASK - 1)).count_ones() as usize)
}

/// A key typed with (letters, digits, punctuation, space, Enter, the
/// keypad), by its number on a PC keyboard.
fn typing(key: u32) -> bool {
    matches!(key, 10..=36 | 38..=49 | 51..=61 | 65 | 79..=91)
}

/// Grabbing `key` with these modifiers takes it from whatever the person
/// is typing into: a typing key on its own (or with Shift), or with any
/// modifier at all (`any`: the protocol's AnyModifier). With Ctrl, Alt or
/// the Windows key it's a shortcut.
fn takes_typing(key: u32, modifiers: u32, any: u32) -> bool {
    const SHORTCUT: u32 = 0x4 | 0x8 | 0x40;
    typing(key) && (modifiers & any != 0 || modifiers & SHORTCUT == 0)
}

impl Client {
    /// For a connection whose own ids are `base | n`, n within `mask`.
    pub fn new(files: bool, base: u32, mask: u32) -> Self {
        Client { files, base, mask, ..Default::default() }
    }

    /// An id the connection itself may give out.
    fn own(&self, id: u32) -> bool {
        id != 0 && id & !self.mask == self.base
    }

    /// It asked for a pixmap called `id`. Only an id of its own can become
    /// one (naming the screen doesn't make it the program's), and not one
    /// that's a window of its.
    fn made_pixmap(&mut self, id: u32) {
        if self.own(id) && !self.windows.contains(&id) {
            self.pixmaps.insert(id);
            self.freed.remove(&id);
        }
    }

    fn made_picture(&mut self, id: u32, shows: Shows) {
        if self.own(id) {
            self.pictures.entry(id).and_modify(|s| *s = Shows::Unknown).or_insert(shows);
        }
    }

    /// Pixels it may read back: its own pixmaps, and the desktop's background.
    fn readable(&self, w: &dyn World, id: u32) -> bool {
        self.pixmaps.contains(&id) || (id != 0 && id == w.background())
    }

    /// What's drawn into `id` can be read back later, by this program or
    /// another: anything but the screen and its own windows. (Whether an
    /// id from another connection is a window can't be known for the
    /// moment the request is carried out.)
    fn kept(&self, w: &dyn World, id: u32) -> bool {
        id != w.root() && !self.windows.contains(&id)
    }

    /// A picture not made on this connection is unknown: not its to read.
    fn picture(&self, pic: u32) -> Shows {
        self.pictures.get(&pic).copied().unwrap_or(Shows::Unknown)
    }

    fn picture_readable(&self, w: &dyn World, pic: u32) -> bool {
        match self.picture(pic) {
            Shows::Drawable(d) => self.readable(w, d),
            Shows::Fill => true,
            Shows::Unknown => false,
        }
    }

    /// RENDER drawing from `srcs` (a source, a mask; 0: none) into `dst`:
    /// seeing the screen if a source is a window or another program's
    /// pixmap and the result lands where it can be read.
    fn render_reads(&self, w: &dyn World, srcs: &[u32], dst: u32) -> bool {
        let kept = match self.picture(dst) {
            Shows::Drawable(d) => self.kept(w, d),
            _ => true,
        };
        kept && srcs.iter().any(|&p| p != 0 && !self.picture_readable(w, p))
    }

    pub fn classify(&mut self, w: &dyn World, ops: &Opcodes, atoms: &Atoms, r: &Req) -> Action {
        let op = r.opcode();
        let get = |at| r.u32(at).unwrap_or(0);
        if ops.hidden.contains(&op) {
            return Action::Refuse(Deny::NoOp);
        }
        match op {
            core::CREATE_PIXMAP => {
                self.made_pixmap(get(4));
                Action::Pass
            }
            core::FREE_PIXMAP => {
                let id = get(4);
                if self.pictures.values().any(|&d| d == Shows::Drawable(id)) {
                    self.freed.insert(id);
                } else {
                    self.pixmaps.remove(&id);
                }
                Action::Pass
            }
            // An id used again, for a window: no longer a pixmap.
            core::CREATE_WINDOW => {
                let id = get(4);
                self.pixmaps.remove(&id);
                self.freed.remove(&id);
                if self.own(id) {
                    self.windows.insert(id);
                }
                Action::Pass
            }
            core::DESTROY_WINDOW => {
                self.windows.remove(&get(4));
                Action::Pass
            }
            // Key presses and clicks in other programs' windows, and on the
            // desktop (where what's typed into a window that didn't ask for
            // keys this way ends up): not for others to listen to.
            core::CHANGE_WINDOW_ATTRIBUTES if !w.mine(get(4)) && event_mask_at(r).is_some_and(|at| get(at) & SNOOPING != 0) => Action::StripEvents,
            core::GET_IMAGE => {
                let d = get(4);
                if self.readable(w, d) {
                    Action::Pass
                } else {
                    let read = Read::Image {
                        shm: false,
                        drawable: d,
                        format: r.minor(),
                        w: r.u16(12).unwrap_or(0),
                        h: r.u16(14).unwrap_or(0),
                        plane_mask: get(16),
                    };
                    Action::Ask(read, Deny::Black)
                }
            }
            core::COPY_AREA | core::COPY_PLANE => {
                let (src, dst) = (get(4), get(8));
                if !self.readable(w, src) && src != dst && self.kept(w, dst) {
                    Action::Ask(Read::Copy, Deny::NoOp)
                } else {
                    Action::Pass
                }
            }
            core::CREATE_GC | core::CHANGE_GC => {
                // A tile: a picture to paint with. Another program's (or a
                // window, which can't be one) painted into a pixmap of its own is a copy.
                let (mask_at, values_at) = if op == core::CREATE_GC { (12, 16) } else { (8, 12) };
                let mask = get(mask_at);
                const TILE: u32 = 0x400;
                if mask & TILE != 0 {
                    let tile = get(values_at + 4 * (mask & (TILE - 1)).count_ones() as usize);
                    if !self.readable(w, tile) {
                        return Action::Ask(Read::Copy, Deny::NoOp);
                    }
                }
                Action::Pass
            }
            core::SEND_EVENT => self.send_event(w, atoms, r),
            core::GRAB_KEY => {
                let (window, modifiers, key) = (get(4), u32::from(r.u16(8).unwrap_or(0)), u32::from(r.u8(10).unwrap_or(0)));
                // Every key (AnyKey), the keys typed with, or one by one: a keylogger.
                if w.mine(window) || (key != 0 && !takes_typing(key, modifiers, 0x8000) && w.key_grab()) {
                    Action::Pass
                } else {
                    Action::Refuse(Deny::NoOp)
                }
            }
            core::QUERY_KEYMAP => Action::Watch(Expect::Keymap),
            core::GET_PROPERTY => self.get_property(w, atoms, r),
            // Pasting while another program is being used: reading the
            // clipboard behind the person's back. (Drag and drop's is
            // XdndSelection, which goes through.)
            core::CONVERT_SELECTION if atoms.selections.contains(&get(8)) && !atoms.selection_info.contains(&get(12)) && !w.focused() => {
                Action::Refuse(Deny::NothingCopied)
            }
            core::QUERY_EXTENSION => {
                let n = usize::from(r.u16(4).unwrap_or(0));
                let at = r.at(8);
                Action::Watch(Expect::Extension(r.bytes.get(at..at + n).unwrap_or_default().to_vec()))
            }
            core::LIST_EXTENSIONS => Action::Watch(Expect::Extensions),
            // What the gate itself goes by: a program that could write
            // these would make any picture "the background", or itself
            // "the window in use".
            core::CHANGE_PROPERTY | core::DELETE_PROPERTY if get(4) == w.root() && atoms.guarded.contains(&get(8)) => Action::Refuse(Deny::NoOp),
            core::ROTATE_PROPERTIES if get(4) == w.root() => {
                let n = usize::from(r.u16(8).unwrap_or(0));
                if (0..n).any(|i| atoms.guarded.contains(&get(12 + 4 * i))) {
                    Action::Refuse(Deny::NoOp)
                } else {
                    Action::Pass
                }
            }
            // Freezing every other program, the question included.
            core::GRAB_SERVER => Action::Refuse(Deny::NoOp),
            // Letting other computers onto the display.
            core::CHANGE_HOSTS | core::SET_ACCESS_CONTROL => Action::Refuse(Deny::NoOp),
            core::KILL_CLIENT if !w.mine(get(4)) => Action::Refuse(Deny::NoOp),
            _ => self.extension(w, ops, r),
        }
    }

    fn get_property(&self, w: &dyn World, atoms: &Atoms, r: &Req) -> Action {
        let (window, property, delete) = (r.u32(4).unwrap_or(0), r.u32(8).unwrap_or(0), r.minor() != 0);
        if window == w.root() {
            // CUT_BUFFER0 to 7: where terminals also leave what was selected.
            if (9..=16).contains(&property) && !w.focused() {
                Action::Watch(Expect::Absent)
            } else if delete && atoms.guarded.contains(&property) {
                Action::NoDelete
            } else {
                Action::Pass
            }
        } else if w.mine(window) {
            // Its own: what's pasted or dropped arrives here.
            Action::Pass
        } else if atoms.dirs.contains(&property) {
            if self.files {
                Action::Pass
            } else {
                Action::Watch(Expect::Private)
            }
        } else if atoms.private.contains(&property) {
            Action::Watch(Expect::Private)
        } else if !atoms.open.contains(&property) {
            Action::Watch(Expect::Others)
        } else if delete {
            Action::NoDelete
        } else {
            Action::Pass
        }
    }

    fn send_event(&self, w: &dyn World, atoms: &Atoms, r: &Req) -> Action {
        const SELECTION_NOTIFY: u8 = 31;
        const CLIENT_MESSAGE: u8 = 33;
        let get = |at| r.u32(at).unwrap_or(0);
        let dest = get(4);
        if w.mine(dest) {
            return Action::Pass;
        }
        // The event itself starts at 12.
        let allowed = match r.u8(12).unwrap_or(0) & 0x7f {
            // Handing over what was copied, to whoever asked to paste it.
            SELECTION_NOTIFY => true,
            CLIENT_MESSAGE => {
                let (window, kind, first) = (get(16), get(20), get(24));
                // To the desktop: asking the window manager (EWMH), which decides.
                dest == w.root() || w.mine(window) || atoms.messages.contains(&kind) || (atoms.xdnd.contains(&kind) && w.mine(first))
            }
            // Pretend key presses, clicks and the like for other programs.
            _ => false,
        };
        if allowed {
            Action::Pass
        } else {
            Action::Refuse(Deny::NoOp)
        }
    }

    fn extension(&mut self, w: &dyn World, ops: &Opcodes, r: &Req) -> Action {
        let (op, minor) = (Some(r.opcode()), r.minor());
        let get = |at| r.u32(at).unwrap_or(0);
        if op == ops.shm {
            match minor {
                // Attach: only memory the program could open itself (the display runs as root).
                1 if !w.may_use_shm(get(8)) => return Action::Refuse(Deny::NoOp),
                4 if !self.readable(w, get(4)) => {
                    let read = Read::Image {
                        shm: true,
                        drawable: get(4),
                        format: r.u8(20).unwrap_or(2),
                        w: r.u16(12).unwrap_or(0),
                        h: r.u16(14).unwrap_or(0),
                        plane_mask: get(16),
                    };
                    return Action::Ask(read, Deny::Black);
                }
                5 => self.made_pixmap(get(4)),
                _ => {}
            }
        } else if op == ops.composite {
            match minor {
                // Becoming a compositor: every window's pixels. Not for programs.
                // One that asks to draw the whole screen itself (the desktop's
                // windows, by hand) is told no with an error, as when there's
                // a compositor already: it then gives up, where one told
                // nothing would go on to paint a black screen over the desktop.
                1 | 2 if get(4) == w.root() && r.u8(8) == Some(1) => return Action::Refuse(Deny::Clear(4)),
                1 | 2 if !w.mine(get(4)) => return Action::Refuse(Deny::NoOp),
                6 => return Action::Ask(Read::WindowPixmap, Deny::NoOp),
                _ => {}
            }
        } else if op == ops.render {
            match minor {
                4 => self.made_picture(get(4), Shows::Drawable(get(8))),
                7 => {
                    if let Some(Shows::Drawable(d)) = self.pictures.remove(&get(4)) {
                        if self.freed.contains(&d) && !self.pictures.values().any(|&o| o == Shows::Drawable(d)) {
                            self.freed.remove(&d);
                            self.pixmaps.remove(&d);
                        }
                    }
                }
                8 if self.render_reads(w, &[get(8), get(12)], get(16)) => return Action::Ask(Read::Copy, Deny::NoOp),
                10..=13 | 23..=25 if self.render_reads(w, &[get(8)], get(12)) => return Action::Ask(Read::Copy, Deny::NoOp),
                // A cursor made of the screen, read back through XFIXES.
                27 if !self.picture_readable(w, get(8)) => return Action::Ask(Read::Copy, Deny::NoOp),
                // A plain colour, a gradient.
                33..=36 => self.made_picture(get(4), Shows::Fill),
                _ => {}
            }
        } else if op == ops.xinput {
            match minor {
                // XInput 1: key events from another's window, grabbing its
                // keys, or sending it pretend ones.
                6 | 15 | 31 if !w.mine(get(4)) => return Action::Refuse(Deny::NoOp),
                30 => return Action::Watch(Expect::DeviceState),
                46 if !w.mine(get(4)) => return Action::StripKeys,
                // XInput 2 grabbing keys on another's window (it answers: an error instead).
                54 if !w.mine(get(8)) && !self.may_grab(w, r) => return Action::Refuse(Deny::Clear(8)),
                _ => {}
            }
        } else if op == ops.dri3 {
            match minor {
                2 | 7 => self.made_pixmap(get(4)),
                3 | 8 if !self.readable(w, get(4)) => return Action::Ask(Read::Buffer, Deny::Clear(4)),
                _ => {}
            }
        } else if op == ops.dri2 {
            // DRI2 hands out a window's buffers: only its own.
            if (3..=12).contains(&minor) && !w.mine(get(4)) {
                return Action::Refuse(if matches!(minor, 3 | 4 | 12) { Deny::NoOp } else { Deny::Clear(4) });
            }
        } else if op == ops.glx {
            // A GL picture of a pixmap (CreateGLXPixmap, CreatePixmap).
            if matches!(minor, 13 | 22) && !self.readable(w, get(12)) {
                return Action::Ask(Read::Copy, Deny::NoOp);
            }
        } else if op == ops.big_requests && op.is_some() {
            return Action::Watch(Expect::BigRequests);
        }
        Action::Pass
    }

    /// XInput 2's passive grab (XIPassiveGrabDevice) on a window not the
    /// program's own. What's grabbed is at 26: a button (0), a key (1),
    /// the pointer entering (2), the keyboard arriving (3), a touch (4).
    /// The key is at 16 (0: every key); the modifiers follow the event
    /// mask, which starts at 32 and is `mask_len` (at 24) units long.
    fn may_grab(&self, w: &dyn World, r: &Req) -> bool {
        const ANY_MODIFIER: u32 = 1 << 31;
        match r.u8(26) {
            Some(1) => {
                let key = r.u32(16).unwrap_or(0);
                let (count, at) = (usize::from(r.u16(22).unwrap_or(0)), 32 + 4 * usize::from(r.u16(24).unwrap_or(0)));
                let typing = (0..count).any(|i| takes_typing(key, r.u32(at + 4 * i).unwrap_or(ANY_MODIFIER), ANY_MODIFIER));
                key != 0 && !typing && w.key_grab()
            }
            // The whole keyboard (or pointer) whenever it comes to that window.
            Some(2 | 3) => false,
            _ => true,
        }
    }
}

/// XInput 2's event selection on a window not the program's own, without
/// key presses and releases and the raw (everywhere) key events. Masks are
/// (device u16, length u16 in 4-byte units, bits) from offset 12; event
/// `n`'s bit is bit `n % 8` of byte `n / 8`, whatever the byte order.
pub fn strip_keys(r: &mut [u8], order: crate::wire::Order, big: bool) {
    const KEYS: [usize; 4] = [2, 3, 13, 14];
    let shift = if big { 4 } else { 0 };
    if r.len() < 12 + shift {
        return;
    }
    let count = usize::from(order.u16(r, 8 + shift));
    let mut at = 12 + shift;
    for _ in 0..count {
        if r.len() < at + 4 {
            return;
        }
        let len = 4 * usize::from(order.u16(r, at + 2));
        let bits = at + 4;
        for k in KEYS {
            let pos = bits + k / 8;
            if k / 8 < len && pos < r.len() {
                r[pos] &= !(1 << (k % 8));
            }
        }
        at = bits + len;
    }
}

/// ChangeWindowAttributes on a window not the program's own, without key
/// presses, clicks and which keys are down in the events it asks for.
pub fn strip_events(r: &mut [u8], order: crate::wire::Order, big: bool) {
    let req = Req { order, bytes: r, big };
    let Some(at) = event_mask_at(&req).map(|at| req.at(at)) else { return };
    if r.len() >= at + 4 {
        let v = order.u32(r, at) & !SNOOPING;
        order.put_u32(r, at, v);
    }
}

/// XInput's QueryDeviceState answer without which keys are down. After
/// the 32-byte header come `p[8]` classes, each (class, length, ...); a
/// keyboard's (class 0) is (0, length, number of keys, pad, 32 bytes of keys).
pub fn without_keys_down(p: &[u8]) -> Vec<u8> {
    let mut r = p.to_vec();
    let mut at = 32;
    for _ in 0..p.get(8).copied().unwrap_or(0) {
        let Some(&[class, len]) = p.get(at..at + 2) else { break };
        let end = (at + usize::from(len)).min(r.len());
        if class == 0 && at + 4 <= end {
            r[at + 4..end].fill(0);
        }
        if len == 0 {
            break;
        }
        at += usize::from(len);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Order;

    const LE: Order = Order(true);
    const ROOT: u32 = 0x100;
    const BG: u32 = 0x200;

    thread_local!(static FOCUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    // Keys the program has grabbed on others' windows, all its connections together.
    thread_local!(static GRABS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });

    /// Ids 0x0400_0000 and up are this program's.
    struct Fake;
    impl World for Fake {
        fn mine(&self, id: u32) -> bool {
            id >= 0x0400_0000
        }
        fn root(&self) -> u32 {
            ROOT
        }
        fn background(&self) -> u32 {
            BG
        }
        fn may_use_shm(&self, shmid: u32) -> bool {
            shmid == 7
        }
        fn focused(&self) -> bool {
            FOCUSED.with(|f| f.get())
        }
        fn key_grab(&self) -> bool {
            GRABS.with(|g| {
                g.set(g.get() + 1);
                g.get() <= MOST_KEY_GRABS
            })
        }
    }

    fn req(op: u8, minor: u8, fields: &[(usize, u32)], len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        b[0] = op;
        b[1] = minor;
        LE.put_u16(&mut b, 2, (len / 4) as u16);
        for &(at, v) in fields {
            LE.put_u32(&mut b, at, v);
        }
        b
    }

    fn classify(c: &mut Client, b: &[u8]) -> Action {
        let ops = Opcodes {
            shm: Some(130),
            composite: Some(142),
            render: Some(139),
            xinput: Some(131),
            hidden: vec![132, 146],
            dri3: Some(149),
            big_requests: Some(133),
            ..Default::default()
        };
        let atoms = Atoms {
            screen_in_use: 900,
            guarded: vec![900, 910, 911, 912],
            open: vec![67, 920],
            plain: vec![4, 6, 33],
            messages: vec![901],
            xdnd: vec![902],
            private: vec![39, 903],
            dirs: vec![904],
            selections: vec![905, 1],
            selection_info: vec![907, 908],
        };
        c.classify(&Fake, &ops, &atoms, &Req { order: LE, bytes: b, big: false })
    }

    /// A connection whose ids are 0x0400_0000 and up (2 million of them).
    fn client() -> Client {
        Client::new(false, 0x0400_0000, 0x001f_ffff)
    }

    /// The same, with its window MY_WIN made.
    fn client_with_window() -> Client {
        let mut c = client();
        classify(&mut c, &req(core::CREATE_WINDOW, 24, &[(4, MY_WIN), (8, ROOT)], 32));
        c
    }

    const MY_WIN: u32 = 0x0400_0001;
    const MY_PIX: u32 = 0x0400_0002;
    const THEIRS: u32 = 0x0300_0001;

    #[test]
    fn seeing_the_screen_asks() {
        let mut c = client_with_window();
        classify(&mut c, &req(core::CREATE_PIXMAP, 24, &[(4, MY_PIX), (8, ROOT)], 16));
        let get = |d| req(core::GET_IMAGE, 2, &[(4, d), (16, !0)], 20);
        assert!(matches!(classify(&mut c, &get(ROOT)), Action::Ask(Read::Image { shm: false, drawable: ROOT, .. }, Deny::Black)));
        assert!(matches!(classify(&mut c, &get(THEIRS)), Action::Ask(..)));
        // Its own window shows whatever is behind it where it hasn't drawn.
        assert!(matches!(classify(&mut c, &get(MY_WIN)), Action::Ask(..)));
        assert_eq!(classify(&mut c, &get(MY_PIX)), Action::Pass, "its own pixmap is its own");
        assert_eq!(classify(&mut c, &get(BG)), Action::Pass, "the desktop's background");
        // Copying the screen into its own pixmap is the same as reading it...
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, ROOT), (8, MY_PIX)], 28)), Action::Ask(Read::Copy, Deny::NoOp));
        // ...but scrolling its own window, or drawing into one, isn't.
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, MY_WIN), (8, MY_WIN)], 28)), Action::Pass);
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, MY_PIX), (8, MY_WIN)], 28)), Action::Pass);
        classify(&mut c, &req(core::FREE_PIXMAP, 0, &[(4, MY_PIX)], 8));
        assert!(matches!(classify(&mut c, &get(MY_PIX)), Action::Ask(..)), "freed: no longer known as its own");
    }

    #[test]
    fn a_cursor_from_its_own_freed_pixmap_doesnt_ask() {
        // As libXcursor does (xsetroot -cursor_name, every GTK program):
        // pixmap, picture of it, pixmap freed, cursor made of the picture.
        let mut c = client_with_window();
        let pic = 0x0400_0010;
        classify(&mut c, &req(core::CREATE_PIXMAP, 32, &[(4, MY_PIX), (8, ROOT)], 16));
        classify(&mut c, &req(139, 4, &[(4, pic), (8, MY_PIX)], 20));
        classify(&mut c, &req(core::FREE_PIXMAP, 0, &[(4, MY_PIX)], 8));
        assert_eq!(classify(&mut c, &req(139, 27, &[(4, 0x0400_0011), (8, pic)], 16)), Action::Pass);
        // Once the picture's gone too, the id means nothing of its own.
        classify(&mut c, &req(139, 7, &[(4, pic)], 8));
        assert!(matches!(classify(&mut c, &req(core::GET_IMAGE, 2, &[(4, MY_PIX), (16, !0)], 20)), Action::Ask(..)));
        // And a window given a used id isn't a pixmap.
        classify(&mut c, &req(core::CREATE_PIXMAP, 24, &[(4, MY_WIN), (8, ROOT)], 16));
        classify(&mut c, &req(core::FREE_PIXMAP, 0, &[(4, MY_WIN)], 8));
        classify(&mut c, &req(core::CREATE_WINDOW, 24, &[(4, MY_WIN), (8, ROOT)], 32));
        assert!(matches!(classify(&mut c, &req(core::GET_IMAGE, 2, &[(4, MY_WIN), (16, !0)], 20)), Action::Ask(..)));
    }

    #[test]
    fn shared_memory_and_compositing() {
        let mut c = client_with_window();
        let shm_get = req(130, 4, &[(4, ROOT), (16, !0)], 32);
        assert!(matches!(classify(&mut c, &shm_get), Action::Ask(Read::Image { shm: true, .. }, Deny::Black)));
        assert_eq!(classify(&mut c, &req(130, 1, &[(4, 1), (8, 9)], 16)), Action::Refuse(Deny::NoOp), "another's memory");
        assert_eq!(classify(&mut c, &req(130, 1, &[(4, 1), (8, 7)], 16)), Action::Pass);
        assert_eq!(classify(&mut c, &req(142, 2, &[(4, ROOT)], 12)), Action::Refuse(Deny::NoOp), "redirecting the whole screen");
        let mut by_hand = req(142, 2, &[(4, ROOT)], 12);
        by_hand[8] = 1;
        assert_eq!(classify(&mut c, &by_hand), Action::Refuse(Deny::Clear(4)), "a compositor not started by the gate: an error, so it gives up");
        assert_eq!(classify(&mut c, &req(142, 1, &[(4, MY_WIN)], 12)), Action::Pass);
        assert_eq!(classify(&mut c, &req(142, 6, &[(4, THEIRS), (8, MY_PIX)], 12)), Action::Ask(Read::WindowPixmap, Deny::NoOp));
    }

    #[test]
    fn render_from_a_window_into_a_pixmap_asks() {
        let mut c = client_with_window();
        classify(&mut c, &req(core::CREATE_PIXMAP, 24, &[(4, MY_PIX), (8, ROOT)], 16));
        let (root_pic, win_pic, pix_pic) = (0x0400_0010, 0x0400_0011, 0x0400_0012);
        for (p, d) in [(root_pic, ROOT), (win_pic, MY_WIN), (pix_pic, MY_PIX)] {
            classify(&mut c, &req(139, 4, &[(4, p), (8, d)], 20));
        }
        let composite = |src, dst| req(139, 8, &[(8, src), (12, 0), (16, dst)], 36);
        assert_eq!(classify(&mut c, &composite(root_pic, pix_pic)), Action::Ask(Read::Copy, Deny::NoOp));
        assert_eq!(classify(&mut c, &composite(pix_pic, win_pic)), Action::Pass, "drawing its picture on screen");
        assert_eq!(classify(&mut c, &composite(win_pic, win_pic)), Action::Pass, "within its own window");
    }

    #[test]
    fn no_pretend_input_or_snooping() {
        let mut c = client_with_window();
        assert_eq!(classify(&mut c, &req(132, 2, &[], 36)), Action::Refuse(Deny::NoOp), "XTEST FakeInput");
        assert_eq!(classify(&mut c, &req(146, 5, &[(4, 3)], 8)), Action::Refuse(Deny::NoOp), "RECORD");
        assert_eq!(classify(&mut c, &req(core::GRAB_KEY, 1, &[(4, ROOT)], 16)), Action::Refuse(Deny::NoOp), "AnyKey on the desktop");
        GRABS.with(|g| g.set(0));
        // Print Screen, on its own: a global shortcut.
        let mut one_key = req(core::GRAB_KEY, 1, &[(4, ROOT)], 16);
        one_key[10] = 107;
        assert_eq!(classify(&mut c, &one_key), Action::Pass, "a global shortcut");
        for _ in 0..MOST_KEY_GRABS {
            classify(&mut c, &one_key);
        }
        assert_eq!(classify(&mut c, &one_key), Action::Refuse(Deny::NoOp), "every key, one at a time");
        GRABS.with(|g| g.set(0));
        let mut ctrl_a = one_key.clone();
        LE.put_u16(&mut ctrl_a, 8, 0x4);
        for _ in 0..MOST_KEY_GRABS {
            assert_eq!(classify(&mut c, &ctrl_a), Action::Pass);
        }
        assert_eq!(classify(&mut client(), &ctrl_a), Action::Refuse(Deny::NoOp), "nor from another of its connections");
        assert_eq!(classify(&mut c, &req(131, 46, &[(4, ROOT)], 20)), Action::StripKeys);
        assert_eq!(classify(&mut c, &req(131, 46, &[(4, MY_WIN)], 20)), Action::Pass);
        assert_eq!(classify(&mut c, &req(core::QUERY_KEYMAP, 0, &[], 4)), Action::Watch(Expect::Keymap));
        assert_eq!(classify(&mut c, &req(core::GRAB_SERVER, 0, &[], 4)), Action::Refuse(Deny::NoOp), "the question must still show");
    }

    #[test]
    fn messages_to_others() {
        let mut c = client_with_window();
        let send = |dest, kind: u8, window, msg, first| {
            let mut b = req(core::SEND_EVENT, 0, &[(4, dest), (16, window), (20, msg), (24, first)], 44);
            b[12] = kind;
            b
        };
        // Asking the window manager (EWMH) is fine; it decides...
        assert_eq!(classify(&mut c, &send(ROOT, 33, MY_WIN, 1, 0)), Action::Pass);
        assert_eq!(classify(&mut c, &send(ROOT, 33, THEIRS, 1, 0)), Action::Pass);
        // ...messages straight to another program's window, not.
        assert_eq!(classify(&mut c, &send(THEIRS, 33, THEIRS, 1, 0)), Action::Refuse(Deny::NoOp));
        assert_eq!(classify(&mut c, &send(THEIRS, 33, THEIRS, 901, 0)), Action::Pass, "FerroOS's own messages");
        assert_eq!(classify(&mut c, &send(THEIRS, 33, THEIRS, 902, MY_WIN)), Action::Pass, "drag and drop, from its window");
        assert_eq!(classify(&mut c, &send(THEIRS, 33, THEIRS, 902, THEIRS)), Action::Refuse(Deny::NoOp));
        assert_eq!(classify(&mut c, &send(THEIRS, 31, 0, 0, 0)), Action::Pass, "a paste's answer");
        assert_eq!(classify(&mut c, &send(THEIRS, 2, 0, 0, 0)), Action::Refuse(Deny::NoOp), "a pretend key press");
        assert_eq!(classify(&mut c, &send(ROOT, 2, 0, 0, 0)), Action::Refuse(Deny::NoOp), "or to the desktop");
        assert_eq!(classify(&mut c, &send(MY_WIN, 2, 0, 0, 0)), Action::Pass, "to itself");
        // The taskbar's screen light is the gate's alone.
        assert_eq!(classify(&mut c, &req(core::CHANGE_PROPERTY, 0, &[(4, ROOT), (8, 900)], 24)), Action::Refuse(Deny::NoOp));
        assert_eq!(classify(&mut c, &req(core::KILL_CLIENT, 0, &[(4, THEIRS)], 8)), Action::Refuse(Deny::NoOp));
    }

    #[test]
    fn other_windows_titles_are_private() {
        let mut c = client_with_window();
        let title = |win, prop| req(core::GET_PROPERTY, 0, &[(4, win), (8, prop), (12, 0), (16, 0), (20, 1024)], 24);
        assert_eq!(classify(&mut c, &title(THEIRS, 39)), Action::Watch(Expect::Private), "WM_NAME");
        assert_eq!(classify(&mut c, &title(THEIRS, 903)), Action::Watch(Expect::Private), "_NET_WM_NAME");
        assert_eq!(classify(&mut c, &title(MY_WIN, 39)), Action::Pass, "its own");
        assert_eq!(classify(&mut c, &title(ROOT, 903)), Action::Pass, "the desktop's");
        assert_eq!(classify(&mut c, &title(THEIRS, 67)), Action::Pass, "WM_CLASS, what kind of program");
        let mut and_delete = title(THEIRS, 67);
        and_delete[1] = 1;
        assert_eq!(classify(&mut c, &and_delete), Action::NoDelete, "read, but not taken from the window");
    }

    #[test]
    fn pasting_is_for_the_program_being_used() {
        let mut c = client_with_window();
        let paste = |sel| req(core::CONVERT_SELECTION, 0, &[(4, MY_WIN), (8, sel), (12, 31), (16, 77), (20, 0)], 24);
        FOCUSED.with(|f| f.set(false));
        assert_eq!(classify(&mut c, &paste(905)), Action::Refuse(Deny::NothingCopied), "CLIPBOARD, from the background");
        assert_eq!(classify(&mut c, &paste(1)), Action::Refuse(Deny::NothingCopied), "PRIMARY");
        assert_eq!(classify(&mut c, &paste(906)), Action::Pass, "drag and drop's, and the rest");
        let kinds = |target| req(core::CONVERT_SELECTION, 0, &[(4, MY_WIN), (8, 905), (12, target), (16, 77), (20, 0)], 24);
        assert_eq!(classify(&mut c, &kinds(907)), Action::Pass, "what kinds of thing were copied (TARGETS), from the background");
        assert_eq!(classify(&mut c, &kinds(908)), Action::Pass, "when (TIMESTAMP)");
        FOCUSED.with(|f| f.set(true));
        assert_eq!(classify(&mut c, &paste(905)), Action::Pass, "Ctrl+V in the window being used");
    }

    #[test]
    fn only_the_file_manager_reads_its_folders() {
        let dir = req(core::GET_PROPERTY, 0, &[(4, THEIRS), (8, 904), (12, 0), (16, 0), (20, 1024)], 24);
        assert_eq!(classify(&mut client(), &dir), Action::Watch(Expect::Private));
        assert_eq!(classify(&mut Client::new(true, 0x0400_0000, 0x001f_ffff), &dir), Action::Pass);
    }

    #[test]
    fn strips_only_key_events() {
        // One mask, one unit: KeyPress (2), ButtonPress (4), RawKeyPress (13), RawMotion (17).
        let mut b = req(131, 46, &[(4, ROOT)], 20);
        LE.put_u16(&mut b, 8, 1);
        LE.put_u16(&mut b, 14, 1);
        LE.put_u32(&mut b, 16, 1 << 2 | 1 << 4 | 1 << 13 | 1 << 17);
        strip_keys(&mut b, LE, false);
        assert_eq!(LE.u32(&b, 16), 1 << 4 | 1 << 17, "clicks and raw motion (games) stay");
    }

    #[test]
    fn naming_something_doesnt_make_it_its_pixmap() {
        let get = |d| req(core::GET_IMAGE, 2, &[(4, d), (16, !0)], 20);
        let pixmap = |id| req(core::CREATE_PIXMAP, 24, &[(4, id), (8, ROOT)], 16);
        // "A pixmap called <the screen>": the display refuses; the gate mustn't believe it.
        let mut c = client();
        classify(&mut c, &pixmap(ROOT));
        assert!(matches!(classify(&mut c, &get(ROOT)), Action::Ask(..)), "the screen");
        classify(&mut c, &pixmap(THEIRS));
        assert!(matches!(classify(&mut c, &get(THEIRS)), Action::Ask(..)), "another program's window");
        // Its own window, then a pixmap of the same id (refused too): still a window.
        let mut c = client_with_window();
        classify(&mut c, &pixmap(MY_WIN));
        assert!(matches!(classify(&mut c, &get(MY_WIN)), Action::Ask(..)), "its window shows the screen behind it");
        // Through MIT-SHM (CreatePixmap) and DRI3 (PixmapFromBuffer, PixmapFromBuffers) the same.
        for (op, minor) in [(130, 5), (149, 2), (149, 7)] {
            let mut c = client_with_window();
            classify(&mut c, &req(op, minor, &[(4, ROOT), (8, ROOT)], 32));
            classify(&mut c, &req(op, minor, &[(4, MY_WIN), (8, ROOT)], 32));
            assert!(matches!(classify(&mut c, &get(ROOT)), Action::Ask(..)));
            assert!(matches!(classify(&mut c, &get(MY_WIN)), Action::Ask(..)));
            classify(&mut c, &req(op, minor, &[(4, MY_PIX), (8, ROOT)], 32));
            assert_eq!(classify(&mut c, &get(MY_PIX)), Action::Pass, "a real one of its own");
        }
        // Once the window's gone its id is free for a pixmap.
        let mut c = client_with_window();
        classify(&mut c, &req(core::DESTROY_WINDOW, 0, &[(4, MY_WIN)], 8));
        classify(&mut c, &pixmap(MY_WIN));
        assert_eq!(classify(&mut c, &get(MY_WIN)), Action::Pass);
    }

    #[test]
    fn copying_the_screen_anywhere_it_could_be_read_asks() {
        // Into a pixmap made on another connection (its own or a friend's),
        // which that one then reads as its own.
        let mut c = client_with_window();
        let elsewhere = 0x0460_0002;
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, ROOT), (8, elsewhere)], 28)), Action::Ask(Read::Copy, Deny::NoOp));
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, MY_WIN), (8, elsewhere)], 28)), Action::Ask(Read::Copy, Deny::NoOp));
        // Onto the screen itself, or its own window: drawing.
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, elsewhere), (8, ROOT)], 28)), Action::Pass);
        assert_eq!(classify(&mut c, &req(core::COPY_AREA, 0, &[(4, elsewhere), (8, MY_WIN)], 28)), Action::Pass);
    }

    #[test]
    fn pictures_from_another_connection_arent_its_to_read() {
        let mut c = client_with_window();
        classify(&mut c, &req(core::CREATE_PIXMAP, 24, &[(4, MY_PIX), (8, ROOT)], 16));
        let (pix_pic, win_pic, fill) = (0x0400_0012, 0x0400_0013, 0x0400_0014);
        classify(&mut c, &req(139, 4, &[(4, pix_pic), (8, MY_PIX)], 20));
        classify(&mut c, &req(139, 4, &[(4, win_pic), (8, MY_WIN)], 20));
        classify(&mut c, &req(139, 33, &[(4, fill)], 16));
        let composite = |src, mask, dst| req(139, 8, &[(8, src), (12, mask), (16, dst)], 36);
        // A picture of the screen made on a second connection, used here.
        let elsewhere = 0x0460_0010;
        assert_eq!(classify(&mut c, &composite(elsewhere, 0, pix_pic)), Action::Ask(Read::Copy, Deny::NoOp), "an unknown source");
        assert_eq!(classify(&mut c, &composite(pix_pic, elsewhere, pix_pic)), Action::Ask(Read::Copy, Deny::NoOp), "an unknown mask");
        assert_eq!(classify(&mut c, &composite(win_pic, 0, elsewhere)), Action::Ask(Read::Copy, Deny::NoOp), "into an unknown picture");
        assert_eq!(classify(&mut c, &req(139, 27, &[(4, 0x0400_0020), (8, elsewhere)], 16)), Action::Ask(Read::Copy, Deny::NoOp), "a cursor of it");
        // What every toolkit does all day still goes through.
        assert_eq!(classify(&mut c, &composite(fill, 0, pix_pic)), Action::Pass, "a colour into its pixmap");
        assert_eq!(classify(&mut c, &composite(pix_pic, 0, pix_pic)), Action::Pass);
        assert_eq!(classify(&mut c, &composite(elsewhere, 0, win_pic)), Action::Pass, "onto its window");
        // An id claimed twice: one of the two was refused, so it may be the screen's.
        classify(&mut c, &req(139, 4, &[(4, pix_pic), (8, ROOT)], 20));
        assert_eq!(classify(&mut c, &composite(pix_pic, 0, 0x0400_0015)), Action::Ask(Read::Copy, Deny::NoOp));
        classify(&mut c, &req(139, 33, &[(4, win_pic)], 16));
        assert_eq!(classify(&mut c, &composite(win_pic, 0, pix_pic)), Action::Ask(Read::Copy, Deny::NoOp), "not a colour after all");
        // And a picture with an id not its own to give isn't one.
        classify(&mut c, &req(139, 33, &[(4, THEIRS)], 16));
        assert_eq!(classify(&mut c, &composite(THEIRS, 0, pix_pic)), Action::Ask(Read::Copy, Deny::NoOp));
    }

    #[test]
    fn no_listening_to_keys_on_other_windows() {
        let mut c = client_with_window();
        // ChangeWindowAttributes: the event mask (0x800) after a background pixel (0x2).
        let select = |win, events| req(core::CHANGE_WINDOW_ATTRIBUTES, 0, &[(4, win), (8, 0x802), (12, 0), (16, events)], 20);
        const KEYS: u32 = 0x3;
        const PROPERTY_CHANGE: u32 = 0x40_0000;
        const STRUCTURE: u32 = 0x2_0000;
        assert_eq!(classify(&mut c, &select(THEIRS, KEYS | STRUCTURE)), Action::StripEvents);
        assert_eq!(classify(&mut c, &select(ROOT, KEYS)), Action::StripEvents, "the desktop gets what windows don't take");
        assert_eq!(classify(&mut c, &select(THEIRS, 0x4000)), Action::StripEvents, "which keys are down");
        assert_eq!(classify(&mut c, &select(MY_WIN, KEYS)), Action::Pass, "its own");
        assert_eq!(classify(&mut c, &select(ROOT, PROPERTY_CHANGE | STRUCTURE)), Action::Pass, "what toolkits watch the desktop for");
        assert_eq!(classify(&mut c, &select(THEIRS, PROPERTY_CHANGE)), Action::Pass, "a big paste's sender watches the taker's window");
        let mut b = select(THEIRS, KEYS | 0xc | 0x4000 | STRUCTURE | PROPERTY_CHANGE);
        strip_events(&mut b, LE, false);
        assert_eq!((LE.u32(&b, 12), LE.u32(&b, 16)), (0, STRUCTURE | PROPERTY_CHANGE), "only keys and clicks go");
        // XInput 1: pretend key events for another window.
        assert_eq!(classify(&mut c, &req(131, 31, &[(4, THEIRS)], 48)), Action::Refuse(Deny::NoOp));
        assert_eq!(classify(&mut c, &req(131, 30, &[(4, 3)], 8)), Action::Watch(Expect::DeviceState));
    }

    #[test]
    fn other_windows_keep_what_passes_through_them() {
        let mut c = client_with_window();
        let read = |win, prop| req(core::GET_PROPERTY, 0, &[(4, win), (8, prop), (12, 0), (16, 0), (20, 1024)], 24);
        // A property a paste lands in (the taker names it as it likes).
        assert_eq!(classify(&mut c, &read(THEIRS, 950)), Action::Watch(Expect::Others));
        assert_eq!(classify(&mut c, &read(MY_WIN, 950)), Action::Pass, "what's pasted into its own window");
        assert_eq!(classify(&mut c, &read(THEIRS, 920)), Action::Pass, "what windows say about themselves");
        assert_eq!(classify(&mut c, &read(ROOT, 950)), Action::Pass, "the desktop's");
        // What a terminal's selection also leaves on the desktop (CUT_BUFFER0).
        FOCUSED.with(|f| f.set(false));
        assert_eq!(classify(&mut c, &read(ROOT, 9)), Action::Watch(Expect::Absent));
        FOCUSED.with(|f| f.set(true));
        assert_eq!(classify(&mut c, &read(ROOT, 9)), Action::Pass);
        FOCUSED.with(|f| f.set(false));
    }

    #[test]
    fn the_desktops_own_properties_are_read_only() {
        let mut c = client_with_window();
        // _XROOTPMAP_ID (910), ESETROOT_PMAP_ID (911), _NET_ACTIVE_WINDOW (912).
        for atom in [900, 910, 911, 912] {
            assert_eq!(classify(&mut c, &req(core::CHANGE_PROPERTY, 0, &[(4, ROOT), (8, atom)], 28)), Action::Refuse(Deny::NoOp));
            assert_eq!(classify(&mut c, &req(core::DELETE_PROPERTY, 0, &[(4, ROOT), (8, atom)], 12)), Action::Refuse(Deny::NoOp));
            let mut rotate = req(core::ROTATE_PROPERTIES, 0, &[(4, ROOT), (12, 950), (16, atom)], 20);
            LE.put_u16(&mut rotate, 8, 2);
            assert_eq!(classify(&mut c, &rotate), Action::Refuse(Deny::NoOp), "swapped with another");
            let mut take = req(core::GET_PROPERTY, 1, &[(4, ROOT), (8, atom), (20, 1)], 24);
            assert_eq!(classify(&mut c, &take), Action::NoDelete, "read and delete");
            take[1] = 0;
            assert_eq!(classify(&mut c, &take), Action::Pass);
        }
        assert_eq!(classify(&mut c, &req(core::CHANGE_PROPERTY, 0, &[(4, ROOT), (8, 950)], 28)), Action::Pass, "any other");
        assert_eq!(classify(&mut c, &req(core::CHANGE_PROPERTY, 0, &[(4, MY_WIN), (8, 910)], 28)), Action::Pass, "on its own window");
        let mut rotate = req(core::ROTATE_PROPERTIES, 0, &[(4, ROOT), (12, 950), (16, 951)], 20);
        LE.put_u16(&mut rotate, 8, 2);
        assert_eq!(classify(&mut c, &rotate), Action::Pass);
    }

    #[test]
    fn grabbing_the_keys_typed_with() {
        let mut c = client_with_window();
        GRABS.with(|g| g.set(0));
        let grab = |win, key: u8, modifiers: u16| {
            let mut b = req(core::GRAB_KEY, 1, &[(4, win)], 16);
            LE.put_u16(&mut b, 8, modifiers);
            b[10] = key;
            b
        };
        // 38 is A.
        assert_eq!(classify(&mut c, &grab(ROOT, 38, 0)), Action::Refuse(Deny::NoOp), "a letter on its own");
        assert_eq!(classify(&mut c, &grab(ROOT, 38, 0x1)), Action::Refuse(Deny::NoOp), "with Shift: a capital");
        assert_eq!(classify(&mut c, &grab(ROOT, 38, 0x8000)), Action::Refuse(Deny::NoOp), "with any modifier");
        assert_eq!(classify(&mut c, &grab(THEIRS, 65, 0x10)), Action::Refuse(Deny::NoOp), "space, Num Lock on");
        assert_eq!(classify(&mut c, &grab(ROOT, 38, 0x4)), Action::Pass, "Ctrl+A: a shortcut");
        assert_eq!(classify(&mut c, &grab(ROOT, 38, 0x40 | 0x1)), Action::Pass, "Shift+Windows+A");
        assert_eq!(classify(&mut c, &grab(ROOT, 96, 0)), Action::Pass, "F12 on its own");
        assert_eq!(classify(&mut c, &grab(MY_WIN, 38, 0)), Action::Pass, "in its own window, anything");
        assert_eq!(GRABS.with(|g| g.get()), 3, "only those let through count");

        // XInput 2's: (window at 8, key at 16, modifiers' count at 22, mask
        // length at 24, what's grabbed at 26), one mask unit, the modifiers.
        let xi2 = |win, key, kind: u8, modifiers: &[u32]| {
            let mut b = req(131, 54, &[(8, win), (16, key)], 36 + 4 * modifiers.len());
            LE.put_u16(&mut b, 22, modifiers.len() as u16);
            LE.put_u16(&mut b, 24, 1);
            b[26] = kind;
            for (i, m) in modifiers.iter().enumerate() {
                LE.put_u32(&mut b, 36 + 4 * i, *m);
            }
            b
        };
        assert_eq!(classify(&mut c, &xi2(ROOT, 0, 1, &[0x4])), Action::Refuse(Deny::Clear(8)), "every key (XIAnyKeycode)");
        assert_eq!(classify(&mut c, &xi2(ROOT, 38, 1, &[0x4, 0])), Action::Refuse(Deny::Clear(8)), "a letter on its own among them");
        assert_eq!(classify(&mut c, &xi2(ROOT, 38, 1, &[1 << 31])), Action::Refuse(Deny::Clear(8)), "any modifier");
        assert_eq!(classify(&mut c, &xi2(ROOT, 38, 1, &[0x4, 0x14])), Action::Pass, "Ctrl+A, Num Lock on or off");
        assert_eq!(classify(&mut c, &xi2(ROOT, 0, 3, &[0])), Action::Refuse(Deny::Clear(8)), "the keyboard whenever it comes to the desktop");
        assert_eq!(classify(&mut c, &xi2(ROOT, 1, 0, &[0])), Action::Pass, "a mouse button");
        assert_eq!(classify(&mut c, &xi2(MY_WIN, 0, 1, &[0])), Action::Pass, "its own window");
        GRABS.with(|g| g.set(MOST_KEY_GRABS));
        assert_eq!(classify(&mut c, &xi2(ROOT, 96, 1, &[0])), Action::Refuse(Deny::Clear(8)), "too many, counted with the core ones");
    }

    #[test]
    fn strips_keys_whatever_the_byte_order() {
        // The mask is bytes, not numbers: a program that says it's
        // big-endian has KeyPress (2) in the same place.
        const BE: Order = Order(false);
        let mut b = vec![0u8; 20];
        b[0] = 131;
        b[1] = 46;
        BE.put_u16(&mut b, 2, 5);
        BE.put_u32(&mut b, 4, ROOT);
        BE.put_u16(&mut b, 8, 1);
        BE.put_u16(&mut b, 14, 1);
        b[16..20].copy_from_slice(&[1 << 2 | 1 << 4, 1 << 5 | 1 << 6, 1 << 1, 0]);
        strip_keys(&mut b, BE, false);
        assert_eq!(b[16..20], [1 << 4, 0, 1 << 1, 0], "KeyPress, RawKeyPress, RawKeyRelease gone; ButtonPress, RawMotion stay");
    }

    #[test]
    fn a_keyboards_state_without_its_keys() {
        // One keyboard class (0, 36 long, 248 keys, pad, 32 bytes), one button class (1, 36 long).
        let mut p = vec![0u8; 32];
        p[0] = 1;
        p[8] = 2;
        p.extend_from_slice(&[0, 36, 248, 0]);
        p.extend_from_slice(&[0xff; 32]);
        p.extend_from_slice(&[1, 36, 5, 0]);
        p.extend_from_slice(&[0x0f; 32]);
        let r = without_keys_down(&p);
        assert_eq!(r[32..36], [0, 36, 248, 0]);
        assert!(r[36..68].iter().all(|&b| b == 0), "no key is down");
        assert_eq!(r[68..], p[68..], "the buttons as they were");
        assert_eq!(without_keys_down(&p[..40]).len(), 40, "cut short: no panic");
    }

    #[test]
    fn turning_long_requests_on_is_watched() {
        assert_eq!(classify(&mut client(), &req(133, 0, &[], 4)), Action::Watch(Expect::BigRequests));
    }
}
