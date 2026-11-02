//! What a program may ask the display for, request by request.
//!
//! A program draws, reads its own pictures and talks to the window manager
//! as before. What it may not do unasked is see the screen: any request
//! that would copy what's on screen, or what other programs drew, to
//! somewhere it can read the pixels back. Those wait for the person's yes
//! (`Action::Ask`). What it may never do: press keys or move the mouse as
//! if it were the person (XTEST), record or snoop on what they type (RECORD,
//! raw key events, grabbing every key), or push other programs' windows
//! around. Those are turned into requests that do nothing.

use crate::wire::Req;
use std::collections::{HashMap, HashSet};

/// The core requests looked at.
mod core {
    pub const SEND_EVENT: u8 = 25;
    pub const CHANGE_PROPERTY: u8 = 18;
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
}

/// The names (atoms) the rules go by.
#[derive(Clone, Debug, Default)]
pub struct Atoms {
    /// Who may see the screen now, for the taskbar's light; only the gate writes it.
    pub screen_in_use: u32,
    /// Messages any program may send to another's window: the tray, XEMBED,
    /// FerroOS's own between its programs (refresh, the clipboard history).
    pub messages: Vec<u32>,
    /// Drag and drop's, which carry the sender's window first.
    pub xdnd: Vec<u32>,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// QueryExtension for this name: hidden ones are said not to exist.
    Extension(Vec<u8>),
    /// ListExtensions: the hidden ones left out.
    Extensions,
    /// QueryKeymap: which keys are down, only while the program has the keyboard.
    Keymap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Pass,
    /// The person decides.
    Ask(Read, Deny),
    Refuse(Deny),
    /// Goes through; its answer is looked at.
    Watch(Expect),
    /// Goes through without the key events it asked for on others' windows.
    StripKeys,
}

/// What the rules remember about one connection.
#[derive(Debug, Default)]
pub struct Client {
    /// Its own pixmaps: what it may read back freely.
    pixmaps: HashSet<u32>,
    /// Its RENDER pictures and what each is a picture of.
    pictures: HashMap<u32, u32>,
    /// Keys grabbed on windows not its own (global shortcuts).
    key_grabs: usize,
}

/// More global shortcuts than this is grabbing the keyboard key by key.
const MOST_KEY_GRABS: usize = 64;

impl Client {
    /// Pixels it may read back: its own pixmaps, and the desktop's background.
    fn readable(&self, w: &dyn World, id: u32) -> bool {
        self.pixmaps.contains(&id) || id == w.background()
    }

    fn picture_drawable(&self, pic: u32) -> Option<u32> {
        self.pictures.get(&pic).copied()
    }

    /// RENDER drawing from `src` (and `mask`) into `dst`: seeing the screen
    /// if a source is a window or another program's pixmap and the result
    /// lands in a pixmap it can read.
    fn render_reads(&self, w: &dyn World, srcs: &[u32], dst: u32) -> bool {
        let Some(into) = self.picture_drawable(dst) else { return false };
        self.pixmaps.contains(&into) && srcs.iter().filter_map(|&p| self.picture_drawable(p)).any(|d| !self.readable(w, d))
    }

