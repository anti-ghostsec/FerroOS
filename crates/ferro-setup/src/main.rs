//! ferro-setup: FerroOS Heavy's installer window. Asks which disk, a
//! passphrase for it and a name and password for your account, makes sure
//! you mean it, then runs `ferro-install` and shows how far it is.
//!
//! It decides nothing about how installing is done: `ferro-install` (the
//! script in heavy/) does all of it, and can still be run by itself in a
//! Command Prompt. This window only answers its questions, through its
//! standard input, so no passphrase is ever in a command line.
//!
//! Everything works from the keyboard too: Tab moves between the boxes,
//! the arrows choose a disk, Space ticks, Enter goes on, Esc leaves.
//!
//! Like ferro-wm it speaks the X11 protocol itself: one static binary.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use view::{Hit, Page, State};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const INSTALLER: &str = "/usr/local/bin/ferro-install";

/// What the running installer reports.
enum Progress {
    Line(String),
    /// It ended; `true` if it worked.
    Ended(bool),
}

fn output(program: &str, args: &[&str]) -> String {
    Command::new(program).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// The disks FerroOS Heavy could go on.
fn disks() -> Vec<view::Disk> {
    // Not loop devices (7) or CD drives (11).
    let all = output("lsblk", &["-d", "-n", "-b", "-o", "NAME,SIZE,MODEL", "-e", "7,11"]);
    let medium = output("findmnt", &["-n", "-o", "SOURCE", "/run/live/medium"]);
    view::parse_disks(&all, &medium)
}

/// Starts `ferro-install` with the window's answers. Its output comes
/// back line by line, then how it ended.
fn install(answers: String) -> Receiver<Progress> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // sudo -n: it never stops to ask for a password nobody would see.
        let child = Command::new("sh").args(["-c", &format!("exec sudo -n {INSTALLER} 2>&1")]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Progress::Line(format!("could not start the installer: {e}")));
                let _ = tx.send(Progress::Ended(false));
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(answers.as_bytes());
        }
        drop(answers);
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(Progress::Line(line));
            }
        }
        let _ = tx.send(Progress::Ended(child.wait().is_ok_and(|s| s.success())));
    });
    rx
}

struct Window {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    w: i32,
    h: i32,
    state: State,
    /// The installer, while it runs.
    running: Option<Receiver<Progress>>,
    /// The last thing it said that wasn't a step: why it stopped, if it does.
    last_line: String,
}

