//! ferro-archive: FerroOS's archive window. Opens .zip, .7z and .rar,
//! lists what's inside (name, size, packed size, date) and extracts all of
//! it, the files selected, or all of it beside the archive (Extract here).
//!
//! `ferro-archive FILE` opens the window; `ferro-archive --extract-here
//! FILE` unpacks beside it with no window (File Explorer's right-click
//! Extract Here), printing `PROGRESS:n%` lines and then one line saying
//! what happened. It exits 0 when done, 2 when a password is needed (open
//! the window to type it), 1 otherwise.

mod view;

use ferro_archive::{extract, extract_here, free_name, human_size, open, stem, Error, Limits, Listing, Progress};
use ferro_gfx::Surface;
use ferro_ui::{Edit, FocusKey, Key, Keymap, Res, TextEdit};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;
use view::{Dialog, Hit, View, H, VISIBLE, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn main() {
    #[cfg(target_os = "linux")]
    if let Err(e) = lock_down() {
        eprintln!("ferro-archive: can't lock itself down ({e}); not opening archives without that.");
        std::process::exit(1);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.as_slice() {
        [flag, file] if flag == "--extract-here" => headless_extract_here(&arg_path(file)),
        [file] if !file.starts_with("--") => window(Some(arg_path(file))),
        [] => window(None),
        _ => {
            eprintln!("usage: ferro-archive [FILE] | ferro-archive --extract-here FILE");
            1
        }
    };
    std::process::exit(code);
}

/// A file named as Windows shows it (`C:\Users\...`) or as it is.
fn arg_path(a: &str) -> PathBuf {
    if ferro_path::is_windows_path(a) {
        if let Ok(p) = ferro_path::DriveTable::heavy().to_posix(a) {
            return PathBuf::from(p);
        }
    }
    PathBuf::from(a)
}

/// No internet, no starting programs (but the clipboard's and the
/// sounds'), none of the kernel's riskier corners: an archive made to take
/// over the decoders gets nowhere.
#[cfg(target_os = "linux")]
fn lock_down() -> Res<()> {
    use ferro_sandbox::linux;
    linux::lock_privileges()?;
    linux::install_seccomp()?;
    linux::install_net_gate(ferro_sandbox::NetMode::Deny)?;
    let allowed: Vec<&str> = ["/usr/bin/xclip", "/usr/local/bin/ferro-sound"].into_iter().filter(|p| Path::new(p).exists()).collect();
    if let Err(e) = linux::restrict_exec(&allowed) {
        // Only a kernel without Landlock; the rest still holds.
        if linux::landlock_abi() >= 1 {
            return Err(e.into());
        }
    }
    Ok(())
}

fn headless_extract_here(path: &Path) -> i32 {
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    let (p, file) = (progress.clone(), path.to_path_buf());
    std::thread::spawn(move || {
        let _ = tx.send(extract_here(&file, None, &p));
    });
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut last = u32::MAX;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok((said, _))) => {
                println!("{said}");
                return 0;
            }
            Ok(Err(Error::NeedPassword | Error::WrongPassword)) => {
                println!("'{name}' is password-protected. Open it to type the password.");
                return 2;
            }
            Ok(Err(e)) => {
                println!("Couldn't extract '{name}': {e}");
                return 1;
            }
            Err(_) => {
                let pct = progress.percent();
                if pct != last {
                    println!("PROGRESS:{pct}%");
                    last = pct;
                }
            }
        }
    }
}

/// What an extraction running aside was asked to do.
#[derive(Clone)]
enum Job {
    /// Into `dest`: `which` entries, or all.
    To {
        dest: PathBuf,
        which: Option<Vec<usize>>,
    },
    Here,
}

/// What a running extraction ended with: the line to show.
type Outcome = Result<String, Error>;

