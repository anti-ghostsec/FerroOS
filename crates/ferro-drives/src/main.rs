//! ferro-drives: encrypted extra drives, the windows.
//!
//!   ferro-drives                 Encrypt a Drive: pick a disk, name it, give
//!                                it a passphrase; it's erased and made one
//!                                LUKS2 (Argon2id) drive holding ext4, yours
//!   ferro-drives --unlock DEV    the box that asks for an encrypted drive's
//!                                passphrase (ferro-automount opens it when
//!                                one is plugged in, or is there at logon)
//!
//! Both can tick "Unlock automatically on this PC": the drive's key is then
//! kept on the encrypted system disk and it opens at startup, before anyone
//! logs on, at /Drives/NAME (a fixed place, for a server's files). Without
//! it the passphrase is asked each time, and the drive opens under /media.
//!
//! The erasing, and keeping keys, is done by ferro-drive (heavy/), as root
//! through pkexec, which checks the same rules again. Unlocking is udisks'
//! (administrators may, see harden.sh), so it needs no password but the
//! drive's. Passphrases go to both on their standard input, never in a
//! command line, and are wiped from memory when done with.
//!
//! Everything works from the keyboard: Tab and Shift+Tab move (the control
//! the keyboard is on is outlined), the arrows choose a disk, Space ticks or
//! presses, Enter goes on and says what's missing if it can't, Esc leaves.

mod disks;

use disks::Disk;
use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button, focus_rect, forget, text_field, Key, Keymap, Res};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const HELPER: &str = "/usr/local/sbin/ferro-drive";
const MARGIN: i32 = 14;
const ROW_H: i32 = 18;
const BTN_W: i32 = 84;
const BTN_H: i32 = 24;

fn output(program: &str, args: &[&str]) -> String {
    Command::new(program).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Runs `program` with `secret` on its standard input; what it printed (or
/// its error), and whether it worked.
fn run_with_secret(program: &str, args: &[&str], secret: &str, newline: bool) -> (bool, String) {
    let child = Command::new(program)
        .args(args)
        .env("FERRO_ADMIN_FOR", "Encrypted drives")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return (false, format!("couldn't start {program}")) };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(secret.as_bytes());
        if newline {
            let _ = stdin.write_all(b"\n");
        }
    }
    match child.wait_with_output() {
        Ok(o) => {
            let text = if o.status.success() { o.stdout } else { o.stderr };
            (o.status.success(), String::from_utf8_lossy(&text).trim().to_owned())
        }
        Err(e) => (false, e.to_string()),
    }
}

/// The last line of an error, without the program's name in front.
fn why(said: &str) -> String {
    let line = said.lines().last().unwrap_or("").trim();
    line.split_once(": ").map_or(line, |(_, w)| w).to_owned()
}

/// udisks unlocks `dev` with `pass` and mounts what's inside under /media.
fn unlock_with_udisks(dev: &str, pass: &str) -> Result<String, String> {
    let (ok, said) = run_with_secret("udisksctl", &["unlock", "--no-user-interaction", "-b", dev, "--key-file", "/dev/stdin"], pass, false);
    if !ok {
        return Err(if said.contains("passphrase") || said.contains("No key available") {
            "That passphrase doesn't unlock it.".into()
        } else {
            why(&said)
        });
    }
    let inner = disks::unlocked_as(&said).ok_or_else(|| "It unlocked, but udisks didn't say as what.".to_owned())?;
    let out = Command::new("udisksctl").args(["mount", "--no-user-interaction", "-b", &inner]).output().map_err(|e| e.to_string())?;
    let said = String::from_utf8_lossy(&out.stdout);
    disks::mounted_at(&said).ok_or_else(|| why(&String::from_utf8_lossy(&out.stderr)))
}