    pub fn classify(&mut self, w: &dyn World, ops: &Opcodes, atoms: &Atoms, r: &Req) -> Action {
        let op = r.opcode();
        let get = |at| r.u32(at).unwrap_or(0);
        if ops.hidden.contains(&op) {
            return Action::Refuse(Deny::NoOp);
        }
        match op {
            core::CREATE_PIXMAP => {
                self.pixmaps.insert(get(4));
                Action::Pass
            }
            core::FREE_PIXMAP => {
                self.pixmaps.remove(&get(4));
                Action::Pass
            }
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
                if !self.readable(w, src) && src != dst && self.pixmaps.contains(&dst) {
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
                let (window, key) = (get(4), r.u8(10).unwrap_or(0));
                if w.mine(window) {
                    Action::Pass
                } else if key == 0 || self.key_grabs >= MOST_KEY_GRABS {
                    // Every key (AnyKey), or one by one: a keylogger.
                    Action::Refuse(Deny::NoOp)
                } else {
                    self.key_grabs += 1;
                    Action::Pass
                }
            }
            core::QUERY_KEYMAP => Action::Watch(Expect::Keymap),
            core::QUERY_EXTENSION => {
                let n = usize::from(r.u16(4).unwrap_or(0));
                let at = r.at(8);
                Action::Watch(Expect::Extension(r.bytes.get(at..at + n).unwrap_or_default().to_vec()))
            }
            core::LIST_EXTENSIONS => Action::Watch(Expect::Extensions),
            core::CHANGE_PROPERTY | core::DELETE_PROPERTY if get(4) == w.root() && get(8) == atoms.screen_in_use && atoms.screen_in_use != 0 => {
                Action::Refuse(Deny::NoOp)
            }
            // Freezing every other program, the question included.
            core::GRAB_SERVER => Action::Refuse(Deny::NoOp),
            // Letting other computers onto the display.
            core::CHANGE_HOSTS | core::SET_ACCESS_CONTROL => Action::Refuse(Deny::NoOp),
            core::KILL_CLIENT if !w.mine(get(4)) => Action::Refuse(Deny::NoOp),
            _ => self.extension(w, ops, r),
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
                5 => {
                    self.pixmaps.insert(get(4));
                }
                _ => {}
            }
        } else if op == ops.composite {
            match minor {
                // Becoming a compositor: every window's pixels. Not for programs.
                1 | 2 if !w.mine(get(4)) => return Action::Refuse(Deny::NoOp),
                6 => return Action::Ask(Read::WindowPixmap, Deny::NoOp),
                _ => {}
            }
        } else if op == ops.render {
            match minor {
                4 => {
                    self.pictures.insert(get(4), get(8));
                }
                7 => {
                    self.pictures.remove(&get(4));
                }
                8 if self.render_reads(w, &[get(8), get(12)], get(16)) => return Action::Ask(Read::Copy, Deny::NoOp),
                10..=13 | 23..=25 if self.render_reads(w, &[get(8)], get(12)) => return Action::Ask(Read::Copy, Deny::NoOp),
                // A cursor made of the screen, read back through XFIXES.
                27 if self.picture_drawable(get(8)).is_some_and(|d| !self.readable(w, d)) => return Action::Ask(Read::Copy, Deny::NoOp),
                _ => {}
            }
        } else if op == ops.xinput {
            match minor {
                // XInput 1: key events from another's window, or grabbing its keys.
                6 | 15 if !w.mine(get(4)) => return Action::Refuse(Deny::NoOp),
                46 if !w.mine(get(4)) => return Action::StripKeys,
                // XInput 2 grabbing every key on another's window (it answers: an error instead).
                54 if !w.mine(get(8)) && r.u8(26) == Some(1) && (get(16) == 1 << 31 || self.key_grabs >= MOST_KEY_GRABS) => {
                    return Action::Refuse(Deny::Clear(8))
                }
                54 if !w.mine(get(8)) && r.u8(26) == Some(1) => self.key_grabs += 1,
                _ => {}
            }
        } else if op == ops.dri3 {
            match minor {
                2 | 7 => {
                    self.pixmaps.insert(get(4));
                }
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
        }
        Action::Pass
    }
}