struct App {
    path: Option<PathBuf>,
    name: String,
    listing: Option<Listing>,
    password: Option<String>,
    selected: Vec<bool>,
    cursor: usize,
    anchor: usize,
    scroll: usize,
    focus: Option<usize>,
    status: String,
    dialog: Option<Dialog>,
    /// After the password: open, or this extraction again.
    after_password: Option<Option<Job>>,
    running: Option<(Arc<Progress>, Receiver<Outcome>)>,
    drives: ferro_path::DriveTable,
}

impl App {
    fn new(path: Option<PathBuf>) -> Self {
        let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut app = App {
            path,
            name,
            listing: None,
            password: None,
            selected: Vec::new(),
            cursor: 0,
            anchor: 0,
            scroll: 0,
            focus: None,
            status: "Open a .zip, .7z or .rar file from File Explorer.".into(),
            dialog: None,
            after_password: None,
            running: None,
            drives: ferro_path::DriveTable::heavy(),
        };
        app.load();
        app
    }

    fn view(&self) -> View<'_> {
        View {
            name: &self.name,
            listing: self.listing.as_ref(),
            selected: &self.selected,
            cursor: self.cursor,
            scroll: self.scroll,
            focus: self.focus,
            status: &self.status,
            dialog: self.dialog.as_ref(),
        }
    }

    fn shown(&self, p: &Path) -> String {
        let s = p.to_string_lossy();
        self.drives.to_windows(&s).unwrap_or_else(|| s.into_owned())
    }

    /// Reads the list (again, with the password once it's typed).
    fn load(&mut self) {
        let Some(path) = self.path.clone() else { return };
        match open(&path, self.password.as_deref()) {
            Ok(l) => {
                self.selected = vec![false; l.entries.len()];
                self.listing = Some(l);
                self.status = self.totals();
            }
            Err(e @ (Error::NeedPassword | Error::WrongPassword)) => {
                self.ask_password(None, e == Error::WrongPassword);
                self.status = "Password-protected.".into();
            }
            Err(e) => {
                self.status = format!("Couldn't open '{}'.", self.name);
                self.message("Archive", e.to_string());
            }
        }
    }

    /// The status bar: what's in it, or what's selected.
    fn totals(&self) -> String {
        let Some(l) = &self.listing else { return String::new() };
        let picked: Vec<usize> = (0..l.entries.len()).filter(|&i| self.selected[i]).collect();
        if !picked.is_empty() {
            return format!("{} selected, {}", picked.len(), human_size(l.size_of(Some(&picked))));
        }
        let files = l.entries.iter().filter(|e| !e.is_dir).count();
        let packed: u64 = l.entries.iter().map(|e| e.packed).sum();
        let locked = if l.entries.iter().any(|e| e.encrypted) { ", password-protected" } else { "" };
        format!("{} archive: {files} file(s), {} ({} packed){locked}", l.format.name(), human_size(l.size_of(None)), human_size(packed))
    }

    fn message(&mut self, title: &str, text: String) {
        ferro_ui::sound("error");
        self.dialog = Some(Dialog::Message { title: title.into(), text });
    }

    fn ask_password(&mut self, then: Option<Job>, wrong: bool) {
        if let Some(mut old) = self.password.take() {
            ferro_ui::forget(&mut old);
        }
        self.after_password = Some(then);
        self.dialog = Some(Dialog::Password { typed: String::new(), wrong });
    }

    fn picked(&self) -> Vec<usize> {
        (0..self.selected.len()).filter(|&i| self.selected[i]).collect()
    }

    /// Extract all / selected: asks where, starting at a new folder named
    /// after the archive, beside it.
    fn ask_folder(&mut self, which: Option<Vec<usize>>) {
        let Some(path) = &self.path else { return };
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let dest = free_name(&dir, stem(&self.name));
        let which = which.map(|w| self.listing.as_ref().map_or(w.clone(), |l| l.with_contents(&w)));
        self.dialog = Some(Dialog::Folder { edit: TextEdit::new(&self.shown(&dest)), which });
    }

    /// The folder typed: as Windows shows it, `~`, or as it is.
    fn typed_folder(&self, typed: &str) -> PathBuf {
        let typed = typed.trim();
        if ferro_path::is_windows_path(typed) {
            if let Ok(p) = self.drives.to_posix(typed) {
                return PathBuf::from(p);
            }
        }
        if let (Some(rest), Some(home)) = (typed.strip_prefix('~'), std::env::var_os("HOME")) {
            return PathBuf::from(home).join(rest.trim_start_matches('/'));
        }
        PathBuf::from(typed)
    }

    fn start(&mut self, job: Job) {
        let (Some(path), Some(listing)) = (self.path.clone(), self.listing.clone()) else { return };
        if let Job::To { dest, .. } = &job {
            if let Err(e) = std::fs::create_dir_all(dest) {
                self.message("Extract", format!("Couldn't make the folder '{}': {e}", self.shown(dest)));
                return;
            }
        }
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let (p, pw, j) = (progress.clone(), self.password.clone(), job.clone());
        let shown_dest = match &job {
            Job::To { dest, .. } => self.shown(dest),
            Job::Here => String::new(),
        };
        std::thread::spawn(move || {
            let outcome = match j {
                Job::To { dest, which } => extract(&path, &listing, &dest, which.as_deref(), pw.as_deref(), Limits::for_dest(&dest), &p)
                    .map(|r| r.summary_into(Some(&shown_dest))),
                Job::Here => extract_here(&path, pw.as_deref(), &p).map(|(said, _)| said),
            };
            let _ = tx.send(outcome);
        });
        self.after_password = Some(Some(job));
        self.running = Some((progress, rx));
        self.dialog = Some(Dialog::Progress { title: "Extracting".into(), percent: 0, done: 0, total: 0, current: String::new(), cancelling: false });
    }

    /// Checks on the running extraction. Whether anything changed.
    fn poll(&mut self) -> bool {
        let Some((progress, rx)) = &self.running else { return false };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => {
                if let Some(Dialog::Progress { percent, done, total, current, .. }) = &mut self.dialog {
                    *percent = progress.percent();
                    *done = progress.done.load(Ordering::Relaxed);
                    *total = progress.total.load(Ordering::Relaxed);
                    *current = progress.current.lock().map(|c| c.clone()).unwrap_or_default();
                }
                true
            }
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                self.dialog = None;
                self.status = "Extracting stopped unexpectedly.".into();
                true
            }
            Ok(outcome) => {
                self.running = None;
                self.dialog = None;
                let job = self.after_password.take().flatten();
                match outcome {
                    Ok(said) => self.status = said,
                    Err(e @ (Error::NeedPassword | Error::WrongPassword)) => self.ask_password(job, e == Error::WrongPassword),
                    Err(Error::Cancelled) => self.status = "Cancelled. What was extracted whole is kept.".into(),
                    Err(e) => {
                        self.status = "Extracting stopped.".into();
                        self.message("Extract", e.to_string());
                    }
                }
                true
            }
        }
    }

    fn cancel_job(&mut self) {
        if let Some((progress, _)) = &self.running {
            progress.cancel.store(true, Ordering::Relaxed);
            if let Some(Dialog::Progress { cancelling, .. }) = &mut self.dialog {
                *cancelling = true;
            }
        }
    }

    /// OK in the box over the window.
    fn dialog_ok(&mut self) {
        match self.dialog.take() {
            Some(Dialog::Folder { edit, which }) => {
                let dest = self.typed_folder(edit.text());
                if edit.text().trim().is_empty() {
                    self.dialog = Some(Dialog::Folder { edit, which });
                    return;
                }
                self.start(Job::To { dest, which });
            }
            Some(Dialog::Password { typed, .. }) => {
                self.password = Some(typed);
                match self.after_password.take() {
                    Some(Some(job)) => self.start(job),
                    _ => self.load(),
                }
            }
            Some(Dialog::Progress { .. }) => {}
            Some(Dialog::Message { .. }) | None => {}
        }
    }

    fn dialog_cancel(&mut self) {
        match self.dialog.take() {
            Some(Dialog::Password { mut typed, .. }) => {
                ferro_ui::forget(&mut typed);
                self.after_password = None;
                if self.listing.is_none() {
                    self.status = "Password-protected: not opened.".into();
                }
            }
            Some(d @ Dialog::Progress { .. }) => {
                self.dialog = Some(d);
                self.cancel_job();
            }
            _ => {}
        }
    }

    fn click_row(&mut self, i: usize, ctrl: bool, shift: bool) {
        if shift {
            let (a, b) = (self.anchor.min(i), self.anchor.max(i));
            if !ctrl {
                self.selected.iter_mut().for_each(|s| *s = false);
            }
            for s in &mut self.selected[a..=b] {
                *s = true;
            }
        } else if ctrl {
            self.selected[i] = !self.selected[i];
            self.anchor = i;
        } else {
            self.selected.iter_mut().for_each(|s| *s = false);
            self.selected[i] = true;
            self.anchor = i;
        }
        self.cursor = i;
        self.status = self.totals();
    }

    fn scroll_by(&mut self, d: isize) {
        let total = self.selected.len();
        let max = total.saturating_sub(VISIBLE);
        self.scroll = (self.scroll as isize + d).clamp(0, max as isize) as usize;
    }

    /// Keeps the cursor's row in sight.
    fn follow(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + VISIBLE {
            self.scroll = self.cursor + 1 - VISIBLE;
        }
    }

    /// A click (or a key handed on as one). Whether to close.
    fn click(&mut self, x: i32, y: i32, ctrl: bool, shift: bool) -> bool {
        match view::hit(x, y, &self.view()) {
            Hit::ExtractAll => self.ask_folder(None),
            Hit::ExtractSelected => {
                let p = self.picked();
                self.ask_folder(Some(p));
            }
            Hit::ExtractHere => self.start(Job::Here),
            Hit::Row(i) => self.click_row(i, ctrl, shift),
            Hit::ScrollUp => self.scroll_by(-1),
            Hit::ScrollDown => self.scroll_by(1),
            Hit::PageUp => self.scroll_by(-(VISIBLE as isize)),
            Hit::PageDown => self.scroll_by(VISIBLE as isize),
            Hit::DialogOk => self.dialog_ok(),
            Hit::DialogCancel => self.dialog_cancel(),
            Hit::DialogField => {
                if let Some(Dialog::Folder { edit, .. }) = &mut self.dialog {
                    edit.click(view::DLG_FIELD.inset(2), x, shift);
                }
            }
            Hit::Nothing => {}
        }
        false
    }

    /// A key. Whether to close.
    fn key(&mut self, e: &KeyPressEvent, key: Key, win: u32, pending: &mut Option<Event>) -> bool {
        let ctrl = e.state.contains(KeyButMask::CONTROL);
        let shift = e.state.contains(KeyButMask::SHIFT);
        match &mut self.dialog {
            Some(Dialog::Folder { edit, .. }) => match edit.key(key, ctrl, shift) {
                Edit::Done => self.dialog_ok(),
                Edit::Cancel => self.dialog = None,
                _ => {}
            },
            Some(Dialog::Password { typed, .. }) => match key {
                Key::Enter => self.dialog_ok(),
                Key::Escape => self.dialog_cancel(),
                Key::Backspace => {
                    typed.pop();
                }
                Key::Char(c) if !c.is_control() && !ctrl && typed.len() < 256 => typed.push(c),
                _ => {}
            },
            Some(Dialog::Progress { .. }) => {
                if key == Key::Escape {
                    self.cancel_job();
                }
            }
            Some(Dialog::Message { .. }) => {
                if ferro_ui::dialog_answer(key).is_some() || key == Key::Char(' ') {
                    self.dialog = None;
                }
            }
            None => {
                let n = self.selected.len();
                match (key, ferro_ui::focus_key(e, key, self.focus, &view::CONTROLS)) {
                    (Key::Escape, _) => return true,
                    (_, FocusKey::Moved(f)) => self.focus = f,
                    (_, FocusKey::Click(x, y)) => *pending = Some(ferro_ui::click_event(win, x, y)),
                    (Key::Char(c), _) if ctrl && c.eq_ignore_ascii_case(&'a') => {
                        self.selected.iter_mut().for_each(|s| *s = true);
                        self.status = self.totals();
                    }
                    (Key::Up | Key::Down | Key::Home | Key::End, _) if n > 0 => {
                        let to = match key {
                            Key::Up => self.cursor.saturating_sub(1),
                            Key::Down => (self.cursor + 1).min(n - 1),
                            Key::Home => 0,
                            _ => n - 1,
                        };
                        if ctrl {
                            self.cursor = to;
                        } else {
                            self.click_row(to, false, shift);
                        }
                        self.follow();
                    }
                    (Key::Char(' '), _) if ctrl && n > 0 => self.click_row(self.cursor, true, false),
                    // Enter: extract what's selected (or all of it).
                    (Key::Enter, _) if n > 0 => {
                        let p = self.picked();
                        self.ask_folder(if p.is_empty() { None } else { Some(p) });
                    }
                    _ => {}
                }
            }
        }
        false
    }
}

