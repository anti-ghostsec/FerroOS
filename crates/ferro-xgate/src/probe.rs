//! `ferro-xgate probe`: tries, on $DISPLAY, the ways a program could see
//! the screen or fake input, and prints what each got: `seen` (the
//! screen's pixels came back), `black` (they didn't), `hidden` / `visible`
//! for the extensions programs shouldn't know of. For the tests, and for
//! anyone who'd like to see the gate working.
//!
//! `ferro-xgate probe tricks` tries the ways round the gate it has been
//! found to need closing: each line says what the trick got. Behind a gate
//! every one of them comes to nothing, without a question being asked
//! (answer "Don't allow" if one is: the trick then got `black`).

use x11rb::connection::Connection;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

const SIZE: u16 = 16;

fn verdict(data: &[u8]) -> &'static str {
    // Only the colour bytes: in 32 bits a pixel the fourth is padding.
    if data.chunks(4).any(|p| p.iter().take(3).any(|&b| b != 0)) {
        "seen"
    } else {
        "black"
    }
}

fn get_image(conn: &RustConnection, root: Window) -> Res<&'static str> {
    let r = conn.get_image(ImageFormat::Z_PIXMAP, root, 0, 0, SIZE, SIZE, !0)?.reply()?;
    Ok(verdict(&r.data))
}

/// The colour where the question was (the middle, a third of the way down):
/// once allowed, the screen there, not the question.
fn middle(conn: &RustConnection, screen: &Screen) -> Res<String> {
    let (x, y) = (screen.width_in_pixels / 2, screen.height_in_pixels * 2 / 5);
    let r = conn.get_image(ImageFormat::Z_PIXMAP, screen.root, x as i16, y as i16, 1, 1, !0)?.reply()?;
    let p = r.data.get(..3).ok_or("no pixel")?;
    Ok(format!("{:02x}{:02x}{:02x}", p[2], p[1], p[0]))
}

fn copy_area(conn: &RustConnection, screen: &Screen) -> Res<&'static str> {
    let (pix, gc) = (conn.generate_id()?, conn.generate_id()?);
    conn.create_pixmap(screen.root_depth, pix, screen.root, SIZE, SIZE)?;
    conn.create_gc(gc, pix, &CreateGCAux::new().foreground(0).subwindow_mode(SubwindowMode::INCLUDE_INFERIORS))?;
    conn.poly_fill_rectangle(pix, gc, &[Rectangle { x: 0, y: 0, width: SIZE, height: SIZE }])?;
    conn.copy_area(screen.root, pix, gc, 0, 0, 0, 0, SIZE, SIZE)?;
    let r = conn.get_image(ImageFormat::Z_PIXMAP, pix, 0, 0, SIZE, SIZE, !0)?.reply()?;
    conn.free_pixmap(pix)?;
    conn.free_gc(gc)?;
    Ok(verdict(&r.data))
}

fn shm_get_image(conn: &RustConnection, root: Window) -> Res<&'static str> {
    let bytes = usize::from(SIZE) * usize::from(SIZE) * 4;
    // SAFETY: a fresh private segment, attached here and removed at once (it
    // goes when both this process and the display have let go).
    let (id, mem) = unsafe {
        let id = libc::shmget(libc::IPC_PRIVATE, bytes, libc::IPC_CREAT | 0o600);
        if id < 0 {
            return Err("no shared memory".into());
        }
        let mem = libc::shmat(id, std::ptr::null(), 0);
        libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
        if mem as isize == -1 {
            return Err("no shared memory".into());
        }
        (id, mem.cast::<u8>())
    };
    let seg = conn.generate_id()?;
    conn.shm_attach(seg, id as u32, false)?;
    let r = conn.shm_get_image(root, 0, 0, SIZE, SIZE, !0, ImageFormat::Z_PIXMAP.into(), seg, 0)?.reply();
    // SAFETY: `mem` is `bytes` long, attached above.
    let data = unsafe { std::slice::from_raw_parts(mem, bytes) }.to_vec();
    conn.shm_detach(seg)?;
    // SAFETY: attached above.
    unsafe { libc::shmdt(mem.cast()) };
    r?;
    Ok(verdict(&data))
}