fn notify(title: &str, body: &str) {
    if let Ok(mut child) = Command::new("notify-send")
        .args(["-a", "Drives", "-i", "drive-harddisk", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
}

fn checkbox(s: &mut Surface, x: i32, y: i32, label: &str, ticked: bool, focused: bool) -> Rect {
    let b = Rect::new(x, y, 13, 13);
    s.fill(b, color::WHITE);
    s.bevel(b, Bevel::Sunken);
    if ticked {
        s.sprite_mono(b.x + 3, b.y + 3, icons::GLYPH_CHECK, color::BLACK);
    }
    s.text(b.right() + 8, b.y + 3, label, color::BLACK);
    let around = Rect::new(b.right() + 5, b.y, text_width(label) + 6, GLYPH_H + 6);
    if focused {
        focus_rect(s, around);
    }
    Rect::new(b.x, b.y - 2, around.right() - b.x, 17)
}

/// The focus outline in `c`: white on the navy of a chosen row, where
/// Windows' black one wouldn't show.
fn dotted(s: &mut Surface, r: Rect, c: u32) {
    for x in (r.x..r.right()).step_by(2) {
        s.hline(x, r.y, 1, c);
        s.hline(x, r.bottom() - 1, 1, c);
    }
    for y in (r.y..r.bottom()).step_by(2) {
        s.vline(r.x, y, 1, c);
        s.vline(r.right() - 1, y, 1, c);
    }
}

fn masked(secret: &str) -> String {
    "*".repeat(secret.chars().count())
}

// ---------------------------------------------------------------- Encrypt a Drive

const EW: i32 = 470;
const EH: i32 = 396;
const LIST: Rect = Rect::new(MARGIN, 56, EW - 2 * MARGIN, 4 * ROW_H + 4);
const FIELD_X: i32 = 166;

fn e_field(i: i32) -> Rect {
    Rect::new(FIELD_X, 150 + i * 26, EW - MARGIN - FIELD_X, 20)
}
const AUTO_Y: i32 = 236;
const SURE_Y: i32 = 284;
const E_GO: Rect = Rect::new(EW - MARGIN - 2 * BTN_W - 8, EH - 36, BTN_W, BTN_H);
const E_CANCEL: Rect = Rect::new(EW - MARGIN - BTN_W, EH - 36, BTN_W, BTN_H);

/// The controls in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EC {
    List,
    Name,
    Pass,
    Again,
    Auto,
    Sure,
    Go,
    Cancel,
}
const E_ORDER: [EC; 8] = [EC::List, EC::Name, EC::Pass, EC::Again, EC::Auto, EC::Sure, EC::Go, EC::Cancel];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    Asking,
    Working,
    Done,
    Failed,
}

struct Encrypt {
    disks: Vec<Disk>,
    chosen: usize,
    name: String,
    pass: String,
    again: String,
    auto: bool,
    sure: bool,
    focus: EC,
    message: String,
    stage: Stage,
    /// What ferro-drive says as it goes.
    lines: Vec<String>,
}

impl Encrypt {
    fn new(disks: Vec<Disk>) -> Self {
        Self {
            disks,
            chosen: 0,
            name: String::new(),
            pass: String::new(),
            again: String::new(),
            auto: false,
            sure: false,
            focus: EC::List,
            message: String::new(),
            stage: Stage::Asking,
            lines: Vec::new(),
        }
    }

    /// Why it can't go yet, if it can't.
    fn missing(&self) -> Option<String> {
        if self.disks.is_empty() {
            return Some("There's no drive that can be encrypted.".into());
        }
        if let Err(w) = disks::check_name(&self.name) {
            return Some(w.into());
        }
        if let Err(w) = disks::check_passphrase(&self.pass, &self.again) {
            return Some(w.into());
        }
        (!self.sure).then(|| "Tick \"Yes, erase this drive\" first (Space).".into())
    }

    fn rect(&self, c: EC) -> Rect {
        match c {
            EC::List => LIST,
            EC::Name => e_field(0),
            EC::Pass => e_field(1),
            EC::Again => e_field(2),
            EC::Auto => Rect::new(MARGIN, AUTO_Y - 2, 300, 17),
            EC::Sure => Rect::new(MARGIN, SURE_Y - 2, 300, 17),
            EC::Go => E_GO,
            EC::Cancel => E_CANCEL,
        }
    }

    fn typing(&mut self) -> Option<&mut String> {
        match self.focus {
            EC::Name => Some(&mut self.name),
            EC::Pass => Some(&mut self.pass),
            EC::Again => Some(&mut self.again),
            _ => None,
        }
    }

    fn forget_secrets(&mut self) {
        forget(&mut self.pass);
        forget(&mut self.again);
    }

