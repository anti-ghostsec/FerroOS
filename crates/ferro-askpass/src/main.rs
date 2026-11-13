//! ferro-askpass: the "Run as administrator" password box.
//!
//! sudo runs it (`sudo -A`, with `SUDO_ASKPASS` pointing here) when a
//! program needs the administrator's say-so and there's no terminal to ask
//! in: a shortcut "as Administrator" (through FerroOS's `pkexec`), Restricted
//! folders, Task Manager ending someone else's process. It prints what's
//! typed on one line for sudo and exits 0; Cancel, Esc or closing it exits 1,
//! and sudo then gives up.
//!
//! `FERRO_ADMIN_FOR` (set by whoever asks) names what wants the rights, so
//! the box can say so; without it, sudo's own prompt is shown. Whoever asks
//! can put anything there, so under it the box always shows the command
//! sudo is about to run as administrator, read from sudo itself (the
//! program that started this box): that one can't be made to say
//! something else.

use ferro_gfx::{color, ellipsize, Rect, Surface, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, forget, text_field_with_cursor, Key, Keymap, Res};
use std::io::Write;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 400;
const H: i32 = 170;
const FIELD: Rect = Rect::new(14, 90, W - 28, 24);
const OK_BTN: Rect = Rect::new(W - 175, 130, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 90, 130, 75, 24);

/// The lines over the password field.
fn lines(asking_for: Option<&str>, sudo_prompt: &str, command: Option<&str>) -> [String; 3] {
    let first = match asking_for.map(str::trim).filter(|s| !s.is_empty()) {
        Some(what) => format!("'{what}' wants to make changes to this computer."),
        None => sudo_prompt.trim().trim_end_matches(':').to_owned(),
    };
    let runs = match command {
        Some(c) => format!("It runs: {c}"),
        None => "What it runs isn't known.".into(),
    };
    [first, runs, "Type your password to allow it.".into()]
}

/// The command a sudo command line runs: what follows sudo's own options.
fn sudo_command(argv: &[String]) -> Option<String> {
    // sudo's options that take a value as the next word.
    const VALUE: &[&str] = &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U", "-T", "-R"];
    let mut rest = argv.iter().skip(1);
    let mut words = Vec::new();
    while let Some(a) = rest.next() {
        if a == "--" {
            break;
        }
        if VALUE.contains(&a.as_str()) {
            rest.next();
        } else if !a.starts_with('-') || a == "-" {
            words.push(a.clone());
            break;
        }
    }
    words.extend(rest.cloned());
    // sudo VAR=value ... : settings before the command, not the command.
    let words: Vec<String> = words.into_iter().skip_while(|w| is_setting(w)).collect();
    (!words.is_empty()).then(|| words.iter().map(|w| shown(w)).collect::<Vec<_>>().join(" "))
}

/// VAR=value.
fn is_setting(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// One word as it would be typed: quoted if it has spaces or odd letters,
/// with anything unprintable shown as '?', so nothing can hide in it.
fn shown(w: &str) -> String {
    let w: String = w.chars().map(|c| if c.is_control() { '?' } else { c }).collect();
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:,=@%".contains(c)) {
        w
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// What the sudo that started this box is about to run, if sudo did.
fn parent_command() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).ok()?;
    if comm.trim() != "sudo" {
        return None;
    }
    let raw = std::fs::read(format!("/proc/{ppid}/cmdline")).ok()?;
    let argv: Vec<String> = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    sudo_command(&argv)
}

/// What the field shows: a star for each letter typed.
fn masked(typed: &str) -> String {
    "*".repeat(typed.chars().count())
}

fn draw(s: &mut Surface, text: &[String; 3], typed: &str) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let room = ((W - 28) / GLYPH_W).max(1) as usize;
    s.text(14, 16, &ellipsize(&text[0], room), color::BLACK);
    s.text(14, 36, &ellipsize(&text[1], room), color::BLACK);
    s.text(14, 56, &ellipsize(&text[2], room), color::BLACK);
    let shown = masked(typed);
    text_field_with_cursor(s, FIELD, &shown, shown.chars().count(), None, true);
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
}

