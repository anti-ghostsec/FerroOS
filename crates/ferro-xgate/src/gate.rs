//! The gate itself: programs connect to it as if it were the display (`:0`)
//! and it connects each to the real one (`:5`, which only root can reach),
//! handing the real cookie over for them. What goes each way is passed on
//! request by request, reply by reply; what a program may not do (policy.rs)
//! is changed on the way into something harmless, and what it may only do
//! with the person's yes waits for their answer (popup.rs).

use crate::policy::{self, Action, Atoms, Deny, Expect, Opcodes, Read, World};
use crate::wire::{self, Order, Req, Setup};
use crate::{popup, sock, trust};
use std::collections::HashMap;
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode};
use x11rb::protocol::Event;
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;

/// How long Allow once lasts: a screenshot may be taken in several pieces.
const ONCE_FOR: Duration = Duration::from_secs(3);
/// After Don't allow, it isn't asked again at once (a program asking in a loop).
const REFUSED_FOR: Duration = Duration::from_secs(5);

/// Which process: its number and when it started, since numbers are used again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Proc {
    pub pid: u32,
    pub start: u64,
}

#[derive(Default)]
struct State {
    /// Each connection's ids (its base) and whose they are.
    owners: HashMap<u32, Proc>,
    /// May see the screen until it quits, and what it's called (for the taskbar's light).
    always: HashMap<Proc, String>,
    once: HashMap<Proc, Instant>,
    refused: HashMap<Proc, Instant>,
}

pub struct Gate {
    upstream: PathBuf,
    cookie: Vec<u8>,
    /// The person whose session it is; root may connect too.
    uid: u32,
    /// The gate's own connection to the display.
    control: RustConnection,
    screen_num: usize,
    mask: u32,
    root: u32,
    /// _NET_ACTIVE_WINDOW: the window the desktop says is in use.
    active_atom: u32,
    ops: Opcodes,
    atoms: Atoms,
    background: AtomicU32,
    background_atom: u32,
    utf8: u32,
    state: Mutex<State>,
    /// One question at a time.
    asking: Mutex<()>,
}

fn connect(path: &PathBuf, cookie: &[u8]) -> Result<RustConnection, Box<dyn std::error::Error>> {
    let (stream, _) = DefaultStream::from_unix_stream(UnixStream::connect(path)?)?;
    Ok(RustConnection::connect_to_stream_with_auth_info(stream, 0, b"MIT-MAGIC-COOKIE-1".to_vec(), cookie.to_vec())?)
}