    fn draw(&self, s: &mut Surface) {
        s.fill(Rect::new(0, 0, EW, EH), color::FACE);
        s.sprite(MARGIN, 12, icons::DRIVE, 1);
        s.text_bold(MARGIN + 24, 16, "Encrypt a Drive", color::BLACK);
        s.text(MARGIN, 38, "Everything on the drive you pick is erased.", color::BLACK);
        if self.stage != Stage::Asking {
            return self.draw_progress(s);
        }
        s.fill(LIST, color::WHITE);
        s.bevel(LIST, Bevel::Sunken);
        if self.disks.is_empty() {
            s.text(LIST.x + 6, LIST.y + 6, "No drive can be encrypted: plug one in.", color::SHADOW);
        }
        for (i, d) in self.disks.iter().enumerate().take(4) {
            let r = Rect::new(LIST.x + 2, LIST.y + 2 + i as i32 * ROW_H, LIST.w - 4, ROW_H);
            let chosen = i == self.chosen;
            if chosen {
                s.fill(r, color::NAVY);
            }
            let room = ((r.w - 10) / GLYPH_W).max(1) as usize;
            s.text(r.x + 4, r.y + 4, &ellipsize(&d.line(), room), if chosen { color::WHITE } else { color::BLACK });
            if chosen && self.focus == EC::List {
                dotted(s, r.inset(1), color::WHITE);
            }
        }
        s.text(MARGIN, 136, "FerroOS's own disk, and drives in use, aren't listed.", color::SHADOW);
        for (i, (label, text, secret)) in
            [("Name:", &self.name, false), ("Passphrase:", &self.pass, true), ("Passphrase again:", &self.again, true)].into_iter().enumerate()
        {
            let r = e_field(i as i32);
            s.text(MARGIN, r.y + (r.h - GLYPH_H) / 2, label, color::BLACK);
            let shown = if secret { masked(text) } else { text.clone() };
            text_field(s, r, &shown, self.focus == [EC::Name, EC::Pass, EC::Again][i]);
        }
        checkbox(s, MARGIN, AUTO_Y, "Unlock automatically on this PC", self.auto, self.focus == EC::Auto);
        s.text(MARGIN + 21, AUTO_Y + 18, "At startup, at /Drives/NAME: for servers.", color::SHADOW);
        checkbox(s, MARGIN, SURE_Y, "Yes, erase this drive", self.sure, self.focus == EC::Sure);
        s.text(MARGIN, SURE_Y + 26, &ellipsize(&self.message, ((EW - 2 * MARGIN) / GLYPH_W) as usize), color::RED);
        let ready = self.missing().is_none();
        default_button(s, E_GO, "Encrypt", ready);
        if self.focus == EC::Go {
            focus_rect(s, E_GO.inset(4));
        }
        button(s, E_CANCEL, "Cancel", true);
        if self.focus == EC::Cancel {
            focus_rect(s, E_CANCEL.inset(4));
        }
    }

    fn draw_progress(&self, s: &mut Surface) {
        let d = self.disks.get(self.chosen).map(Disk::line).unwrap_or_default();
        s.text(MARGIN, 64, &d, color::BLACK);
        for (i, line) in self.lines.iter().rev().take(9).rev().enumerate() {
            s.text(MARGIN, 92 + i as i32 * 18, &ellipsize(line, ((EW - 2 * MARGIN) / GLYPH_W) as usize), color::BLACK);
        }
        match self.stage {
            Stage::Working => {
                s.text(MARGIN, EH - 60, "Working... this can take a minute.", color::SHADOW);
            }
            Stage::Failed => {
                s.text(MARGIN, EH - 60, &ellipsize(&self.message, ((EW - 2 * MARGIN) / GLYPH_W) as usize), color::RED);
            }
            _ => {}
        }
        if self.stage != Stage::Working {
            default_button(s, E_CANCEL, "Close", true);
        }
    }

    /// A press on control `c` (a click, or Space or Enter on it).
    fn press(&mut self, c: EC) -> Action {
        self.focus = c;
        match c {
            EC::Auto => self.auto = !self.auto,
            EC::Sure => {
                self.sure = !self.sure;
                self.message.clear();
            }
            EC::Go => return self.go(),
            EC::Cancel => return Action::Close,
            _ => {}
        }
        Action::Repaint
    }

    fn go(&mut self) -> Action {
        match self.missing() {
            Some(m) => {
                self.message = m;
                ferro_ui::sound("error");
                Action::Repaint
            }
            None => {
                self.stage = Stage::Working;
                Action::Start
            }
        }
    }

