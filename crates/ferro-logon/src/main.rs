//! ferro-logon: FerroOS Heavy's logon screen. The Windows 95 box asking for
//! a name and a password, full screen, before anyone's desktop starts.
//!
//! It only asks and checks. Checking is done by the program named in its
//! first argument, which gets the name and the password on two lines of its
//! standard input and says yes by exiting with 0 (on Heavy that's a small
//! script comparing against the system's own password file). On a yes, the
//! name is written to the file named in the second argument and this exits;
//! whoever started it (heavy's `logon` script) starts that person's desktop.
//!
//!     ferro-logon <checker> <file for the name>
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::io::Write;
use std::process::{Command, Stdio};
use view::{Field, Hit, Look};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// Asks the checker whether `password` is `name`'s.
fn check(checker: &str, name: &str, password: &str) -> bool {
    let Ok(mut child) = Command::new(checker).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{name}\n{password}");
    }
    child.wait().is_ok_and(|s| s.success())
}

struct Screen {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    w: i32,
    h: i32,
    keys: Keymap,
    name: String,
    password: String,
    focus: Field,
    message: &'static str,
    desktop_color: Option<u32>,
    wallpaper: Option<ferro_gfx::DecodedImage>,
}

fn load_appearance() -> (Option<u32>, Option<ferro_gfx::DecodedImage>) {
    let content = std::fs::read_to_string("/etc/ferro/appearance.conf")
        .or_else(|_| {
            std::env::var("HOME")
                .map(|h| format!("{h}/.config/ferro/appearance.conf"))
                .and_then(|p| std::fs::read_to_string(p).map_err(|_e| std::env::VarError::NotPresent))
        })
        .ok();
    let mut color = None;
    let mut wp_path = None;
    if let Some(content) = content {
        for line in content.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim();
                if key == "desktop_color" || key == "color" {
                    let hex = val.strip_prefix("0x").or_else(|| val.strip_prefix('#')).unwrap_or(val);
                    if let Ok(c) = u32::from_str_radix(hex, 16) {
                        color = Some(c & 0x00ffffff);
                    }
                } else if (key == "logon_wallpaper" || key == "wallpaper") && !val.is_empty() {
                    wp_path = Some(val.to_string());
                }
            }
        }
    }
    let img = wp_path.as_deref().and_then(ferro_gfx::DecodedImage::load_from_file);
    (color, img)
}

impl Screen {
    fn paint(&self) -> Res<()> {
        let (w, h) = (self.w, self.h);
        let mut s = Surface::new(w as usize, h as usize);
        view::draw(
            &mut s,
            w,
            h,
            &Look {
                name: &self.name,
                password_len: self.password.chars().count(),
                focus: Some(self.focus),
                message: self.message,
                desktop_color: self.desktop_color,
                wallpaper: self.wallpaper.as_ref(),
            },
        );
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, w, h)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [checker, name_file] = args.as_slice() else {
        eprintln!("usage: ferro-logon <checker> <file for the name>");
        std::process::exit(2);
    };
    match run(checker, name_file) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("ferro-logon: {e}");
            std::process::exit(1);
        }
    }
}

fn run(checker: &str, name_file: &str) -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let (w, h) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::VISIBILITY_CHANGE;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        0,
        0,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        // There's no window manager yet: nothing should try to frame this.
        &CreateWindowAux::new().override_redirect(1).event_mask(events),
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME)?;
    // As the lock screen, the desktop is right underneath: without the
    // keyboard to itself, what's typed could reach a window there.
    if !ferro_ui::grab_input(&conn, win, EventMask::BUTTON_PRESS)? {
        return Err("couldn't take the keyboard and mouse".into());
    }

    let keys = Keymap::load(&conn)?;
    let (desktop_color, wallpaper) = load_appearance();
    let mut s = Screen {
        win,
        gc,
        depth: screen.root_depth,
        w,
        h,
        keys,
        name: String::new(),
        password: String::new(),
        focus: Field::Name,
        message: "",
        desktop_color,
        wallpaper,
        conn,
    };
    s.paint()?;
    loop {
        let mut submit = false;
        match s.conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => s.paint()?,
            Event::VisibilityNotify(e) if e.state != Visibility::UNOBSCURED => ferro_ui::raise(&s.conn, s.win)?,
            Event::ButtonPress(e) if e.detail == 1 => match view::hit(s.w, s.h, i32::from(e.event_x), i32::from(e.event_y)) {
                Hit::Field(f) => s.focus = f,
                Hit::Ok => submit = true,
                Hit::ShutDown => {
                    let _ = Command::new("systemctl").arg("poweroff").status();
                }
                Hit::Nothing => {}
            },
            Event::KeyPress(e) => match s.keys.typed(&e) {
                Key::Char(c) => {
                    s.message = "";
                    match s.focus {
                        Field::Name if s.name.chars().count() < 32 => s.name.push(c),
                        Field::Password if s.password.chars().count() < 128 => s.password.push(c),
                        _ => {}
                    }
                }
                Key::Backspace => {
                    match s.focus {
                        Field::Name => s.name.pop(),
                        Field::Password => s.password.pop(),
                    };
                }
                Key::Next => s.focus = if s.focus == Field::Name { Field::Password } else { Field::Name },
                // Enter on the name moves on to the password, as in Windows.
                Key::Enter if s.focus == Field::Name => s.focus = Field::Password,
                // Enter on the password is OK. There's no Cancel, so Esc does nothing.
                k if ferro_ui::dialog_answer(k) == Some(true) => submit = true,
                _ => {}
            },
            _ => continue,
        }
        if submit {
            let ok = !s.name.is_empty() && check(checker, &s.name, &s.password);
            // The password isn't kept a moment longer than the check.
            ferro_ui::forget(&mut s.password);
            if ok {
                std::fs::write(name_file, &s.name)?;
                return Ok(());
            }
            s.message = "The name or the password isn't right.";
            s.focus = Field::Password;
            s.paint()?;
            ferro_ui::sound("error");
            // Slow down guessing.
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        s.paint()?;
    }
}