impl Gate {
    pub fn new(upstream: PathBuf, cookie: Vec<u8>, uid: u32) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let control = connect(&upstream, &cookie)?;
        let screen = &control.setup().roots[0];
        let (root, mask) = (screen.root, control.setup().resource_id_mask);
        let ext = |name: &str| -> Option<u8> {
            let r = control.query_extension(name.as_bytes()).ok()?.reply().ok()?;
            r.present.then_some(r.major_opcode)
        };
        let ops = Opcodes {
            shm: ext("MIT-SHM"),
            composite: ext("Composite"),
            render: ext("RENDER"),
            xinput: ext("XInputExtension"),
            hidden: policy::HIDDEN.iter().filter_map(|n| ext(std::str::from_utf8(n).unwrap_or_default())).collect(),
            dri2: ext("DRI2"),
            dri3: ext("DRI3"),
            glx: ext("GLX"),
        };
        let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> { Ok(control.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let messages = ["_NET_SYSTEM_TRAY_OPCODE", "_XEMBED", "_FERRO_REFRESH", "_FERRO_CLIPBOARD_PICK", "WM_PROTOCOLS"];
        let xdnd = ["XdndEnter", "XdndPosition", "XdndStatus", "XdndLeave", "XdndDrop", "XdndFinished"];
        let atoms = Atoms {
            screen_in_use: atom("_FERRO_SCREEN")?,
            messages: messages.iter().map(|n| atom(n)).collect::<Result<_, _>>()?,
            xdnd: xdnd.iter().map(|n| atom(n)).collect::<Result<_, _>>()?,
            // WM_COMMAND, WM_ICON_NAME, WM_NAME, and EWMH's.
            private: [34, 37, 39]
                .into_iter()
                .map(Ok)
                .chain(["_NET_WM_NAME", "_NET_WM_ICON_NAME", "_NET_WM_VISIBLE_NAME", "_NET_WM_VISIBLE_ICON_NAME"].iter().map(|n| atom(n)))
                .collect::<Result<_, _>>()?,
            dirs: vec![atom("_FERRO_CURRENT_DIR")?],
            selections: vec![atom("CLIPBOARD")?, 1, 2],
        };
        let background_atom = atom("_XROOTPMAP_ID")?;
        let active_atom = atom("_NET_ACTIVE_WINDOW")?;
        let utf8 = atom("UTF8_STRING")?;
        // Told when the desktop's background changes.
        control.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
        // Nobody may see the screen yet; a light left by a gate before this one goes.
        control.delete_property(root, atoms.screen_in_use)?;
        control.flush()?;
        let gate = Arc::new(Gate {
            upstream,
            cookie,
            uid,
            control,
            screen_num: 0,
            mask,
            root,
            active_atom,
            ops,
            atoms,
            background: AtomicU32::new(0),
            background_atom,
            utf8,
            state: Mutex::default(),
            asking: Mutex::new(()),
        });
        gate.read_background();
        Ok(gate)
    }

    fn read_background(&self) {
        let id = self
            .control
            .get_property(false, self.root, self.background_atom, AtomEnum::PIXMAP, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .unwrap_or(0);
        self.background.store(id, Ordering::Relaxed);
    }

    /// Watches the display for the gate's lifetime: when it goes, so does the gate.
    pub fn watch(self: &Arc<Self>) -> ! {
        loop {
            match self.control.wait_for_event() {
                Ok(Event::PropertyNotify(p)) if p.atom == self.background_atom => self.read_background(),
                Ok(_) => {}
                Err(_) => std::process::exit(0),
            }
        }
    }

    pub fn serve(self: &Arc<Self>, listener: UnixListener) {
        for client in listener.incoming().flatten() {
            let gate = Arc::clone(self);
            std::thread::spawn(move || {
                let _ = gate.handle(client);
            });
        }
    }

    fn publish(&self, state: &State) {
        let mut names: Vec<&str> = state.always.values().map(String::as_str).collect();
        names.sort_unstable();
        names.dedup();
        let _ = if names.is_empty() {
            self.control.delete_property(self.root, self.atoms.screen_in_use).map(drop)
        } else {
            self.control.change_property8(PropMode::REPLACE, self.root, self.atoms.screen_in_use, self.utf8, names.join("\n").as_bytes()).map(drop)
        };
        let _ = self.control.flush();
    }

    /// Allowed to see the screen right now (without asking).
    fn may_see(&self, who: Proc) -> bool {
        let s = self.state.lock().unwrap();
        s.always.contains_key(&who) || s.once.get(&who).is_some_and(|&t| t > Instant::now())
    }

    /// The person's answer (or one they gave a moment ago) to whether `who` may see the screen.
    fn allowed(&self, who: Proc) -> bool {
        let known = |s: &State| -> Option<bool> {
            let now = Instant::now();
            if s.always.contains_key(&who) || s.once.get(&who).is_some_and(|&t| t > now) {
                Some(true)
            } else if s.refused.get(&who).is_some_and(|&t| t > now) {
                Some(false)
            } else {
                None
            }
        };
        if let Some(yes) = known(&self.state.lock().unwrap()) {
            return yes;
        }
        let _turn = self.asking.lock().unwrap();
        // Answered while it waited its turn (another of its connections asked).
        if let Some(yes) = known(&self.state.lock().unwrap()) {
            return yes;
        }
        let name = trust::name(who.pid);
        let answer = match connect(&self.upstream, &self.cookie) {
            Ok(conn) => popup::ask(&conn, self.screen_num, &name, who.pid),
            Err(_) => popup::Answer::No,
        };
        let mut s = self.state.lock().unwrap();
        // Gone while the question was up.
        if !s.owners.values().any(|&p| p == who) {
            return false;
        }
        match answer {
            popup::Answer::Once => {
                s.once.insert(who, Instant::now() + ONCE_FOR);
                true
            }
            popup::Answer::UntilQuit => {
                s.always.insert(who, name);
                self.publish(&s);
                true
            }
            popup::Answer::No => {
                s.refused.insert(who, Instant::now() + REFUSED_FOR);
                false
            }
        }
    }

    fn handle(self: &Arc<Self>, client: UnixStream) -> io::Result<()> {
        let Some((pid, uid)) = sock::peer(&client) else { return Ok(()) };
        if uid != self.uid && uid != 0 {
            return Ok(());
        }
        let mut head = [0u8; 12];
        (&client).read_exact(&mut head)?;
        let (order, major, minor, len) = wire::setup_request(&head).ok_or(io::ErrorKind::InvalidData)?;
        // What it offered as a cookie doesn't matter: who it is, is known.
        let mut offered = vec![0u8; len - 12];
        (&client).read_exact(&mut offered)?;
        let up = UnixStream::connect(&self.upstream)?;
        (&up).write_all(&wire::setup_request_with(order, major, minor, &self.cookie))?;
        let mut reply = vec![0u8; 8];
        (&up).read_exact(&mut reply)?;
        reply.resize(wire::setup_reply_len(order, &reply), 0);
        (&up).read_exact(&mut reply[8..])?;
        (&client).write_all(&reply)?;
        let Some(setup) = wire::parse_setup(order, &reply) else { return Ok(()) };

        let who = Proc { pid, start: trust::started(pid) };
        self.state.lock().unwrap().owners.insert(setup.base, who);
        // Root may do all this and more without the gate.
        let why = if uid == 0 { Ok(()) } else { trust::trusted(pid) };
        let trusted = why.is_ok();
        eprintln!(
            "ferro-xgate: {} (process {pid}) connected, {}",
            trust::name(pid),
            why.map_or_else(|e| format!("looked at: {e}"), |()| "trusted".into())
        );
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (c2, u2) = (client.try_clone()?, up.try_clone()?);
        let back = {
            let (gate, pending, setup) = (Arc::clone(self), Arc::clone(&pending), setup.clone());
            std::thread::spawn(move || {
                let _ = if trusted { pump(&u2, &c2) } else { gate.replies(&u2, &c2, order, &setup, who, &pending) };
                let _ = c2.shutdown(std::net::Shutdown::Both);
                let _ = u2.shutdown(std::net::Shutdown::Both);
            })
        };
        let _ = if trusted { pump(&client, &up) } else { self.requests(&client, &up, order, who, uid, &pending) };
        let _ = client.shutdown(std::net::Shutdown::Both);
        let _ = up.shutdown(std::net::Shutdown::Both);
        let _ = back.join();

        let mut s = self.state.lock().unwrap();
        s.owners.remove(&setup.base);
        if !s.owners.values().any(|&p| p == who) {
            s.once.remove(&who);
            s.refused.remove(&who);
            if s.always.remove(&who).is_some() {
                self.publish(&s);
            }
        }
        Ok(())
    }

    /// The program's requests on their way to the display.
    fn requests(
        &self,
        client: &UnixStream,
        up: &UnixStream,
        order: Order,
        who: Proc,
        uid: u32,
        pending: &Mutex<HashMap<u16, Pending>>,
    ) -> io::Result<()> {
        let world = Seen { gate: self, who, uid };
        let mut rules = policy::Client::new(trust::is_file_manager(who.pid));
        let (mut buf, mut have, mut fds) = (vec![0u8; 64 * 1024], Vec::new(), Vec::<OwnedFd>::new());
        let mut seq: u16 = 0;
        loop {
            let n = sock::recv(client, &mut buf, &mut fds)?;
            if n == 0 {
                return Ok(());
            }
            have.extend_from_slice(&buf[..n]);
            let (mut out, mut at) = (Vec::with_capacity(have.len()), 0);
            while let Some((len, big)) = wire::request_len(order, &have[at..]) {
                if have.len() - at < len {
                    break;
                }
                seq = seq.wrapping_add(1);
                let bytes = &have[at..at + len];
                at += len;
                let r = Req { order, bytes, big };
                match rules.classify(&world, &self.ops, &self.atoms, &r) {
                    Action::Pass => out.extend_from_slice(bytes),
                    Action::Watch(e) => {
                        pending.lock().unwrap().insert(seq, Pending::Watch(e));
                        out.extend_from_slice(bytes);
                    }
                    Action::StripKeys => {
                        let mut b = bytes.to_vec();
                        policy::strip_keys(&mut b, order, big);
                        out.extend_from_slice(&b);
                    }
                    Action::Refuse(deny) => out.extend_from_slice(&denied(&r, deny, None, seq, pending)),
                    Action::Ask(read, deny) => {
                        // What came before it is drawn first: the screen as the program left it.
                        sock::send(up, &out, &mut fds)?;
                        out.clear();
                        if self.allowed(who) {
                            out.extend_from_slice(bytes);
                        } else {
                            out.extend_from_slice(&denied(&r, deny, Some(read), seq, pending));
                        }
                    }
                }
            }
            have.drain(..at);
            sock::send(up, &out, &mut fds)?;
        }
    }

    /// The display's replies, errors and events on their way to the program.
    fn replies(
        &self,
        up: &UnixStream,
        client: &UnixStream,
        order: Order,
        setup: &Setup,
        who: Proc,
        pending: &Mutex<HashMap<u16, Pending>>,
    ) -> io::Result<()> {
        let (mut buf, mut have, mut fds) = (vec![0u8; 64 * 1024], Vec::new(), Vec::<OwnedFd>::new());
        loop {
            let n = sock::recv(up, &mut buf, &mut fds)?;
            if n == 0 {
                return Ok(());
            }
            have.extend_from_slice(&buf[..n]);
            let (mut out, mut at) = (Vec::with_capacity(have.len()), 0);
            while have.len() - at >= 32 {
                let len = wire::packet_len(order, &have[at..at + 32]);
                if have.len() - at < len {
                    break;
                }
                let p = &have[at..at + len];
                at += len;
                let waiting = if p[0] <= 1 { wire::sequence(order, p).and_then(|s| pending.lock().unwrap().remove(&s)) } else { None };
                match waiting {
                    // An error stays one.
                    Some(w) if p[0] == 1 => out.extend_from_slice(&self.answer(w, p, order, setup, who)),
                    _ => out.extend_from_slice(p),
                }
            }
            have.drain(..at);
            sock::send(client, &out, &mut fds)?;
        }
    }

    /// The reply the program gets in place of the display's.
    fn answer(&self, w: Pending, p: &[u8], order: Order, setup: &Setup, who: Proc) -> Vec<u8> {
        match w {
            Pending::Black { shm, format, w, h, plane_mask } => black(setup, order, p, shm, format, w, h, plane_mask),
            Pending::Watch(Expect::Extension(name)) if policy::HIDDEN.contains(&name.as_slice()) => {
                let mut r = p.to_vec();
                r[8..12].fill(0);
                r
            }
            Pending::Watch(Expect::Extension(_)) => p.to_vec(),
            Pending::Watch(Expect::Extensions) => without_hidden(order, p),
            Pending::Watch(Expect::Private) if self.may_see(who) => p.to_vec(),
            // As if the window had no such property.
            Pending::Watch(Expect::Private) => {
                let mut r = vec![0u8; 32];
                r[0] = 1;
                r[2..4].copy_from_slice(&p[2..4]);
                r
            }
            Pending::Watch(Expect::Keymap) => {
                let focus = self.control.get_input_focus().ok().and_then(|c| c.reply().ok()).map_or(0, |r| r.focus);
                let mut r = p.to_vec();
                // Which keys are down is for whoever has the keyboard.
                let theirs = self.state.lock().unwrap().owners.get(&(focus & !self.mask)).copied();
                if focus <= 1 || theirs != Some(who) {
                    let end = r.len().min(40);
                    r[8..end].fill(0);
                }
                r
            }
        }
    }
}

/// Programs the gate doesn't need to look at: everything passed on, files too.
fn pump(from: &UnixStream, to: &UnixStream) -> io::Result<()> {
    let (mut buf, mut fds) = (vec![0u8; 64 * 1024], Vec::<OwnedFd>::new());
    loop {
        let n = sock::recv(from, &mut buf, &mut fds)?;
        if n == 0 {
            return Ok(());
        }
        sock::send(to, &buf[..n], &mut fds)?;
    }
}

/// A request's answer the gate makes up or changes.
#[derive(Debug, PartialEq, Eq)]
pub enum Pending {
    /// A black picture, its size from the drawable's depth (GetGeometry's reply).
    Black {
        shm: bool,
        format: u8,
        w: u16,
        h: u16,
        plane_mask: u32,
    },
    Watch(Expect),
}

/// What's sent instead of a request that may not go through.
fn denied(r: &Req, deny: Deny, read: Option<Read>, seq: u16, pending: &Mutex<HashMap<u16, Pending>>) -> Vec<u8> {
    let mut b = r.bytes.to_vec();
    match (deny, read) {
        (Deny::Black, Some(Read::Image { shm, drawable, format, w, h, plane_mask })) => {
            pending.lock().unwrap().insert(seq, Pending::Black { shm, format, w, h, plane_mask });
            // Its depth, under the same number: the made-up picture's reply.
            b = vec![xproto::GET_GEOMETRY_REQUEST, 0, 0, 0, 0, 0, 0, 0];
            r.order.put_u16(&mut b, 2, 2);
            r.order.put_u32(&mut b, 4, drawable);
        }
        (Deny::Clear(at), _) => {
            let at = r.at(at);
            if b.len() >= at + 4 {
                r.order.put_u32(&mut b, at, 0);
            }
        }
        (Deny::NothingCopied, _) => {
            // SendEvent of SelectionNotify, property None, to its own window,
            // as an owner with nothing to give would send.
            let (requestor, selection, target, time) = (r.u32(4).unwrap_or(0), r.u32(8).unwrap_or(0), r.u32(12).unwrap_or(0), r.u32(20).unwrap_or(0));
            b = vec![0u8; 44];
            b[0] = xproto::SEND_EVENT_REQUEST;
            r.order.put_u16(&mut b, 2, 11);
            r.order.put_u32(&mut b, 4, requestor);
            b[12] = xproto::SELECTION_NOTIFY_EVENT;
            r.order.put_u32(&mut b, 16, time);
            r.order.put_u32(&mut b, 20, requestor);
            r.order.put_u32(&mut b, 24, selection);
            r.order.put_u32(&mut b, 28, target);
        }
        _ => b[0] = policy::NO_OPERATION,
    }
    b
}

/// A black GetImage (or MIT-SHM GetImage) reply in place of `geometry`, GetGeometry's.
#[allow(clippy::too_many_arguments)]
pub fn black(setup: &Setup, order: Order, geometry: &[u8], shm: bool, format: u8, w: u16, h: u16, plane_mask: u32) -> Vec<u8> {
    let depth = geometry[1];
    let bytes = wire::image_bytes(setup, format, depth, plane_mask, w, h);
    let visual = setup.roots.first().filter(|r| r.2 == depth).map_or(0, |r| r.1);
    let mut r = vec![0u8; 32];
    r[0] = 1;
    r[1] = depth;
    r[2..4].copy_from_slice(&geometry[2..4]);
    order.put_u32(&mut r, 8, visual);
    if shm {
        // The picture would be in the program's shared memory: it's left as it was.
        order.put_u32(&mut r, 12, bytes as u32);
    } else {
        let padded = wire::pad4(bytes);
        order.put_u32(&mut r, 4, (padded / 4) as u32);
        r.resize(32 + padded, 0);
    }
    r
}

/// ListExtensions' reply without the hidden ones.
pub fn without_hidden(order: Order, p: &[u8]) -> Vec<u8> {
    let mut names = Vec::new();
    let mut at = 32;
    for _ in 0..p[1] {
        let Some(&n) = p.get(at) else { break };
        let Some(name) = p.get(at + 1..at + 1 + usize::from(n)) else { break };
        if !policy::HIDDEN.contains(&name) {
            names.push(name);
        }
        at += 1 + usize::from(n);
    }
    let mut r = p[..32].to_vec();
    r[1] = names.len() as u8;
    for n in &names {
        r.push(n.len() as u8);
        r.extend_from_slice(n);
    }
    r.resize(32 + wire::pad4(r.len() - 32), 0);
    let units = ((r.len() - 32) / 4) as u32;
    order.put_u32(&mut r, 4, units);
    r
}

/// The display as one program's requests see it.
struct Seen<'a> {
    gate: &'a Gate,
    who: Proc,
    uid: u32,
}

impl World for Seen<'_> {
    fn mine(&self, id: u32) -> bool {
        id != 0 && self.gate.state.lock().unwrap().owners.get(&(id & !self.gate.mask)) == Some(&self.who)
    }