impl Window {
    fn paint(&self) -> Res<()> {
        let mut s = Surface::new(self.w as usize, self.h as usize);
        view::draw(&mut s, self.w, self.h, &self.state);
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, &s, self.w, self.h)
    }

    /// The forward button. `false` means the window is finished with.
    fn next(&mut self) -> bool {
        let s = &mut self.state;
        if !view::can_go_on(s) {
            return true;
        }
        match s.page {
            Page::Disk => s.page = Page::Account,
            Page::Account => match view::check_account(&s.fields, s.encrypt) {
                Ok(()) => {
                    s.message = "";
                    s.sure = false;
                    s.page = Page::Confirm;
                }
                Err(why) => s.message = why,
            },
            Page::Confirm => {
                self.running = Some(install(s.answers()));
                s.forget();
                s.step = 0;
                s.page = Page::Working;
            }
            Page::Done => {
                let _ = Command::new("systemctl").arg("reboot").status();
                return false;
            }
            Page::Working | Page::Failed => {}
        }
        true
    }

    fn back(&mut self) {
        let s = &mut self.state;
        s.page = match s.page {
            Page::Account => Page::Disk,
            Page::Confirm => Page::Account,
            other => other,
        };
        s.message = "";
    }

    /// Leaving is always possible, except in the middle of installing.
    fn may_leave(&self) -> bool {
        self.state.page != Page::Working
    }

    /// `false` means the window is finished with.
    fn key(&mut self, key: Key) -> bool {
        let s = &mut self.state;
        match (s.page, key) {
            (_, Key::Escape) => return !self.may_leave(),
            (Page::Disk, Key::Up) => s.disk = s.disk.saturating_sub(1),
            (Page::Disk, Key::Down) => s.disk = (s.disk + 1).min(s.disks.len().saturating_sub(1)),
            (Page::Account, Key::Char(c)) if s.focus < view::FIELDS => {
                s.message = "";
                if s.fields[s.focus].chars().count() < 128 {
                    s.fields[s.focus].push(c);
                }
            }
            (Page::Account, Key::Backspace) if s.focus < view::FIELDS => {
                s.fields[s.focus].pop();
            }
            // Space ticks the checkbox when the keyboard is on it.
            (Page::Account, Key::Char(' ')) if s.focus == view::ENCRYPT => toggle_encrypt(s),
            (Page::Account, Key::Next | Key::Down) => s.focus = account_stop(s, true),
            (Page::Account, Key::Up) => s.focus = account_stop(s, false),
            // Enter moves down the boxes, and goes on from the last one.
            (Page::Account, Key::Enter) if s.focus == view::ENCRYPT || s.focus + 1 < view::FIELDS => s.focus = account_stop(s, true),
            (Page::Confirm, Key::Char(' ')) => confirm_key(s, key),
            (Page::Confirm, Key::Enter) if !s.sure => confirm_key(s, key),
            (Page::Failed, Key::Enter) => return false,
            (_, Key::Enter) => return self.next(),
            _ => {}
        }
        true
    }

    /// `false` means the window is finished with.
    fn click(&mut self, x: i32, y: i32) -> bool {
        match view::hit(&self.state, x, y) {
            Hit::Disk(i) => self.state.disk = i,
            Hit::Field(i) => self.state.focus = i,
            Hit::EncryptToggle => toggle_encrypt(&mut self.state),
            Hit::Sure => {
                self.state.sure = !self.state.sure;
                self.state.message = "";
            }
            Hit::Back => self.back(),
            Hit::Next => return self.next(),
            Hit::Cancel => return false,
            Hit::Nothing => {}
        }
        true
    }

    /// Takes in what the installer has said since last time. `true` if the
    /// window needs drawing again.
    fn progress(&mut self) -> bool {
        let Some(rx) = &self.running else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(Progress::Line(line)) => match view::step_of(&line) {
                    Some(step) => {
                        self.state.step = step;
                        changed = true;
                    }
                    // Its questions are on the same line as what follows
                    // them; only its own complaint is worth showing.
                    None if !line.trim().is_empty() => {
                        self.last_line = line.rfind("ferro-install: ").map_or(line.as_str(), |at| &line[at..]).to_owned();
                    }
                    None => {}
                },
                Ok(Progress::Ended(ok)) => {
                    self.state.page = if ok { Page::Done } else { Page::Failed };
                    self.state.error = std::mem::take(&mut self.last_line);
                    self.running = None;
                    return true;
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.state.page = Page::Failed;
                    self.state.error = "the installer stopped without saying why".into();
                    self.running = None;
                    return true;
                }
            }
        }
    }
}

/// Ticks or unticks "Encrypt disk": without encryption the passphrase
/// boxes are greyed out, so the keyboard moves off them.
fn toggle_encrypt(s: &mut view::State) {
    s.encrypt = !s.encrypt;
    if !s.encrypt && s.focus < view::NAME {
        s.focus = view::NAME;
    }
}

