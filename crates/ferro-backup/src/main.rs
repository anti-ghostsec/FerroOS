//! ferro-backup: your folders copied to a USB drive, as Windows' File
//! History does it. Only what's new or changed since the last time is
//! copied, and nothing on the drive is ever deleted, so a file deleted or
//! spoiled here is still there. The copies are in "FerroOS Backup" on the
//! drive, each folder under your name, to open and copy back like any other.
//!
//!     ferro-backup          the window: which folders, which drive, Back Up Now
//!     ferro-backup --auto   what ferro-automount runs when a drive is plugged
//!                           in: backs up if it's the drive chosen, and
//!                           "automatically" was ticked

mod copy;

use ferro_gfx::{color, ellipsize, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 460;
const H: i32 = 400;
const FOLDERS: [&str; 6] = ["Desktop", "Documents", "Pictures", "Music", "Videos", "Downloads"];

/// What was chosen last time: the folders, the drive (by its name), and
/// whether to back up by itself when that drive is plugged in.
#[derive(Debug, PartialEq, Eq)]
struct Choice {
    folders: Vec<String>,
    drive: Option<String>,
    auto: bool,
}

impl Choice {
    fn parse(text: &str) -> Self {
        let field = |k: &str| text.lines().find_map(|l| l.split_once('=').filter(|(key, _)| key.trim() == k).map(|(_, v)| v.trim().to_owned()));
        Choice {
            folders: field("folders").map_or_else(
                || FOLDERS[..5].iter().map(|f| (*f).to_string()).collect(),
                |f| f.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            ),
            drive: field("drive").filter(|d| !d.is_empty()),
            auto: field("auto").is_some_and(|a| a == "true"),
        }
    }

    fn to_text(&self) -> String {
        format!(
            "# FerroOS Backup: what to copy, and where\nfolders = {}\ndrive = {}\nauto = {}\n",
            self.folders.join(", "),
            self.drive.as_deref().unwrap_or(""),
            self.auto
        )
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn choice_path() -> PathBuf {
    home().join(".config/ferro/backup.conf")
}

fn load() -> Choice {
    Choice::parse(&std::fs::read_to_string(choice_path()).unwrap_or_default())
}

fn save(c: &Choice) {
    let p = choice_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, c.to_text());
}

/// The drives plugged in: (name, where it's mounted).
fn drives() -> Vec<(String, PathBuf)> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut out = Vec::new();
    for media in [Path::new("/media").join(&user), Path::new("/run/media").join(&user)] {
        for e in std::fs::read_dir(media).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                out.push((e.file_name().to_string_lossy().into_owned(), e.path()));
            }
        }
    }
    out.sort();
    out
}

/// Where the copies of `folder` go on a drive mounted at `drive`.
fn target(drive: &Path, folder: &str) -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    drive.join("FerroOS Backup").join(user).join(folder)
}

fn notify(summary: &str, body: &str) {
    let _ = std::process::Command::new("notify-send").args(["-a", "Backup", summary, body]).spawn();
}

/// Copies the chosen folders to the drive, reporting as it goes.
fn back_up(choice: &Choice, drive: &Path, progress: impl Fn(copy::Progress)) -> copy::Done {
    let mut total = copy::Done::default();
    for f in &choice.folders {
        let from = home().join(f);
        if from.is_dir() {
            total.add(copy::mirror(&from, &target(drive, f), &progress));
        }
    }
    total
}

fn main() -> Res<()> {
    if std::env::args().nth(1).as_deref() == Some("--auto") {
        let choice = load();
        if let Some((name, at)) = drives().into_iter().find(|(n, _)| choice.auto && Some(n) == choice.drive.as_ref()) {
            let done = back_up(&choice, &at, |_| {});
            notify(&format!("Backed up to {name}"), &done.said());
        }
        return Ok(());
    }
    window()
}

enum State {
    Idle(String),
    Busy(String),
}

fn check_rect(i: usize) -> Rect {
    Rect::new(24 + (i as i32 % 2) * 200, 40 + (i as i32 / 2) * 24, 190, 18)
}

fn drive_rect(i: usize) -> Rect {
    Rect::new(24, 160 + i as i32 * 24, W - 48, 20)
}

fn auto_rect() -> Rect {
    Rect::new(24, H - 104, W - 48, 18)
}

fn go_button() -> Rect {
    Rect::new(W - 150, H - 40, 140, 28)
}

fn open_button() -> Rect {
    Rect::new(10, H - 40, 140, 28)
}

fn checkbox(s: &mut Surface, r: Rect, label: &str, on: bool) {
    let b = Rect::new(r.x, r.y + (r.h - 13) / 2, 13, 13);
    s.fill(b, color::WHITE);
    s.bevel(b, Bevel::Sunken);
    if on {
        s.text(b.x + 3, b.y + 3, "x", color::BLACK);
    }
    s.text(r.x + 20, r.y + (r.h - GLYPH_H) / 2, label, color::BLACK);
}