fn extension(conn: &RustConnection, name: &str) -> Res<&'static str> {
    Ok(if conn.query_extension(name.as_bytes())?.reply()?.present { "visible" } else { "hidden" })
}

/// Calls the screen (or its own window, which shows what's behind it) a
/// pixmap of its own, then reads "its pixmap".
fn claimed_pixmap(conn: &RustConnection, screen: &Screen, own_window: bool) -> Res<&'static str> {
    let id = if own_window {
        let win = conn.generate_id()?;
        let values = CreateWindowAux::new().override_redirect(1);
        conn.create_window(0, win, screen.root, 0, 0, SIZE, SIZE, 0, WindowClass::INPUT_OUTPUT, 0, &values)?;
        conn.map_window(win)?;
        win
    } else {
        screen.root
    };
    conn.create_pixmap(screen.root_depth, id, screen.root, SIZE, SIZE)?;
    let r = conn.get_image(ImageFormat::Z_PIXMAP, id, 0, 0, SIZE, SIZE, !0)?.reply();
    if own_window {
        conn.destroy_window(id)?;
    }
    Ok(verdict(&r?.data))
}

/// A picture of the screen made on a second connection, drawn through
/// this one into a pixmap of its own.
fn picture_from_elsewhere(conn: &RustConnection, screen: &Screen) -> Res<&'static str> {
    use x11rb::protocol::render::{ConnectionExt as _, CreatePictureAux, PictOp};
    let (other, _) = RustConnection::connect(None)?;
    let formats = conn.render_query_pict_formats()?.reply()?;
    let visual = formats
        .screens
        .iter()
        .flat_map(|s| &s.depths)
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual == screen.root_visual)
        .ok_or("no picture format")?;
    let theirs = other.generate_id()?;
    other.render_create_picture(theirs, screen.root, visual.format, &CreatePictureAux::new().subwindowmode(SubwindowMode::INCLUDE_INFERIORS))?;
    other.get_input_focus()?.reply()?;
    let (pix, mine) = (conn.generate_id()?, conn.generate_id()?);
    conn.create_pixmap(screen.root_depth, pix, screen.root, SIZE, SIZE)?;
    conn.render_create_picture(mine, pix, visual.format, &CreatePictureAux::new())?;
    conn.render_composite(PictOp::SRC, theirs, 0u32, mine, 0, 0, 0, 0, 0, 0, SIZE, SIZE)?;
    let r = conn.get_image(ImageFormat::Z_PIXMAP, pix, 0, 0, SIZE, SIZE, !0)?.reply()?;
    Ok(verdict(&r.data))
}

/// The screen copied through this connection into a second one's pixmap,
/// which that one reads as its own.
fn copy_to_elsewhere(conn: &RustConnection, screen: &Screen) -> Res<&'static str> {
    let (other, _) = RustConnection::connect(None)?;
    let (pix, gc) = (other.generate_id()?, conn.generate_id()?);
    other.create_pixmap(screen.root_depth, pix, screen.root, SIZE, SIZE)?;
    other.get_input_focus()?.reply()?;
    conn.create_gc(gc, screen.root, &CreateGCAux::new().subwindow_mode(SubwindowMode::INCLUDE_INFERIORS))?;
    conn.copy_area(screen.root, pix, gc, 0, 0, 0, 0, SIZE, SIZE)?;
    conn.get_input_focus()?.reply()?;
    let r = other.get_image(ImageFormat::Z_PIXMAP, pix, 0, 0, SIZE, SIZE, !0)?.reply()?;
    Ok(verdict(&r.data))
}