/// XInput 2's event selection on a window not the program's own, without
/// key presses and releases and the raw (everywhere) key events. Masks are
/// (device u16, length u16 in 4-byte units, bits) from offset 12.
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
            // Each 4-byte unit is a 32-bit number in the program's byte order.
            let (unit, bit) = (k / 32, k % 32);
            let pos = bits + unit * 4;
            if unit * 4 < len && r.len() >= pos + 4 {
                let v = order.u32(r, pos) & !(1 << bit);
                order.put_u32(r, pos, v);
            }
        }
        at = bits + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Order;

    const LE: Order = Order(true);
    const ROOT: u32 = 0x100;
    const BG: u32 = 0x200;

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
        let ops =
            Opcodes { shm: Some(130), composite: Some(142), render: Some(139), xinput: Some(131), hidden: vec![132, 146], ..Default::default() };
        let atoms = Atoms { screen_in_use: 900, messages: vec![901], xdnd: vec![902] };
        c.classify(&Fake, &ops, &atoms, &Req { order: LE, bytes: b, big: false })
    }

    const MY_WIN: u32 = 0x0400_0001;
    const MY_PIX: u32 = 0x0400_0002;
    const THEIRS: u32 = 0x0300_0001;

    #[test]
    fn seeing_the_screen_asks() {
        let mut c = Client::default();
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
    fn shared_memory_and_compositing() {
        let mut c = Client::default();
        let shm_get = req(130, 4, &[(4, ROOT), (16, !0)], 32);
        assert!(matches!(classify(&mut c, &shm_get), Action::Ask(Read::Image { shm: true, .. }, Deny::Black)));
        assert_eq!(classify(&mut c, &req(130, 1, &[(4, 1), (8, 9)], 16)), Action::Refuse(Deny::NoOp), "another's memory");
        assert_eq!(classify(&mut c, &req(130, 1, &[(4, 1), (8, 7)], 16)), Action::Pass);
        assert_eq!(classify(&mut c, &req(142, 2, &[(4, ROOT)], 12)), Action::Refuse(Deny::NoOp), "redirecting the whole screen");
        assert_eq!(classify(&mut c, &req(142, 1, &[(4, MY_WIN)], 12)), Action::Pass);
        assert_eq!(classify(&mut c, &req(142, 6, &[(4, THEIRS), (8, MY_PIX)], 12)), Action::Ask(Read::WindowPixmap, Deny::NoOp));
    }

    #[test]
    fn render_from_a_window_into_a_pixmap_asks() {
        let mut c = Client::default();
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
        let mut c = Client::default();
        assert_eq!(classify(&mut c, &req(132, 2, &[], 36)), Action::Refuse(Deny::NoOp), "XTEST FakeInput");
        assert_eq!(classify(&mut c, &req(146, 5, &[(4, 3)], 8)), Action::Refuse(Deny::NoOp), "RECORD");
        assert_eq!(classify(&mut c, &req(core::GRAB_KEY, 1, &[(4, ROOT)], 16)), Action::Refuse(Deny::NoOp), "AnyKey on the desktop");
        let mut one_key = req(core::GRAB_KEY, 1, &[(4, ROOT)], 16);
        one_key[10] = 38;
        assert_eq!(classify(&mut c, &one_key), Action::Pass, "a global shortcut");
        for _ in 0..MOST_KEY_GRABS {
            classify(&mut c, &one_key);
        }
        assert_eq!(classify(&mut c, &one_key), Action::Refuse(Deny::NoOp), "every key, one at a time");
        assert_eq!(classify(&mut c, &req(131, 46, &[(4, ROOT)], 20)), Action::StripKeys);
        assert_eq!(classify(&mut c, &req(131, 46, &[(4, MY_WIN)], 20)), Action::Pass);
        assert_eq!(classify(&mut c, &req(core::QUERY_KEYMAP, 0, &[], 4)), Action::Watch(Expect::Keymap));
        assert_eq!(classify(&mut c, &req(core::GRAB_SERVER, 0, &[], 4)), Action::Refuse(Deny::NoOp), "the question must still show");
    }

    #[test]
    fn messages_to_others() {
        let mut c = Client::default();
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
    fn strips_only_key_events() {
        // One mask, one unit: KeyPress (2), ButtonPress (4), RawKeyPress (13), RawMotion (17).
        let mut b = req(131, 46, &[(4, ROOT)], 20);
        LE.put_u16(&mut b, 8, 1);
        LE.put_u16(&mut b, 14, 1);
        LE.put_u32(&mut b, 16, 1 << 2 | 1 << 4 | 1 << 13 | 1 << 17);
        strip_keys(&mut b, LE, false);
        assert_eq!(LE.u32(&b, 16), 1 << 4 | 1 << 17, "clicks and raw motion (games) stay");
    }
}
