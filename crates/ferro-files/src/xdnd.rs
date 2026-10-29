//! Dropping files on other programs' windows: XDND, the drag-and-drop the
//! browser, editors and the rest of the Linux desktop speak. FerroOS's own
//! drags (into folders, onto the desktop) are its own; let go over a window
//! that takes XDND drops (a page's "drop a picture here", say), the files are
//! offered to it as a list of file:// addresses, always to copy: the files
//! themselves stay where they are.
//!
//! The whole exchange happens when the button is let go: enter, one
//! position (where it was let go), and if the window says it takes it, the
//! drop; its request for the list is answered, then it says it's finished.
//! Each step waits a moment at most, so a window that never answers can't
//! hold the File Manager up.

use std::path::Path;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;

/// The version of XDND spoken (5, the one everything takes).
const VERSION: u32 = 5;

pub struct Atoms {
    aware: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    selection: Atom,
    copy: Atom,
    uri_list: Atom,
    text_plain: Atom,
    wm_type: Atom,
    desktop: Atom,
}

impl Atoms {
    pub fn new(conn: &RustConnection) -> Result<Self, Box<dyn std::error::Error>> {
        let a = |n: &[u8]| -> Result<Atom, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
        Ok(Self {
            aware: a(b"XdndAware")?,
            enter: a(b"XdndEnter")?,
            position: a(b"XdndPosition")?,
            status: a(b"XdndStatus")?,
            leave: a(b"XdndLeave")?,
            drop: a(b"XdndDrop")?,
            finished: a(b"XdndFinished")?,
            selection: a(b"XdndSelection")?,
            copy: a(b"XdndActionCopy")?,
            uri_list: a(b"text/uri-list")?,
            text_plain: a(b"text/plain")?,
            wm_type: a(b"_NET_WM_WINDOW_TYPE")?,
            desktop: a(b"_NET_WM_WINDOW_TYPE_DESKTOP")?,
        })
    }
}

/// What's under the pointer, for a drop.
#[derive(Debug, PartialEq, Eq)]
pub enum Under {
    /// The desktop (or nothing but the root).
    Desktop,
    /// A window that takes XDND drops, and the XDND version it speaks.
    Taker(Window, u32),
    /// Another program's window that doesn't take drops.
    Other,
}

fn property32(conn: &RustConnection, win: Window, prop: Atom) -> Vec<u32> {
    conn.get_property(false, win, prop, AtomEnum::ANY, 0, 32)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect()))
        .unwrap_or_default()
}

/// Down from the root's child under the pointer (a frame, usually) to the
/// window that says it takes drops, if one does.
pub fn under(conn: &RustConnection, root: Window, atoms: &Atoms) -> Under {
    let Some(mut win) = conn.query_pointer(root).ok().and_then(|c| c.reply().ok()).map(|q| q.child) else { return Under::Desktop };
    if win == x11rb::NONE {
        return Under::Desktop;
    }
    for _ in 0..8 {
        if property32(conn, win, atoms.wm_type).contains(&atoms.desktop) {
            return Under::Desktop;
        }
        if let Some(&v) = property32(conn, win, atoms.aware).first() {
            return Under::Taker(win, v.min(VERSION));
        }
        match conn.query_pointer(win).ok().and_then(|c| c.reply().ok()).map(|q| q.child) {
            Some(child) if child != x11rb::NONE => win = child,
            _ => break,
        }
    }
    Under::Other
}

/// A path as a file:// address: everything but letters, digits and -._~/
/// written as %XX, byte by byte.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The list as text/uri-list wants it: one address a line, CR LF after each.
pub fn uri_list(paths: &[std::path::PathBuf]) -> String {
    paths.iter().map(|p| file_uri(p) + "\r\n").collect()
}

fn send(conn: &RustConnection, to: Window, kind: Atom, data: [u32; 5]) {
    let _ = conn.send_event(false, to, EventMask::NO_EVENT, ClientMessageEvent::new(32, to, kind, data));
    let _ = conn.flush();
}

