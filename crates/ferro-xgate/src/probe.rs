//! `ferro-xgate probe`: tries, on $DISPLAY, the ways a program could see
//! the screen or fake input, and prints what each got: `seen` (the
//! screen's pixels came back), `black` (they didn't), `hidden` / `visible`
//! for the extensions programs shouldn't know of. For the tests, and for
//! anyone who'd like to see the gate working.

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