    fn key(&mut self, key: Key, shift: bool) -> Action {
        if self.stage != Stage::Asking {
            return match key {
                Key::Enter | Key::Escape if self.stage != Stage::Working => Action::Close,
                _ => Action::Nothing,
            };
        }
        let at = E_ORDER.iter().position(|&c| c == self.focus).unwrap_or(0);
        match key {
            Key::Escape => return Action::Close,
            Key::Next => {
                self.focus = E_ORDER[ferro_ui::focus_step(Some(at), E_ORDER.len(), shift).unwrap_or(0)];
            }
            Key::Up if self.focus == EC::List => self.chosen = self.chosen.saturating_sub(1),
            Key::Down if self.focus == EC::List => self.chosen = (self.chosen + 1).min(self.disks.len().saturating_sub(1)),
            Key::Char(' ') if matches!(self.focus, EC::Auto | EC::Sure | EC::Go | EC::Cancel) => return self.press(self.focus),
            Key::Enter if matches!(self.focus, EC::Cancel) => return Action::Close,
            Key::Enter => return self.go(),
            Key::Backspace => {
                if let Some(t) = self.typing() {
                    t.pop();
                }
            }
            Key::Char(c) if !c.is_control() => {
                let limit = if self.focus == EC::Name { 16 } else { 256 };
                if let Some(t) = self.typing() {
                    if t.chars().count() < limit {
                        t.push(c);
                    }
                }
                self.message.clear();
            }
            _ => return Action::Nothing,
        }
        Action::Repaint
    }

    fn click(&mut self, x: i32, y: i32) -> Action {
        if self.stage != Stage::Asking {
            return if self.stage != Stage::Working && E_CANCEL.contains(x, y) { Action::Close } else { Action::Nothing };
        }
        if LIST.contains(x, y) {
            let i = ((y - LIST.y - 2) / ROW_H).max(0) as usize;
            if i < self.disks.len() {
                self.chosen = i;
            }
            self.focus = EC::List;
            return Action::Repaint;
        }
        match E_ORDER.into_iter().find(|&c| self.rect(c).contains(x, y)) {
            Some(c) => self.press(c),
            None => Action::Nothing,
        }
    }
}

/// What the work aside reports.
enum Report {
    Line(String),
    Done(String),
    Failed(String),
}

enum Action {
    Nothing,
    Repaint,
    Start,
    Close,
}

/// The work, aside from the window: close what's open from it, then
/// ferro-drive encrypts (and keeps the key, if asked); without the key kept,
/// udisks opens it now so it's ready in My Computer.
fn encrypt_aside(disk: Disk, name: String, mut pass: String, auto: bool) -> Receiver<Report> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let say = |l: &str| {
            let _ = tx.send(Report::Line(l.to_owned()));
        };
        for part in &disk.to_unmount {
            say(&format!("Closing {part}..."));
            let _ = Command::new("udisksctl").args(["unmount", "--no-user-interaction", "-b", part]).output();
        }
        say("Asking for the administrator's password...");
        let dev = format!("/dev/{}", disk.name);
        let mut args = vec![HELPER, "encrypt", &dev, &name];
        if auto {
            args.push("auto");
        }
        let child = Command::new("pkexec")
            .args(&args)
            .env("FERRO_ADMIN_FOR", "Encrypt a Drive")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                forget(&mut pass);
                let _ = tx.send(Report::Failed(format!("couldn't start ferro-drive: {e}")));
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(pass.as_bytes());
            let _ = stdin.write_all(b"\n");
        }
        let err = child.stderr.take();
        let (mut part, mut open) = (String::new(), String::new());
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(p) = line.strip_prefix("Done: ") {
                    part = p.trim().to_owned();
                } else if let Some(o) = line.strip_prefix("Open: ") {
                    open = o.trim().to_owned();
                } else {
                    say(&line);
                }
            }
        }
        let mut problem = String::new();
        if let Some(err) = err {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                problem = line;
            }
        }
        if !child.wait().is_ok_and(|s| s.success()) || part.is_empty() {
            forget(&mut pass);
            let _ = tx.send(Report::Failed(if problem.is_empty() { "It stopped (no administrator's password?).".into() } else { why(&problem) }));
            return;
        }
        forget(&mut pass);
        // With the key kept it's open at /Drives/NAME already; without, it's
        // open (never closed) and udisks mounts it under /media.
        let place = if auto || open.is_empty() {
            Ok(open)
        } else {
            say("Opening it...");
            Command::new("udisksctl")
                .args(["mount", "--no-user-interaction", "-b", &open])
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| disks::mounted_at(&String::from_utf8_lossy(&o.stdout)).ok_or_else(|| why(&String::from_utf8_lossy(&o.stderr))))
        };
        match place {
            Ok(at) => {
                let _ = tx.send(Report::Done(format!("Done. {name} is ready at {at}")));
            }
            Err(e) => {
                let _ = tx.send(Report::Failed(format!("It's encrypted, but couldn't be opened: {e}")));
            }
        }
    });
    rx
}