/// Answers a request for the dropped files (the window asks for the
/// selection, in one of the two kinds offered).
fn answer(conn: &RustConnection, atoms: &Atoms, req: &SelectionRequestEvent, list: &str) {
    let mut prop = req.property;
    if req.target == atoms.uri_list || req.target == atoms.text_plain {
        // text/plain: the paths themselves, a line each.
        let body = if req.target == atoms.uri_list {
            list.to_owned()
        } else {
            list.lines().filter_map(|l| l.strip_prefix("file://")).map(|p| percent_decode(p) + "\n").collect()
        };
        let _ = conn.change_property8(PropMode::REPLACE, req.requestor, req.property, req.target, body.as_bytes());
    } else {
        prop = x11rb::NONE;
    }
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: prop,
    };
    let _ = conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify);
    let _ = conn.flush();
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Waits (a little) for a client message of `kind` from `from`, answering
/// requests for the files meanwhile. Other events seen while waiting are
/// let go: the window is painted again afterwards.
fn wait_for(conn: &RustConnection, atoms: &Atoms, kind: Atom, from: Window, list: &str, within: Duration) -> Option<[u32; 5]> {
    let until = Instant::now() + within;
    while Instant::now() < until {
        match conn.poll_for_event() {
            Ok(Some(Event::ClientMessage(cm))) if cm.type_ == kind && cm.data.as_data32()[0] == from => return Some(cm.data.as_data32()),
            Ok(Some(Event::SelectionRequest(req))) if req.selection == atoms.selection => answer(conn, atoms, &req, list),
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
    None
}

/// Offers `paths` to `target` (a window and the XDND version it speaks),
/// let go at (`x`, `y`) on the screen, from our window `source`. True if it
/// took them.
pub fn drop_files(
    conn: &RustConnection,
    atoms: &Atoms,
    source: Window,
    (target, version): (Window, u32),
    (x, y): (i32, i32),
    time: Timestamp,
    paths: &[std::path::PathBuf],
) -> bool {
    let list = uri_list(paths);
    // The files are offered through the XdndSelection, which is ours now.
    let _ = conn.set_selection_owner(source, atoms.selection, time);
    // Enter: the version, and the two kinds offered (no more than three, so
    // no type list is needed).
    send(conn, target, atoms.enter, [source, version << 24, atoms.uri_list, atoms.text_plain, 0]);
    let at = ((x.clamp(0, 0xffff) as u32) << 16) | (y.clamp(0, 0xffff) as u32);
    send(conn, target, atoms.position, [source, 0, at, time, atoms.copy]);
    let accepted = wait_for(conn, atoms, atoms.status, target, &list, Duration::from_millis(1500)).is_some_and(|d| d[1] & 1 == 1);
    if !accepted {
        send(conn, target, atoms.leave, [source, 0, 0, 0, 0]);
        return false;
    }
    send(conn, target, atoms.drop, [source, 0, time, 0, 0]);
    // It asks for the list now (answered while waiting), then says it's done.
    let _ = wait_for(conn, atoms, atoms.finished, target, &list, Duration::from_secs(5));
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.selection, CURRENT_TIME);
    let _ = conn.flush();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_addresses() {
        assert_eq!(file_uri(Path::new("/home/you/Pictures/cat.png")), "file:///home/you/Pictures/cat.png");
        assert_eq!(file_uri(Path::new("/home/you/My Pics/ü #1.jpg")), "file:///home/you/My%20Pics/%C3%BC%20%231.jpg");
        assert_eq!(uri_list(&[PathBuf::from("/a"), PathBuf::from("/b c")]), "file:///a\r\nfile:///b%20c\r\n");
        assert_eq!(percent_decode("/home/you/My%20Pics/%C3%BC%20%231.jpg"), "/home/you/My Pics/ü #1.jpg");
        assert_eq!(percent_decode("/50%"), "/50%");
    }
}