/// Says the screen is the desktop's background picture (which anyone may
/// read), then reads "the background"; and that its own window is the one
/// in use.
fn desktop_properties(conn: &RustConnection, screen: &Screen) -> Res<String> {
    use x11rb::wrapper::ConnectionExt as _;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (background, active) = (atom(b"_XROOTPMAP_ID")?, atom(b"_NET_ACTIVE_WINDOW")?);
    let read = |a: Atom| -> Res<u32> {
        Ok(conn.get_property(false, screen.root, a, AtomEnum::ANY, 0, 1)?.reply()?.value32().and_then(|mut v| v.next()).unwrap_or(0))
    };
    let before = (read(background)?, read(active)?);
    let win = conn.generate_id()?;
    conn.create_window(0, win, screen.root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
    conn.change_property32(PropMode::REPLACE, screen.root, background, AtomEnum::PIXMAP, &[screen.root])?;
    conn.change_property32(PropMode::REPLACE, screen.root, active, AtomEnum::WINDOW, &[win])?;
    // The gate hears of the change before the next request is looked at.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let after = (read(background)?, read(active)?);
    let seen = get_image(conn, screen.root);
    // Put back as they were (where it was allowed to change them).
    if after.0 == screen.root {
        if before.0 == 0 {
            conn.delete_property(screen.root, background)?;
        } else {
            conn.change_property32(PropMode::REPLACE, screen.root, background, AtomEnum::PIXMAP, &[before.0])?;
        }
    }
    if after.1 == win {
        conn.change_property32(PropMode::REPLACE, screen.root, active, AtomEnum::WINDOW, &[before.1])?;
    }
    conn.get_input_focus()?.reply()?;
    let said = |changed: bool| if changed { "changed" } else { "refused" };
    Ok(format!("background {}, the screen as it {}, window in use {}", said(after.0 == screen.root), seen?, said(after.1 == win)))
}

/// Asks for every key press on the desktop and in every window on it, and
/// grabs the keys typed with; then listens for `secs` seconds and counts
/// what arrives and which keys are seen held down.
fn listen_for_keys(conn: &RustConnection, screen: &Screen, secs: u64) -> Res<String> {
    use x11rb::protocol::xinput::{self, ConnectionExt as _};
    use x11rb::protocol::Event;
    let keys = EventMask::KEY_PRESS | EventMask::KEY_RELEASE;
    let mut windows = vec![screen.root];
    let mut at = 0;
    while at < windows.len() && windows.len() < 500 {
        if let Ok(tree) = conn.query_tree(windows[at])?.reply() {
            windows.extend(tree.children);
        }
        at += 1;
    }
    for &w in &windows {
        conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().event_mask(keys | EventMask::KEYMAP_STATE))?;
    }
    // A, with nothing held and with anything held.
    conn.grab_key(true, screen.root, ModMask::from(0u16), 38, GrabMode::ASYNC, GrabMode::ASYNC)?;
    conn.grab_key(true, screen.root, ModMask::ANY, 38, GrabMode::ASYNC, GrabMode::ASYNC)?;
    let selected = conn.get_window_attributes(screen.root)?.reply()?.your_event_mask.contains(EventMask::KEY_PRESS);
    // XInput 2: every key of every keyboard (XIAnyKeycode, XIAnyModifier), and the raw key events.
    let mut every_key = "no XInput";
    if conn.xinput_xi_query_version(2, 0)?.reply().is_ok() {
        let mask = [u32::from(xinput::XIEventMask::KEY_PRESS | xinput::XIEventMask::KEY_RELEASE)];
        let grab = conn.xinput_xi_passive_grab_device(
            0u32,
            screen.root,
            0,
            0,
            3u16,
            xinput::GrabType::KEYCODE,
            xinput::GrabMode22::ASYNC,
            GrabMode::ASYNC,
            xinput::GrabOwner::NO_OWNER,
            &mask,
            &[1 << 31],
        )?;
        every_key = match grab.reply() {
            Ok(r) if r.modifiers.is_empty() => "grabbed",
            _ => "refused",
        };
        let raw = xinput::EventMask { deviceid: 0, mask: vec![xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::KEY_PRESS] };
        conn.xinput_xi_select_events(screen.root, &[raw])?;
    }
    conn.flush()?;
    let (mut heard, mut down) = (0, false);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::KeyPress(_) | Event::XinputKeyPress(_) | Event::XinputRawKeyPress(_) => heard += 1,
                Event::KeymapNotify(k) => down |= k.keys.iter().any(|&b| b != 0),
                _ => {}
            }
        }
        down |= conn.query_keymap()?.reply()?.keys.iter().any(|&b| b != 0);
        if let Ok(devices) = conn.xinput_list_input_devices()?.reply() {
            for d in devices.devices {
                if let Ok(state) = conn.xinput_query_device_state(d.device_id)?.reply() {
                    down |= state.classes.iter().any(|c| matches!(&c.data, xinput::InputStateData::Key(k) if k.keys.iter().any(|&b| b != 0)));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(format!(
        "key presses on the desktop {}, every key {every_key}, {heard} heard, keys held {}",
        if selected { "selected" } else { "refused" },
        if down { "seen" } else { "not seen" }
    ))
}

/// A request with a length of 0, which only means "a long request" once
/// that's been turned on: sent without, the display reads the bytes after
/// it as requests of their own, a GetImage of the screen among them.
fn hidden_request(root: Window) -> Res<&'static str> {
    use std::io::{Read, Write};
    let display = std::env::var("DISPLAY")?;
    let number = crate::run::display_number(&display).ok_or("not a display on this computer")?;
    let mut s = std::os::unix::net::UnixStream::connect(format!("/tmp/.X11-unix/X{number}"))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    s.write_all(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
    let mut head = [0u8; 8];
    s.read_exact(&mut head)?;
    if head[0] != 1 {
        return Err("no connection without a cookie (not a gate?)".into());
    }
    let mut rest = vec![0u8; 4 * usize::from(u16::from_le_bytes([head[6], head[7]]))];
    s.read_exact(&mut rest)?;
    // "NoOperation, 7 units long with the long header", holding a GetImage.
    let mut b = vec![127u8, 0, 0, 0, 7, 0, 0, 0, 73, 2, 5, 0];
    b.extend_from_slice(&root.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&SIZE.to_le_bytes());
    b.extend_from_slice(&SIZE.to_le_bytes());
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    // Then something every display answers: the end of what's read here.
    b.extend_from_slice(&[43, 0, 1, 0]);
    s.write_all(&b)?;
    let mut packet = [0u8; 32];
    loop {
        if s.read_exact(&mut packet).is_err() {
            return Ok("connection closed");
        }
        if packet[0] != 1 {
            continue;
        }
        let mut data = vec![0u8; 4 * u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]) as usize];
        if data.is_empty() {
            return Ok("ignored");
        }
        s.read_exact(&mut data)?;
        return Ok(verdict(&data));
    }
}