// ---------------------------------------------------------------- the unlock box

const UW: i32 = 420;
const UH: i32 = 200;
const U_FIELD: Rect = Rect::new(MARGIN, 74, UW - 2 * MARGIN, 22);
const U_AUTO_Y: i32 = 108;
const U_GO: Rect = Rect::new(UW - MARGIN - 2 * BTN_W - 8, UH - 36, BTN_W, BTN_H);
const U_CANCEL: Rect = Rect::new(UW - MARGIN - BTN_W, UH - 36, BTN_W, BTN_H);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UC {
    Field,
    Auto,
    Go,
    Cancel,
}
const U_ORDER: [UC; 4] = [UC::Field, UC::Auto, UC::Go, UC::Cancel];

struct Unlock {
    dev: String,
    what: String,
    pass: String,
    auto: bool,
    focus: UC,
    message: String,
}

impl Unlock {
    fn draw(&self, s: &mut Surface) {
        s.fill(Rect::new(0, 0, UW, UH), color::FACE);
        s.sprite(MARGIN, 12, icons::DRIVE, 1);
        s.text_bold(MARGIN + 24, 16, "This drive is encrypted", color::BLACK);
        let room = ((UW - 2 * MARGIN) / GLYPH_W) as usize;
        s.text(MARGIN, 38, &ellipsize(&self.what, room), color::BLACK);
        s.text(MARGIN, 56, "Type its passphrase to open it:", color::BLACK);
        text_field(s, U_FIELD, &masked(&self.pass), self.focus == UC::Field);
        checkbox(s, MARGIN, U_AUTO_Y, "Unlock automatically on this PC", self.auto, self.focus == UC::Auto);
        s.text(MARGIN + 21, U_AUTO_Y + 18, "Asks for the administrator's password once.", color::SHADOW);
        s.text(MARGIN, U_AUTO_Y + 38, &ellipsize(&self.message, room), color::RED);
        let ready = !self.pass.is_empty();
        default_button(s, U_GO, "Unlock", ready);
        if self.focus == UC::Go {
            focus_rect(s, U_GO.inset(4));
        }
        button(s, U_CANCEL, "Cancel", true);
        if self.focus == UC::Cancel {
            focus_rect(s, U_CANCEL.inset(4));
        }
    }

    fn rect(&self, c: UC) -> Rect {
        match c {
            UC::Field => U_FIELD,
            UC::Auto => Rect::new(MARGIN, U_AUTO_Y - 2, 300, 17),
            UC::Go => U_GO,
            UC::Cancel => U_CANCEL,
        }
    }

    /// Unlock pressed: true when it's open (the box can go).
    fn go(&mut self) -> bool {
        if self.pass.is_empty() {
            self.message = "Type the drive's passphrase first.".into();
            return false;
        }
        match unlock_with_udisks(&self.dev, &self.pass) {
            Ok(at) => {
                let label = at.rsplit('/').next().unwrap_or("The drive").to_owned();
                if self.auto {
                    let (ok, said) = run_with_secret("pkexec", &[HELPER, "remember", &self.dev], &self.pass, true);
                    if ok {
                        notify(&format!("{label} is ready"), "It will unlock by itself from now on, at /Drives.");
                    } else {
                        notify(&format!("{label} is ready"), &format!("It couldn't be set to unlock by itself: {}", why(&said)));
                    }
                } else {
                    notify(&format!("{label} is ready"), "Open it from My Computer.");
                }
                forget(&mut self.pass);
                true
            }
            Err(e) => {
                self.message = e;
                forget(&mut self.pass);
                ferro_ui::sound("error");
                false
            }
        }
    }