fn window(path: Option<PathBuf>) -> i32 {
    match run(path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ferro-archive: {e}");
            1
        }
    }
}

fn run(path: Option<PathBuf>) -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
    let keymap = Keymap::load(&conn)?;
    let mut app = App::new(path);

    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values =
        CreateWindowAux::new().background_pixel(screen.white_pixel).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS);
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2).max(0) as i16,
        ((sh - H) / 2).max(0) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &values,
    )?;
    let title = if app.name.is_empty() { "Archive".to_owned() } else { format!("{} - Archive", app.name) };
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-archive\0Ferro-archive\0")?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.map_window(win)?;
    conn.flush()?;

    let repaint = |app: &App, conn: &RustConnection| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        view::draw(&mut s, &app.view());
        ferro_ui::show(conn, win, gc, screen.root_depth, &s, W, H)
    };
    repaint(&app, &conn)?;

    // A key press handed on as a click on the focused button.
    let mut pending: Option<Event> = None;
    // Closed while extracting: once it has stopped.
    let mut closing = false;
    loop {
        if closing && app.running.is_none() {
            break;
        }
        let event = match pending.take() {
            Some(e) => Some(e),
            None if app.running.is_some() => {
                let e = conn.poll_for_event()?;
                if e.is_none() {
                    std::thread::sleep(Duration::from_millis(50));
                    if app.poll() {
                        repaint(&app, &conn)?;
                    }
                }
                e
            }
            None => Some(conn.wait_for_event()?),
        };
        let Some(event) = event else { continue };
        let close = match event {
            Event::ClientMessage(e) if e.data.as_data32()[0] == wm_delete => {
                // Closing while extracting: stop it first.
                app.cancel_job();
                closing = true;
                false
            }
            Event::Expose(e) if e.count == 0 => {
                repaint(&app, &conn)?;
                false
            }
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                let close = app.key(&e, key, win, &mut pending);
                repaint(&app, &conn)?;
                close
            }
            Event::ButtonPress(e) => {
                let ctrl = e.state.contains(KeyButMask::CONTROL);
                let shift = e.state.contains(KeyButMask::SHIFT);
                match e.detail {
                    4 if app.dialog.is_none() => app.scroll_by(-3),
                    5 if app.dialog.is_none() => app.scroll_by(3),
                    1 => {
                        app.click(i32::from(e.event_x), i32::from(e.event_y), ctrl, shift);
                    }
                    _ => {}
                }
                repaint(&app, &conn)?;
                false
            }
            _ => false,
        };
        if close && app.running.is_none() {
            break;
        }
    }
    if let Some(mut pw) = app.password.take() {
        ferro_ui::forget(&mut pw);
    }
    Ok(())
}