    fn root(&self) -> u32 {
        self.gate.root
    }

    fn background(&self) -> u32 {
        self.gate.background.load(Ordering::Relaxed)
    }

    fn may_use_shm(&self, shmid: u32) -> bool {
        // SAFETY: shmid_ds is plain data, filled by IPC_STAT.
        let mut ds: libc::shmid_ds = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::shmctl(shmid as i32, libc::IPC_STAT, &mut ds) } != 0 {
            return false;
        }
        let (perm, mode) = (&ds.shm_perm, u32::from(ds.shm_perm.mode as u16));
        // Its own, or anyone's.
        perm.uid == self.uid || perm.cuid == self.uid || mode & 0o006 == 0o006
    }

    /// Being used by the person: it has the keyboard, or the desktop says
    /// its window is the one in use (the keyboard may still be on its way
    /// there after a click), or it's under the mouse pointer (a right-click
    /// menu's Paste: the program asks what there is to paste while it
    /// builds the menu, before focus has moved). Not: a program in the
    /// background the person isn't using or pointing at.
    fn focused(&self) -> bool {
        let c = &self.gate.control;
        let focus = c.get_input_focus().ok().and_then(|c| c.reply().ok()).map_or(0, |r| r.focus);
        if focus > 1 && self.mine(focus) {
            return true;
        }
        let active = c
            .get_property(false, self.gate.root, self.gate.active_atom, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .unwrap_or(0);
        if active != 0 && self.mine(active) {
            return true;
        }
        // Down from the screen through the desktop's frames to the window
        // the pointer is over.
        let mut win = self.gate.root;
        for _ in 0..8 {
            let Some(r) = c.query_pointer(win).ok().and_then(|c| c.reply().ok()) else { break };
            if r.child == 0 {
                break;
            }
            if self.mine(r.child) {
                return true;
            }
            win = r.child;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LE: Order = Order(true);

    #[test]
    fn a_black_picture_the_right_size() {
        let setup = Setup { bitmap_pad: 32, formats: vec![(24, 32, 32)], roots: vec![(0x100, 0x21, 24)], ..Default::default() };
        let mut geometry = [0u8; 32];
        geometry[0] = 1;
        geometry[1] = 24;
        geometry[2] = 7;
        let r = black(&setup, LE, &geometry, false, 2, 10, 5, !0);
        assert_eq!(r.len(), 32 + 10 * 5 * 4);
        assert_eq!((r[0], r[1], LE.u16(&r, 2)), (1, 24, 7));
        assert_eq!(LE.u32(&r, 4), 50);
        assert_eq!(LE.u32(&r, 8), 0x21, "the screen's visual");
        assert!(r[32..].iter().all(|&b| b == 0));
        let s = black(&setup, LE, &geometry, true, 2, 10, 5, !0);
        assert_eq!((s.len(), LE.u32(&s, 4), LE.u32(&s, 12)), (32, 0, 200));
    }

    #[test]
    fn denied_requests() {
        let pending = Mutex::new(HashMap::new());
        let mut get = vec![73u8, 2, 5, 0];
        get.extend_from_slice(&0x100u32.to_le_bytes());
        get.extend_from_slice(&[0, 0, 0, 0, 10, 0, 5, 0, 255, 255, 255, 255]);
        let r = Req { order: LE, bytes: &get, big: false };
        let read = Read::Image { shm: false, drawable: 0x100, format: 2, w: 10, h: 5, plane_mask: !0 };
        let b = denied(&r, Deny::Black, Some(read), 9, &pending);
        assert_eq!(b, [14, 0, 2, 0, 0, 1, 0, 0], "GetGeometry of the same drawable");
        assert!(matches!(pending.lock().unwrap().get(&9), Some(Pending::Black { w: 10, h: 5, .. })));
        assert_eq!(denied(&r, Deny::NoOp, None, 10, &pending)[0], 127);
        assert_eq!(&denied(&r, Deny::Clear(4), None, 11, &pending)[4..8], [0, 0, 0, 0]);
        // Pasting from the background: told nothing was copied.
        let mut paste = vec![24u8, 0, 6, 0];
        for v in [0x0400_0001u32, 0x99, 0x1f, 0x4d, 1234] {
            paste.extend_from_slice(&v.to_le_bytes());
        }
        let b = denied(&Req { order: LE, bytes: &paste, big: false }, Deny::NothingCopied, None, 12, &pending);
        assert_eq!((b.len(), b[0], LE.u16(&b, 2), LE.u32(&b, 4)), (44, 25, 11, 0x0400_0001), "SendEvent to its own window");
        assert_eq!(
            (b[12], LE.u32(&b, 16), LE.u32(&b, 20), LE.u32(&b, 24), LE.u32(&b, 28), LE.u32(&b, 32)),
            (31, 1234, 0x0400_0001, 0x99, 0x1f, 0),
            "SelectionNotify, no property"
        );
    }

    #[test]
    fn hides_extensions_from_the_list() {
        let mut p = vec![1u8, 3, 1, 0, 0, 0, 0, 0];
        p.resize(32, 0);
        for n in [&b"RENDER"[..], b"XTEST", b"SHAPE"] {
            p.push(n.len() as u8);
            p.extend_from_slice(n);
        }
        p.resize(32 + wire::pad4(p.len() - 32), 0);
        let r = without_hidden(LE, &p);
        assert_eq!(r[1], 2);
        assert_eq!(&r[32..45], b"\x06RENDER\x05SHAPE");
        assert_eq!(r.len() % 4, 0);
        assert_eq!(LE.u32(&r, 4) as usize * 4, r.len() - 32);
    }
}