    /// `Some(true)` to close (done or cancelled), `Some(false)` to repaint.
    fn key(&mut self, key: Key, shift: bool) -> Option<bool> {
        let at = U_ORDER.iter().position(|&c| c == self.focus).unwrap_or(0);
        match key {
            Key::Escape => return Some(true),
            Key::Next => self.focus = U_ORDER[ferro_ui::focus_step(Some(at), U_ORDER.len(), shift).unwrap_or(0)],
            Key::Char(' ') if self.focus == UC::Auto => self.auto = !self.auto,
            Key::Char(' ') | Key::Enter if self.focus == UC::Cancel => return Some(true),
            Key::Char(' ') if self.focus == UC::Go => return Some(self.go()),
            Key::Enter => return Some(self.go()),
            Key::Backspace if self.focus == UC::Field => {
                self.pass.pop();
            }
            Key::Char(c) if self.focus == UC::Field && !c.is_control() && self.pass.len() < 512 => {
                self.pass.push(c);
                self.message.clear();
            }
            _ => return None,
        }
        Some(false)
    }

    fn click(&mut self, x: i32, y: i32) -> Option<bool> {
        let c = U_ORDER.into_iter().find(|&c| self.rect(c).contains(x, y))?;
        self.focus = c;
        Some(match c {
            UC::Auto => {
                self.auto = !self.auto;
                false
            }
            UC::Go => self.go(),
            UC::Cancel => true,
            UC::Field => false,
        })
    }
}

// ---------------------------------------------------------------- the window

struct Win {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    w: i32,
    h: i32,
    delete: Atom,
    keys: Keymap,
}

impl Win {
    fn open(title: &str, w: i32, h: i32) -> Res<Self> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let (sw, sh, root, depth, visual) =
            (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels), screen.root, screen.root_depth, screen.root_visual);
        let win = conn.generate_id()?;
        let gc = conn.generate_id()?;
        let values = CreateWindowAux::new()
            .background_pixel(color::FACE)
            .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY);
        conn.create_window(
            depth,
            win,
            root,
            ((sw - w) / 2) as i16,
            ((sh - h) / 2) as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &values,
        )?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
        let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
        let (wtype, dialog) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?);
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-drives\0ferro-drives\0")?;
        conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[dialog])?;
        conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
        ferro_ui::fixed_size(&conn, win, w, h)?;
        conn.map_window(win)?;
        conn.flush()?;
        let keys = Keymap::load(&conn)?;
        Ok(Self { conn, win, gc, depth, w, h, delete, keys })
    }

    fn show(&self, s: &Surface) -> Res<()> {
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, s, self.w, self.h)
    }
}