/// The next (or previous) stop in the Account page's Tab order: the
/// checkbox, then the boxes that are in use.
fn account_stop(s: &view::State, forward: bool) -> usize {
    let mut stops = vec![view::ENCRYPT];
    stops.extend(if s.encrypt { 0 } else { view::NAME }..view::FIELDS);
    let at = stops.iter().position(|&f| f == s.focus).unwrap_or(0);
    let n = stops.len();
    stops[if forward { (at + 1) % n } else { (at + n - 1) % n }]
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ferro-setup: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let mut state = State::new(disks());
    if !std::path::Path::new("/run/live/medium/live/filesystem.squashfs").exists() {
        state.page = Page::Failed;
        state.error = "This only runs from the FerroOS Heavy installation medium.".into();
    }

    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = conn.setup().roots[screen_num].clone();
    let win = conn.generate_id()?;
    let (w, h) = (view::W, view::H);
    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((i32::from(screen.width_in_pixels) - w) / 2).max(0) as i16,
        ((i32::from(screen.height_in_pixels) - h) / 3).max(0) as i16,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(events),
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?);
    let (net_wm_name, utf8_string) = (atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);
    // The window manager's Close button asks us to close, instead of
    // cutting the connection.
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    let title = "Install FerroOS Heavy";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, title.as_bytes())?;
    let keys = Keymap::load(&conn)?;
    conn.map_window(win)?;

    let mut window = Window { conn, win, gc, depth: screen.root_depth, w, h, state, running: None, last_line: String::new() };
    window.paint()?;
    loop {
        // While installing, nothing may be waited on for long: the
        // installer's news has to be shown as it comes.
        let event = if window.running.is_some() {
            match window.conn.poll_for_event()? {
                Some(e) => e,
                None => {
                    if window.progress() {
                        window.paint()?;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        } else {
            window.conn.wait_for_event()?
        };
        let open = match event {
            Event::Expose(e) if e.count == 0 => true,
            Event::ConfigureNotify(e) => {
                (window.w, window.h) = (i32::from(e.width).max(1), i32::from(e.height).max(1));
                true
            }
            Event::ButtonPress(e) if e.detail == 1 => window.click(i32::from(e.event_x), i32::from(e.event_y)),
            // Alt+B, Alt+N, Alt+C: the Back, Next and Cancel buttons.
            Event::KeyPress(e) if e.state.contains(KeyButMask::MOD1) => match keys.typed(&e) {
                Key::Char('b' | 'B') => {
                    window.back();
                    true
                }
                Key::Char('n' | 'N') => window.next(),
                Key::Char('c' | 'C') => !window.may_leave(),
                _ => continue,
            },
            Event::KeyPress(e) => window.key(keys.typed(&e)),
            Event::ClientMessage(e) if e.type_ == wm_protocols && e.data.as_data32()[0] == wm_delete => !window.may_leave(),
            _ => continue,
        };
        if !open {
            return Ok(());
        }
        window.paint()?;
    }
}

/// The last page, by keyboard: Space ticks "Yes, erase this disk" (or
/// unticks it). Install stays greyed out until it's ticked, so Enter
/// before then says why instead of doing nothing.
fn confirm_key(s: &mut State, key: Key) {
    match key {
        Key::Char(' ') => {
            s.sure = !s.sure;
            s.message = "";
        }
        Key::Enter if !s.sure => s.message = "Tick \"Yes, erase this disk\" first: press Space.",
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_erase_box_by_keyboard() {
        let mut s = State::new(Vec::new());
        s.page = Page::Confirm;
        confirm_key(&mut s, Key::Enter);
        assert!(!s.sure && s.message.contains("press Space"));
        confirm_key(&mut s, Key::Char(' '));
        assert!(s.sure && s.message.is_empty() && view::can_go_on(&s));
        confirm_key(&mut s, Key::Char(' '));
        assert!(!s.sure && !view::can_go_on(&s));
    }

    #[test]
    fn tab_order_has_the_checkbox_and_skips_greyed_boxes() {
        let mut s = State::new(Vec::new());
        s.focus = view::ENCRYPT;
        assert_eq!(account_stop(&s, true), view::PASSPHRASE);
        assert_eq!(account_stop(&s, false), view::PASSWORD_AGAIN);
        // Without encryption the passphrase boxes are skipped.
        toggle_encrypt(&mut s);
        assert_eq!(account_stop(&s, true), view::NAME);
        s.focus = view::NAME;
        assert_eq!(account_stop(&s, false), view::ENCRYPT);
    }
}