fn draw(s: &mut Surface, c: &Choice, drives: &[(String, PathBuf)], state: &State) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.group_box(Rect::new(10, 20, W - 20, 104), "Back up these folders");
    for (i, f) in FOLDERS.iter().enumerate() {
        checkbox(s, check_rect(i), f, c.folders.iter().any(|x| x == f));
    }
    s.group_box(Rect::new(10, 140, W - 20, H - 260), "To this drive");
    if drives.is_empty() {
        s.text(24, 162, "Plug in a USB drive to back up to.", color::SHADOW);
    }
    for (i, (name, _)) in drives.iter().enumerate().take(4) {
        let r = drive_rect(i);
        let picked = c.drive.as_ref() == Some(name);
        if picked {
            s.fill(r, color::NAVY);
        }
        s.text(
            r.x + 6,
            r.y + (r.h - GLYPH_H) / 2,
            &ellipsize(name, ((r.w - 12) / GLYPH_W) as usize),
            if picked { color::WHITE } else { color::BLACK },
        );
    }
    checkbox(s, auto_rect(), "Back up by itself when this drive is plugged in", c.auto);
    let said = match state {
        State::Idle(t) | State::Busy(t) => t.as_str(),
    };
    s.text(14, H - 72, &ellipsize(said, ((W - 28) / GLYPH_W) as usize), color::BLACK);
    let busy = matches!(state, State::Busy(_));
    let ready = !busy && c.drive.as_ref().is_some_and(|d| drives.iter().any(|(n, _)| n == d)) && !c.folders.is_empty();
    ferro_ui::button(s, go_button(), if busy { "Copying..." } else { "Back Up Now" }, ready);
    ferro_ui::button(s, open_button(), "Open Backup", c.drive.as_ref().is_some_and(|d| drives.iter().any(|(n, _)| n == d)));
}

fn window() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
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
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Backup")?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, b"Backup")?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-backup\0ferro-backup\0")?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;
    let mut choice = load();
    let mut found = drives();
    let mut state = State::Idle("Only what's new or changed is copied; nothing on the drive is deleted.".into());
    let mut working: Option<Receiver<Result<copy::Progress, copy::Done>>> = None;
    let paint = |c: &Choice, d: &[(String, PathBuf)], st: &State| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, c, d, st);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    let mut ticks = 0u32;
    loop {
        let Some(event) = conn.poll_for_event()? else {
            std::thread::sleep(Duration::from_millis(50));
            ticks += 1;
            if let Some(rx) = &working {
                let mut last = None;
                while let Ok(m) = rx.try_recv() {
                    last = Some(m);
                }
                match last {
                    Some(Ok(p)) => {
                        state = State::Busy(format!("Copying {}...", p.name));
                        paint(&choice, &found, &state)?;
                    }
                    Some(Err(done)) => {
                        working = None;
                        state = State::Idle(done.said());
                        notify("Backup finished", &done.said());
                        paint(&choice, &found, &state)?;
                    }
                    None => {}
                }
            }
            // Drives come and go.
            if ticks.is_multiple_of(40) {
                let now = drives();
                if now != found {
                    found = now;
                    paint(&choice, &found, &state)?;
                }
            }
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => paint(&choice, &found, &state)?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::KeyPress(k) if keymap.typed(&k) == Key::Escape => return Ok(()),
            Event::ButtonPress(b) if b.detail == 1 && working.is_none() => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if let Some(i) = (0..FOLDERS.len()).find(|&i| check_rect(i).contains(x, y)) {
                    let f = FOLDERS[i].to_string();
                    if choice.folders.contains(&f) {
                        choice.folders.retain(|x| *x != f);
                    } else {
                        choice.folders.push(f);
                    }
                } else if let Some(i) = (0..found.len().min(4)).find(|&i| drive_rect(i).contains(x, y)) {
                    choice.drive = Some(found[i].0.clone());
                } else if auto_rect().contains(x, y) {
                    choice.auto = !choice.auto;
                } else if open_button().contains(x, y) {
                    if let Some((_, at)) = found.iter().find(|(n, _)| Some(n) == choice.drive.as_ref()) {
                        let backup = at.join("FerroOS Backup");
                        let _ = std::fs::create_dir_all(&backup);
                        let _ = std::process::Command::new("ferro-files").arg(backup).spawn();
                    }
                    continue;
                } else if go_button().contains(x, y) {
                    if let Some((_, at)) = found.iter().find(|(n, _)| Some(n) == choice.drive.as_ref()).cloned() {
                        let (tx, rx) = channel();
                        let c = Choice { folders: choice.folders.clone(), drive: choice.drive.clone(), auto: choice.auto };
                        std::thread::spawn(move || {
                            let done = back_up(&c, &at, |p| {
                                let _ = tx.send(Ok(p));
                            });
                            let _ = tx.send(Err(done));
                        });
                        working = Some(rx);
                        state = State::Busy("Looking for what's changed...".into());
                    }
                } else {
                    continue;
                }
                save(&choice);
                paint(&choice, &found, &state)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_read_back_as_written() {
        let c = Choice { folders: vec!["Documents".into(), "Pictures".into()], drive: Some("STICK".into()), auto: true };
        assert_eq!(Choice::parse(&c.to_text()), c);
        let fresh = Choice::parse("");
        assert_eq!(fresh.folders.len(), 5);
        assert!(!fresh.auto && fresh.drive.is_none());
    }
}