/// `probe tricks [SECONDS]`: see the top of this file.
pub fn tricks(secs: u64) -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let show = |name: &str, r: Res<String>| match r {
        Ok(v) => println!("{name}: {v}"),
        Err(e) => println!("{name}: error ({e})"),
    };
    show("screen-as-pixmap", claimed_pixmap(&conn, &screen, false).map(String::from));
    show("window-as-pixmap", claimed_pixmap(&conn, &screen, true).map(String::from));
    show("picture-from-elsewhere", picture_from_elsewhere(&conn, &screen).map(String::from));
    show("copy-to-elsewhere", copy_to_elsewhere(&conn, &screen).map(String::from));
    show("desktop-properties", desktop_properties(&conn, &screen));
    show("hidden-request", hidden_request(screen.root).map(String::from));
    show("keys", listen_for_keys(&conn, &screen, secs));
    Ok(())
}

pub fn run(hold: bool) -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let show = |name: &str, r: Res<&'static str>| match r {
        Ok(v) => println!("{name}: {v}"),
        Err(e) => println!("{name}: error ({e})"),
    };
    show("getimage", get_image(&conn, screen.root));
    match middle(&conn, &screen) {
        Ok(c) => println!("middle: {c}"),
        Err(e) => println!("middle: error ({e})"),
    }
    show("copyarea", copy_area(&conn, &screen));
    show("shmgetimage", shm_get_image(&conn, screen.root));
    show("xtest", extension(&conn, "XTEST"));
    show("record", extension(&conn, "RECORD"));
    if hold {
        // Still running: still allowed, if it was until it quits.
        std::thread::sleep(std::time::Duration::from_secs(6));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn padding_isnt_a_pixel() {
        assert_eq!(super::verdict(&[0, 0, 0, 0xff, 0, 0, 0, 0xff]), "black");
        assert_eq!(super::verdict(&[0, 0, 0x80, 0]), "seen");
    }
}