fn answer(mut typed: String) -> ! {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{typed}");
    let _ = out.flush();
    forget(&mut typed);
    std::process::exit(0)
}

fn main() -> Res<()> {
    let sudo_prompt = std::env::args().nth(1).unwrap_or_else(|| "Password:".into());
    let asking_for = std::env::var("FERRO_ADMIN_FOR").ok();
    let text = lines(asking_for.as_deref(), &sudo_prompt, parent_command().as_deref());
    let mut typed = String::new();

    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values = CreateWindowAux::new()
        .background_pixel(color::FACE)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY);
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - H) / 2) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let (wtype, dialog) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?);
    let (state, above) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?);
    let title = "Run as administrator";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[dialog])?;
    conn.change_property32(PropMode::REPLACE, win, state, AtomEnum::ATOM, &[above])?;
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut surface = Surface::new(W as usize, H as usize);
    let mut repaint = |typed: &str| -> Res<()> {
        draw(&mut surface, &text, typed);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &surface, W, H)
    };
    repaint(&typed)?;

    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => repaint(&typed)?,
            Event::MapNotify(_) => {
                // Typing goes here at once, without a click first.
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == delete => std::process::exit(1),
            Event::ButtonPress(bp) => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if OK_BTN.contains(x, y) {
                    answer(typed);
                } else if CANCEL_BTN.contains(x, y) {
                    forget(&mut typed);
                    std::process::exit(1);
                }
            }
            Event::KeyPress(kp) => {
                let key = keymap.typed(&kp);
                match dialog_answer(key) {
                    Some(true) => answer(typed),
                    Some(false) => {
                        forget(&mut typed);
                        std::process::exit(1);
                    }
                    None => {}
                }
                match key {
                    Key::Backspace => {
                        typed.pop();
                    }
                    Key::Char(c) if !c.is_control() && typed.len() < 512 => typed.push(c),
                    _ => continue,
                }
                repaint(&typed)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_wants_the_rights() {
        let [first, runs, _] = lines(Some("bleachbit"), "[sudo] password for me: ", Some("/usr/bin/bleachbit"));
        assert_eq!(first, "'bleachbit' wants to make changes to this computer.");
        assert_eq!(runs, "It runs: /usr/bin/bleachbit");
        let [first, runs, _] = lines(None, "[sudo] password for me: ", None);
        assert_eq!(first, "[sudo] password for me");
        assert_eq!(runs, "What it runs isn't known.");
        let [first, _, _] = lines(Some("  "), "Password:", None);
        assert_eq!(first, "Password");
    }

    #[test]
    fn shows_what_sudo_runs_whatever_the_label_says() {
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(sudo_command(&argv("sudo -A /usr/local/sbin/ferro-tor off")).as_deref(), Some("/usr/local/sbin/ferro-tor off"));
        assert_eq!(sudo_command(&argv("sudo -A -u root -- rm -rf /x")).as_deref(), Some("rm -rf /x"));
        assert_eq!(sudo_command(&argv("sudo -A HOME=/x sh")).as_deref(), Some("sh"));
        assert_eq!(sudo_command(&argv("sudo -A env A=b sh")).as_deref(), Some("env A=b sh"));
        let v = vec!["sudo".into(), "-A".into(), "sh".into(), "-c".into(), "curl x | sh".into()];
        assert_eq!(sudo_command(&v).as_deref(), Some("sh -c 'curl x | sh'"));
        assert_eq!(shown("a\nb"), "'a?b'");
        assert_eq!(shown("it's"), "'it'\\''s'");
        assert_eq!(sudo_command(&argv("sudo -A")), None);
    }

    #[test]
    fn shows_only_stars_and_forgets() {
        assert_eq!(masked("pässwörd"), "********");
    }
}