fn run_encrypt() -> Res<()> {
    let blocks = disks::parse_lsblk(&output("lsblk", &["-P", "-b", "-e", "2,7,11", "-o", disks::LSBLK_COLUMNS]));
    let mut st = Encrypt::new(disks::offered(&blocks));
    let win = Win::open("Encrypt a Drive", EW, EH)?;
    let mut s = Surface::new(EW as usize, EH as usize);
    let mut work: Option<Receiver<Report>> = None;
    loop {
        // While it works, its reports are looked for between events.
        let event = if work.is_some() {
            match win.conn.poll_for_event()? {
                Some(e) => Some(e),
                None => {
                    let mut changed = false;
                    while let Some(rx) = &work {
                        match rx.try_recv() {
                            Ok(Report::Done(line)) => {
                                st.lines.push(line);
                                st.stage = Stage::Done;
                                work = None;
                            }
                            Ok(Report::Line(line)) => st.lines.push(line),
                            Ok(Report::Failed(e)) => {
                                st.message = e;
                                st.stage = Stage::Failed;
                                ferro_ui::sound("error");
                                work = None;
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => work = None,
                        }
                        changed = true;
                    }
                    if changed {
                        st.draw(&mut s);
                        win.show(&s)?;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    None
                }
            }
        } else {
            Some(win.conn.wait_for_event()?)
        };
        let Some(event) = event else { continue };
        let action = match event {
            Event::Expose(e) if e.count == 0 => Action::Repaint,
            Event::MapNotify(_) => {
                let _ = win.conn.set_input_focus(InputFocus::PARENT, win.win, x11rb::CURRENT_TIME);
                Action::Repaint
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == win.delete && st.stage != Stage::Working => Action::Close,
            Event::ButtonPress(bp) if bp.detail == 1 => st.click(i32::from(bp.event_x), i32::from(bp.event_y)),
            Event::KeyPress(kp) => st.key(win.keys.typed(&kp), kp.state.contains(KeyButMask::SHIFT)),
            _ => Action::Nothing,
        };
        match action {
            Action::Nothing => continue,
            Action::Close => {
                st.forget_secrets();
                return Ok(());
            }
            Action::Start => {
                let disk = st.disks[st.chosen].clone();
                work = Some(encrypt_aside(disk, st.name.clone(), st.pass.clone(), st.auto));
                st.forget_secrets();
            }
            Action::Repaint => {}
        }
        st.draw(&mut s);
        win.show(&s)?;
    }
}

fn run_unlock(dev: &str) -> Res<()> {
    let info = output("lsblk", &["-d", "-n", "-o", "LABEL,SIZE", dev]);
    let mut words = info.split_whitespace().collect::<Vec<_>>();
    let size = words.pop().unwrap_or("");
    let label = words.join(" ");
    let what = if label.is_empty() { format!("{dev} ({size})") } else { format!("{label} ({size}), {dev}") };
    let mut st = Unlock { dev: dev.to_owned(), what, pass: String::new(), auto: false, focus: UC::Field, message: String::new() };
    let win = Win::open("Unlock Drive", UW, UH)?;
    ferro_ui::sound("ding");
    let mut s = Surface::new(UW as usize, UH as usize);
    loop {
        let close = match win.conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => Some(false),
            Event::MapNotify(_) => {
                let _ = win.conn.set_input_focus(InputFocus::PARENT, win.win, x11rb::CURRENT_TIME);
                Some(false)
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == win.delete => Some(true),
            Event::ButtonPress(bp) if bp.detail == 1 => st.click(i32::from(bp.event_x), i32::from(bp.event_y)),
            Event::KeyPress(kp) => st.key(win.keys.typed(&kp), kp.state.contains(KeyButMask::SHIFT)),
            _ => None,
        };
        match close {
            Some(true) => {
                forget(&mut st.pass);
                return Ok(());
            }
            Some(false) => {
                st.draw(&mut s);
                win.show(&s)?;
            }
            None => {}
        }
    }
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--encrypt") => run_encrypt(),
        Some("--unlock") if args.len() == 2 && args[1].starts_with("/dev/") => run_unlock(&args[1]),
        _ => {
            eprintln!("usage: ferro-drives [--encrypt] | --unlock /dev/DEVICE");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> Disk {
        Disk { name: "sda".into(), bytes: 1 << 40, model: "SSD".into(), usb: false, to_unmount: Vec::new() }
    }

    #[test]
    fn encrypt_by_keyboard_alone() {
        let mut st = Encrypt::new(vec![disk()]);
        // Enter too soon says what's missing, and doesn't start.
        assert!(matches!(st.key(Key::Enter, false), Action::Repaint));
        assert!(st.message.contains("name"));
        for c in [EC::Name, EC::Pass, EC::Again] {
            st.key(Key::Next, false);
            assert_eq!(st.focus, c);
            for ch in if c == EC::Name { "Matrix".chars() } else { "a good passphrase".chars() } {
                st.key(Key::Char(ch), false);
            }
        }
        assert!(matches!(st.key(Key::Enter, false), Action::Repaint));
        assert!(st.message.contains("erase"));
        st.key(Key::Next, false);
        st.key(Key::Char(' '), false);
        assert!(st.auto);
        st.key(Key::Next, false);
        assert_eq!(st.focus, EC::Sure);
        st.key(Key::Char(' '), false);
        assert!(st.sure && st.missing().is_none());
        assert!(matches!(st.key(Key::Enter, false), Action::Start));
        // Shift+Tab goes back round.
        let mut st = Encrypt::new(vec![disk()]);
        st.key(Key::Next, true);
        assert_eq!(st.focus, EC::Cancel);
    }

    #[test]
    fn nothing_to_offer() {
        let mut st = Encrypt::new(Vec::new());
        st.name = "Matrix".into();
        assert!(st.missing().unwrap().contains("no drive"));
        assert!(matches!(st.key(Key::Enter, false), Action::Repaint));
    }

    #[test]
    fn unlock_box_by_keyboard() {
        let mut st =
            Unlock { dev: "/dev/sdb1".into(), what: String::new(), pass: String::new(), auto: false, focus: UC::Field, message: String::new() };
        st.key(Key::Char('x'), false);
        assert_eq!(st.pass, "x");
        st.key(Key::Next, false);
        st.key(Key::Char(' '), false);
        assert!(st.auto);
        st.key(Key::Next, false);
        st.key(Key::Next, false);
        assert_eq!(st.key(Key::Enter, false), Some(true));
    }
}
