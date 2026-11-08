//! ferro-archive: FerroOS's archive window. Opens .zip, .7z and .rar,
//! lists what's inside (name, size, packed size, date), extracts all of it,
//! the files selected, or all of it beside the archive (Extract here), opens
//! one file from it (double-click), and makes new 7z or zip archives (Add to
//! archive).
//!
//! - `ferro-archive [FILE]` opens the window.
//! - `ferro-archive --add PATH...` opens it at Add to archive, for those.
//! - `ferro-archive --extract-here FILE` unpacks beside it with no window
//!   (File Explorer's right-click Extract Here), printing `PROGRESS:n%`
//!   lines and then one line saying what happened. It exits 0 when done, 2
//!   when a password is needed (open the window to type it), 1 otherwise.
//!
//! Private by design: nothing is written but what's asked for (no history,
//! no recent files; only the format and level last chosen, in
//! `~/.config/ferro/archive.conf`). A file opened from an archive goes to a
//! private folder in memory (`$XDG_RUNTIME_DIR`), removed when the window
//! closes. Passwords live only in memory and are wiped when done with.

mod view;

use ferro_archive::create::{add_to, create, free_archive_name, suggested_stem, Level, NewFormat};
use ferro_archive::{extract, extract_here, free_name, human_size, is_runnable_name, open, safe_path, stem, Error, Limits, Listing, Progress};
use ferro_gfx::Surface;
use ferro_ui::{Edit, FocusKey, Key, Keymap, Res, TextEdit};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;
use view::{Dialog, Hit, PickItem, Picker, View, ADD_FIELDS, H, PICK_VISIBLE, VISIBLE, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use zeroize::Zeroizing;

type Secret = Zeroizing<String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let windowed = args.first().is_none_or(|a| a != "--extract-here");
    // Before the sandbox closes: the helper that starts the program for a
    // file opened from the archive (the sandbox can't start programs).
    let opener = if windowed { Opener::start() } else { None };
    #[cfg(target_os = "linux")]
    if let Err(e) = lock_down() {
        eprintln!("ferro-archive: can't lock itself down ({e}); not opening archives without that.");
        std::process::exit(1);
    }
    let code = match args.as_slice() {
        [flag, file] if flag == "--extract-here" => headless_extract_here(&arg_path(file)),
        [flag, rest @ ..] if flag == "--add" && !rest.is_empty() => window(None, Some(rest.iter().map(|a| arg_path(a)).collect()), opener),
        [file] if !file.starts_with("--") => window(Some(arg_path(file)), None, opener),
        [] => window(None, None, opener),
        _ => {
            eprintln!("usage: ferro-archive [FILE] | --add PATH... | --extract-here FILE");
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

// ---------------------------------------------------------------------------
// Opening one file from the archive

/// The private folder for files opened from the archive: in
/// `$XDG_RUNTIME_DIR` (memory, only ours, gone at log off), never on disk.
/// `None` without one.
fn private_base() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let run = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let m = fs::metadata(&run).ok()?;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    (m.is_dir() && m.uid() == me && m.mode() & 0o077 == 0).then(|| run.join(format!("ferro-archive-{}", std::process::id())))
}

fn private_dir(base: &Path, n: u64) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let mut b = fs::DirBuilder::new();
    b.mode(0o700);
    if !base.is_dir() {
        b.create(base)?;
    }
    let dir = base.join(n.to_string());
    b.create(&dir)?;
    Ok(dir)
}

/// The helper that starts the program for a file opened from the archive.
/// It's forked before the sandbox closes, holds nothing of the archive,
/// and only opens files in this window's private folder: no programs, no
/// shortcuts, nothing executable, even if the window were taken over.
struct Opener {
    pipe: fs::File,
    base: PathBuf,
}

impl Opener {
    fn start() -> Option<Opener> {
        use std::os::fd::FromRawFd;
        let base = private_base()?;
        let mut fds = [0i32; 2];
        // SAFETY: pipe2 fills the two-int array given.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return None;
        }
        // SAFETY: no threads exist yet (this runs first in main), so the
        // child is a whole copy; it only reads the pipe and starts programs.
        match unsafe { libc::fork() } {
            -1 => None,
            0 => {
                // SAFETY: closing our copy of the write end.
                unsafe { libc::close(fds[1]) };
                // SAFETY: fds[0] is the read end, ours alone now.
                opener_loop(unsafe { fs::File::from_raw_fd(fds[0]) }, &base)
            }
            _ => {
                // SAFETY: closing the read end the child has.
                unsafe { libc::close(fds[0]) };
                // SAFETY: fds[1] is the write end, ours alone now.
                Some(Opener { pipe: unsafe { fs::File::from_raw_fd(fds[1]) }, base })
            }
        }
    }

    fn open(&mut self, path: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let mut msg = path.as_os_str().as_bytes().to_vec();
        msg.push(0);
        self.pipe.write_all(&msg).is_ok()
    }
}

fn opener_loop(pipe: fs::File, base: &Path) -> ! {
    use std::io::BufRead;
    use std::os::unix::ffi::OsStringExt;
    let mut r = std::io::BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(0, &mut buf) {
            Ok(0) | Err(_) => std::process::exit(0),
            Ok(_) => {}
        }
        if buf.pop() != Some(0) {
            continue;
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(std::mem::take(&mut buf)));
        let (Ok(real), Ok(inside)) = (fs::canonicalize(&path), fs::canonicalize(base)) else { continue };
        let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !real.starts_with(&inside) || real == inside || is_runnable_name(&name) || !fs::symlink_metadata(&real).is_ok_and(|m| m.is_file()) {
            continue;
        }
        // Readable by us, runnable by nobody.
        use std::os::unix::fs::PermissionsExt;
        if fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).is_err() {
            continue;
        }
        ferro_associations::open_in_program(&real);
    }
}

// ---------------------------------------------------------------------------
// Settings: the format and level last chosen, and nothing else.

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/ferro/archive.conf"))
}

fn load_settings() -> (NewFormat, Level) {
    let text = settings_path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let get = |key: &str| text.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_owned()));
    let format = if get("format").as_deref() == Some("zip") { NewFormat::Zip } else { NewFormat::SevenZ };
    let level = match get("level").as_deref() {
        Some("store") => Level::Store,
        Some("maximum") => Level::Maximum,
        _ => Level::Normal,
    };
    (format, level)
}

fn save_settings(format: NewFormat, level: Level) {
    let Some(p) = settings_path() else { return };
    let level = match level {
        Level::Store => "store",
        Level::Normal => "normal",
        Level::Maximum => "maximum",
    };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(p, format!("# FerroOS Archive: the choices last made for new archives.\nformat = {}\nlevel = {level}\n", format.ext()));
}

// ---------------------------------------------------------------------------
// The window

/// What's running aside.
#[derive(Clone)]
enum Job {
    /// Into `dest`: `which` entries, or all.
    To {
        dest: PathBuf,
        which: Option<Vec<usize>>,
    },
    Here,
    /// Entry `index`, into the private folder, then opened.
    Open {
        index: usize,
    },
    /// More in the open 7z or zip (written anew beside it, then renamed over it).
    AddTo {
        archive: PathBuf,
        sources: Vec<PathBuf>,
        level: Level,
        password: Option<Secret>,
    },
    /// A new archive.
    Add {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        format: NewFormat,
        level: Level,
        password: Option<Secret>,
    },
}

/// What a job ended with, or why it didn't.
type Outcome = Result<Done, Error>;

/// What a job ended with.
enum Done {
    /// A line for the status bar.
    Said(String),
    /// Out, to be opened.
    Opened(PathBuf),
    /// A new archive: the line, and the archive (to show it).
    Made(String, PathBuf),
}

struct App {
    path: Option<PathBuf>,
    name: String,
    listing: Option<Listing>,
    password: Option<Secret>,
    selected: Vec<bool>,
    cursor: usize,
    anchor: usize,
    scroll: usize,
    focus: Option<usize>,
    status: String,
    dialog: Option<Dialog>,
    /// After the password: open, or this job again.
    after_password: Option<Option<Job>>,
    running: Option<(Arc<Progress>, Receiver<Outcome>)>,
    /// The job running (or last run), for its password and its archive.
    job: Option<Job>,
    drives: ferro_path::DriveTable,
    opener: Option<Opener>,
    opened: u64,
    /// The last left click on a row: (row, when), for double-clicks.
    last_click: Option<(usize, u32)>,
    /// The same, in the file picker.
    last_pick: Option<(usize, u32)>,
}

impl App {
    fn new(path: Option<PathBuf>, opener: Option<Opener>) -> Self {
        let mut app = App {
            path: None,
            name: String::new(),
            listing: None,
            password: None,
            selected: Vec::new(),
            cursor: 0,
            anchor: 0,
            scroll: 0,
            focus: None,
            status: "Open a .zip, .7z or .rar file from File Explorer, or add files to a new archive.".into(),
            dialog: None,
            after_password: None,
            running: None,
            job: None,
            drives: ferro_path::DriveTable::heavy(),
            opener,
            opened: 0,
            last_click: None,
            last_pick: None,
        };
        if let Some(p) = path {
            app.show_archive(p, None);
        }
        app
    }

    fn show_archive(&mut self, path: PathBuf, password: Option<Secret>) {
        self.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.path = Some(path);
        self.password = password;
        self.listing = None;
        self.selected.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.load();
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

    fn pw(&self) -> Option<&str> {
        self.password.as_ref().map(|p| p.as_str())
    }

    /// Reads the list (again, with the password once it's typed).
    fn load(&mut self) {
        let Some(path) = self.path.clone() else { return };
        match open(&path, self.pw()) {
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
        // Dropped, a password is wiped.
        self.password = None;
        self.after_password = Some(then);
        self.dialog = Some(Dialog::Password { typed: String::new(), wrong });
    }

    fn picked(&self) -> Vec<usize> {
        (0..self.selected.len()).filter(|&i| self.selected[i]).collect()
    }

    fn dir_of(path: &Path) -> PathBuf {
        path.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Extract all / selected: asks where, starting at a new folder named
    /// after the archive, beside it.
    fn ask_folder(&mut self, which: Option<Vec<usize>>) {
        let Some(path) = &self.path else { return };
        let dest = free_name(&Self::dir_of(path), stem(&self.name));
        let which = which.map(|w| self.listing.as_ref().map_or(w.clone(), |l| l.with_contents(&w)));
        self.dialog = Some(Dialog::Folder { edit: TextEdit::new(&self.shown(&dest)), which });
    }

    /// Add to archive: for `sources` (none: typed in the box).
    fn ask_add(&mut self, sources: Vec<PathBuf>) {
        let (format, level) = load_settings();
        let files = sources.iter().map(|p| self.shown(p)).collect::<Vec<_>>().join("; ");
        let dest = match sources.first() {
            Some(first) => self.shown(&free_archive_name(&Self::dir_of(first), &suggested_stem(&sources), format)),
            None => String::new(),
        };
        let field = if sources.is_empty() { 0 } else { 2 };
        // The open archive, if it can take more: a 7z or zip (never a RAR).
        let into = match (&self.listing, self.path.is_some()) {
            (Some(l), true) if l.format == ferro_archive::Format::SevenZ => Some((self.name.clone(), NewFormat::SevenZ)),
            (Some(l), true) if l.format == ferro_archive::Format::Zip => Some((self.name.clone(), NewFormat::Zip)),
            _ => None,
        };
        // Its password, if it was typed: the new files get it too.
        let password = if into.is_some() { self.password.as_ref().map(|p| p.to_string()).unwrap_or_default() } else { String::new() };
        let use_into = into.is_some();
        self.dialog = Some(Dialog::Add {
            files: TextEdit::new(&files),
            dest: TextEdit::new(&dest),
            password,
            field,
            format,
            level,
            error: None,
            into,
            use_into,
        });
    }

    /// The picker, at `dir`: the open archive's folder, or home.
    fn open_picker(&mut self) {
        let dir = match &self.path {
            Some(p) => Self::dir_of(p),
            None => std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        };
        let dir = fs::canonicalize(&dir).unwrap_or(dir);
        self.last_pick = None;
        self.dialog = Some(Dialog::Pick(self.picker_at(dir)));
    }

    /// What's in `dir` (not the hidden things), folders first, by name.
    fn picker_at(&self, dir: PathBuf) -> Picker {
        let (items, note) = match fs::read_dir(&dir) {
            Ok(rd) => {
                let mut items: Vec<PickItem> = rd
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            return None;
                        }
                        // A link to a folder opens as one; its size is its target's.
                        let m = fs::metadata(e.path()).ok()?;
                        Some(PickItem { name, is_dir: m.is_dir(), size: if m.is_dir() { 0 } else { m.len() } })
                    })
                    .collect();
                items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                (items, None)
            }
            Err(e) => (Vec::new(), Some(format!("This folder can't be read: {e}"))),
        };
        let n = items.len();
        Picker { shown: self.shown(&dir), dir, items, selected: vec![false; n], cursor: 0, anchor: 0, scroll: 0, note }
    }

    /// Into folder `dir` in the picker.
    fn pick_go(&mut self, dir: PathBuf) {
        let p = self.picker_at(dir);
        self.last_pick = None;
        self.dialog = Some(Dialog::Pick(p));
    }

    /// The picker's OK: what's picked, to the Add box.
    fn pick_ok(&mut self, p: Picker) {
        let paths: Vec<PathBuf> = p.items.iter().zip(&p.selected).filter(|(_, s)| **s).map(|(it, _)| p.dir.join(&it.name)).collect();
        if paths.is_empty() {
            self.dialog = Some(Dialog::Pick(p));
            return;
        }
        self.ask_add(paths);
    }

    /// A path typed: as Windows shows it, `~`, or as it is.
    fn typed_path(&self, typed: &str) -> PathBuf {
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

    /// Double-click: the file, out into the private folder, opened.
    fn open_entry(&mut self, index: usize) {
        let Some(e) = self.listing.as_ref().and_then(|l| l.entries.get(index)) else { return };
        if e.is_dir || e.is_link {
            return;
        }
        let name = e.name.rsplit('/').next().unwrap_or_default().to_owned();
        if is_runnable_name(&name) {
            return self.message("Open", format!("'{name}' is a program or shortcut. Programs aren't run from inside an archive: extract it first."));
        }
        if self.opener.is_none() {
            return self.message(
                "Open",
                "Files can only be opened from an archive in a private folder in memory, and there isn't one ($XDG_RUNTIME_DIR). Extract it instead."
                    .into(),
            );
        }
        self.start(Job::Open { index });
    }

    fn start(&mut self, job: Job) {
        if let Job::To { dest, .. } = &job {
            if let Err(e) = fs::create_dir_all(dest) {
                self.message("Extract", format!("Couldn't make the folder '{}': {e}", self.shown(dest)));
                return;
            }
        }
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let p = progress.clone();
        let (path, listing, pw) = (self.path.clone(), self.listing.clone(), self.password.clone());
        let shown_dest = match &job {
            Job::To { dest, .. } | Job::Add { dest, .. } => self.shown(dest),
            _ => String::new(),
        };
        let opened = self.opened;
        self.opened += 1;
        let base = self.opener.as_ref().map(|o| o.base.clone());
        let j = job.clone();
        std::thread::spawn(move || {
            let pw = pw.as_ref().map(|p| p.as_str());
            let need =
                || -> Result<(PathBuf, Listing), Error> { Ok((path.clone().ok_or(Error::Cancelled)?, listing.clone().ok_or(Error::Cancelled)?)) };
            let outcome = match j {
                Job::To { dest, which } => need().and_then(|(path, listing)| {
                    extract(&path, &listing, &dest, which.as_deref(), pw, Limits::for_dest(&dest), &p)
                        .map(|r| Done::Said(r.summary_into(Some(&shown_dest))))
                }),
                Job::Here => need().and_then(|(path, _)| extract_here(&path, pw, &p).map(|(said, _)| Done::Said(said))),
                Job::Open { index } => need().and_then(|(path, listing)| {
                    let base = base.ok_or(Error::Cancelled)?;
                    let dir = private_dir(&base, opened)?;
                    extract(&path, &listing, &dir, Some(&[index]), pw, Limits::for_dest(&dir), &p)?;
                    let rel = safe_path(&listing.entries[index].name).ok_or_else(|| Error::Io("That name isn't safe to open.".into()))?;
                    Ok(Done::Opened(dir.join(rel)))
                }),
                Job::AddTo { archive, sources, level, password } => {
                    let pw = password.as_ref().map(|p| p.as_str());
                    add_to(&archive, &sources, level, pw, &p).map(|m| {
                        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let said = format!("Added {} file(s), {}, to '{name}'.", m.files, human_size(m.bytes));
                        Done::Made(if m.skipped > 0 { format!("{said} Left out {} link(s).", m.skipped) } else { said }, archive)
                    })
                }
                Job::Add { sources, dest, format, level, password } => {
                    let pw = password.as_ref().map(|p| p.as_str());
                    create(&sources, &dest, format, level, pw, &p).map(|m| {
                        let made = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let said = format!("Made '{made}': {} file(s), {}.", m.files, human_size(m.bytes));
                        Done::Made(if m.skipped > 0 { format!("{said} Left out {} link(s).", m.skipped) } else { said }, dest)
                    })
                }
            };
            let _ = tx.send(outcome);
        });
        let title = match &job {
            Job::Add { .. } | Job::AddTo { .. } => "Adding",
            Job::Open { .. } => "Opening",
            _ => "Extracting",
        };
        self.after_password = None;
        self.job = Some(job);
        self.running = Some((progress, rx));
        self.dialog = Some(Dialog::Progress { title: title.into(), percent: 0, done: 0, total: 0, current: String::new(), cancelling: false });
    }

    /// Checks on the running job. Whether anything changed.
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
                self.status = "It stopped unexpectedly.".into();
                true
            }
            Ok(outcome) => {
                self.running = None;
                self.dialog = None;
                let job = self.job.take();
                match outcome {
                    Ok(Done::Said(said)) => self.status = said,
                    Ok(Done::Opened(file)) => {
                        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.status = match self.opener.as_mut().map(|o| o.open(&file)) {
                            Some(true) => format!("Opened '{name}' (a copy in memory, gone when this window closes)."),
                            _ => format!("Couldn't open '{name}'."),
                        };
                    }
                    Ok(Done::Made(said, archive)) => {
                        let pw = match job {
                            Some(Job::Add { password, .. }) | Some(Job::AddTo { password, .. }) => password,
                            _ => None,
                        };
                        self.show_archive(archive, pw);
                        self.status = said;
                    }
                    // A new archive's password was the one typed for it: no asking.
                    Err(e @ (Error::NeedPassword | Error::WrongPassword)) if !matches!(job, Some(Job::Add { .. }) | Some(Job::AddTo { .. })) => {
                        self.ask_password(job, e == Error::WrongPassword)
                    }
                    Err(Error::Cancelled) => {
                        self.status = match job {
                            Some(Job::Add { .. }) => "Cancelled. No archive was made.".into(),
                            Some(Job::AddTo { .. }) => "Cancelled. The archive is as it was.".into(),
                            _ => "Cancelled. What was extracted whole is kept.".into(),
                        }
                    }
                    Err(e) => {
                        self.status = "It stopped.".into();
                        let title = if matches!(job, Some(Job::Add { .. }) | Some(Job::AddTo { .. })) { "Add to archive" } else { "Extract" };
                        if let (Some(Job::AddTo { .. }), Error::NeedPassword | Error::WrongPassword) = (&job, &e) {
                            return {
                                self.message(
                                    title,
                                    "The archive's password is needed to add to it (or isn't right). Type it in the Add box's password field.".into(),
                                );
                                true
                            };
                        }
                        self.message(title, e.to_string());
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

    /// OK in the Add box: checks what was typed, then makes it.
    #[allow(clippy::too_many_arguments)]
    fn add_ok(
        &mut self,
        files: TextEdit,
        dest: TextEdit,
        mut password: String,
        field: usize,
        format: NewFormat,
        level: Level,
        into: Option<(String, NewFormat)>,
        use_into: bool,
    ) {
        let sources: Vec<PathBuf> = files.text().split(';').map(str::trim).filter(|s| !s.is_empty()).map(|s| self.typed_path(s)).collect();
        if use_into && into.is_some() {
            let error = if sources.is_empty() {
                Some("Pick or type the files or folders to add.".to_owned())
            } else {
                sources.iter().find(|p| fs::symlink_metadata(p).is_err()).map(|m| format!("'{}' isn't there.", self.shown(m)))
            };
            if let (None, Some(archive)) = (&error, self.path.clone()) {
                let secret = (!password.is_empty()).then(|| Zeroizing::new(std::mem::take(&mut password)));
                ferro_ui::forget(&mut password);
                self.start(Job::AddTo { archive, sources, level, password: secret });
            } else {
                self.dialog = Some(Dialog::Add { files, dest, password, field, format, level, error, into, use_into });
            }
            return;
        }
        let mut target = self.typed_path(dest.text());
        if !dest.text().trim().is_empty() && target.extension().is_none_or(|e| e != format.ext()) {
            let mut s = target.into_os_string();
            s.push(format!(".{}", format.ext()));
            target = PathBuf::from(s);
        }
        let error = if sources.is_empty() {
            Some("Type the files or folders to add.".to_owned())
        } else if let Some(missing) = sources.iter().find(|p| fs::symlink_metadata(p).is_err()) {
            Some(format!("'{}' isn't there.", self.shown(missing)))
        } else if dest.text().trim().is_empty() {
            Some("Type a name for the archive.".to_owned())
        } else if fs::symlink_metadata(&target).is_ok() {
            Some("There's already something by that name.".to_owned())
        } else if !App::dir_of(&target).is_dir() {
            Some("The folder for the archive isn't there.".to_owned())
        } else {
            None
        };
        if error.is_some() {
            self.dialog = Some(Dialog::Add { files, dest, password, field, format, level, error, into, use_into });
            return;
        }
        save_settings(format, level);
        let secret = (!password.is_empty()).then(|| Zeroizing::new(std::mem::take(&mut password)));
        ferro_ui::forget(&mut password);
        self.start(Job::Add { sources, dest: target, format, level, password: secret });
    }

    /// OK in the box over the window.
    fn dialog_ok(&mut self) {
        match self.dialog.take() {
            Some(Dialog::Folder { edit, which }) => {
                if edit.text().trim().is_empty() {
                    self.dialog = Some(Dialog::Folder { edit, which });
                    return;
                }
                let dest = self.typed_path(edit.text());
                self.start(Job::To { dest, which });
            }
            Some(Dialog::Password { typed, .. }) => {
                self.password = Some(Zeroizing::new(typed));
                match self.after_password.take() {
                    Some(Some(job)) => self.start(job),
                    _ => self.load(),
                }
            }
            Some(Dialog::Add { files, dest, password, field, format, level, into, use_into, .. }) => {
                self.add_ok(files, dest, password, field, format, level, into, use_into)
            }
            Some(Dialog::Pick(p)) => self.pick_ok(p),
            Some(Dialog::Progress { .. }) | Some(Dialog::Message { .. }) | None => {}
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
            Some(Dialog::Add { mut password, .. }) => ferro_ui::forget(&mut password),
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

    /// A click (or a key handed on as one) at `time`.
    fn click(&mut self, x: i32, y: i32, ctrl: bool, shift: bool, time: u32) {
        match view::hit(x, y, &self.view()) {
            Hit::ExtractAll => self.ask_folder(None),
            Hit::ExtractSelected => {
                let p = self.picked();
                self.ask_folder(Some(p));
            }
            Hit::ExtractHere => self.start(Job::Here),
            Hit::AddToArchive => self.open_picker(),
            Hit::Row(i) => {
                let double = !ctrl && !shift && time != 0 && self.last_click.is_some_and(|(r, t)| r == i && time.wrapping_sub(t) < 500);
                self.last_click = Some((i, time));
                self.click_row(i, ctrl, shift);
                if double {
                    self.last_click = None;
                    self.open_entry(i);
                }
            }
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
            Hit::AddField(i) => {
                if let Some(Dialog::Add { files, dest, field, .. }) = &mut self.dialog {
                    *field = i;
                    match i {
                        0 => files.click(ADD_FIELDS[0].inset(2), x, shift),
                        1 => dest.click(ADD_FIELDS[1].inset(2), x, shift),
                        _ => {}
                    }
                }
            }
            Hit::AddFormat(f) => {
                if let Some(Dialog::Add { dest, format, use_into: false, .. }) = &mut self.dialog {
                    // The name follows the format.
                    let old = format!(".{}", format.ext());
                    if let Some(base) = dest.text().strip_suffix(&old) {
                        *dest = TextEdit::new(&format!("{base}.{}", f.ext()));
                    }
                    *format = f;
                }
            }
            Hit::AddLevel(l) => {
                if let Some(Dialog::Add { level, .. }) = &mut self.dialog {
                    *level = l;
                }
            }
            Hit::AddInto(on) => {
                if let Some(Dialog::Add { use_into, field, .. }) = &mut self.dialog {
                    *use_into = on;
                    if !on && *field == 0 {
                        *field = 1;
                    }
                }
            }
            Hit::PickUp => {
                if let Some(Dialog::Pick(p)) = &self.dialog {
                    if let Some(up) = p.dir.parent().map(Path::to_path_buf) {
                        self.pick_go(up);
                    }
                }
            }
            Hit::PickScroll(d) => {
                if let Some(Dialog::Pick(p)) = &mut self.dialog {
                    let max = p.items.len().saturating_sub(PICK_VISIBLE);
                    p.scroll = (p.scroll as isize + d).clamp(0, max as isize) as usize;
                }
            }
            Hit::PickRow(i) => {
                let double = !ctrl && !shift && time != 0 && self.last_pick.is_some_and(|(r, t)| r == i && time.wrapping_sub(t) < 500);
                self.last_pick = Some((i, time));
                if let Some(Dialog::Pick(p)) = &mut self.dialog {
                    pick_select(p, i, ctrl, shift);
                    if double && p.items[i].is_dir {
                        let dir = p.dir.join(&p.items[i].name);
                        self.pick_go(dir);
                    }
                }
            }
            Hit::Nothing => {}
        }
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
            Some(Dialog::Add { files, dest, password, field, .. }) => match key {
                Key::Enter => self.dialog_ok(),
                Key::Escape => self.dialog_cancel(),
                Key::Next => *field = ferro_ui::focus_step(Some(*field), ADD_FIELDS.len(), shift).unwrap_or(0),
                _ if *field == 2 => match key {
                    Key::Backspace => {
                        password.pop();
                    }
                    Key::Char(c) if !c.is_control() && !ctrl && password.len() < 256 => password.push(c),
                    _ => {}
                },
                _ => {
                    let edit = if *field == 0 { files } else { dest };
                    edit.key(key, ctrl, shift);
                }
            },
            Some(Dialog::Pick(p)) => {
                let n = p.items.len();
                match key {
                    Key::Escape => self.dialog = None,
                    Key::Backspace => {
                        if let Some(up) = p.dir.parent().map(Path::to_path_buf) {
                            self.pick_go(up);
                        }
                    }
                    // Enter on a folder alone opens it; otherwise it's Add.
                    Key::Enter => {
                        let only = p.selected.iter().filter(|s| **s).count() == 1 && p.selected.get(p.cursor) == Some(&true);
                        if only && p.items[p.cursor].is_dir && !ctrl {
                            let dir = p.dir.join(&p.items[p.cursor].name);
                            self.pick_go(dir);
                        } else {
                            self.dialog_ok();
                        }
                    }
                    Key::Char(c) if ctrl && c.eq_ignore_ascii_case(&'a') => p.selected.iter_mut().for_each(|s| *s = true),
                    Key::Char(' ') if ctrl && n > 0 => pick_select(p, p.cursor, true, false),
                    Key::Up | Key::Down | Key::Home | Key::End if n > 0 => {
                        let to = match key {
                            Key::Up => p.cursor.saturating_sub(1),
                            Key::Down => (p.cursor + 1).min(n - 1),
                            Key::Home => 0,
                            _ => n - 1,
                        };
                        if ctrl {
                            p.cursor = to;
                        } else {
                            pick_select(p, to, false, shift);
                        }
                        if p.cursor < p.scroll {
                            p.scroll = p.cursor;
                        } else if p.cursor >= p.scroll + PICK_VISIBLE {
                            p.scroll = p.cursor + 1 - PICK_VISIBLE;
                        }
                    }
                    _ => {}
                }
            }
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

fn window(path: Option<PathBuf>, add: Option<Vec<PathBuf>>, opener: Option<Opener>) -> i32 {
    let base = opener.as_ref().map(|o| o.base.clone());
    let code = match run(path, add, opener) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ferro-archive: {e}");
            1
        }
    };
    // Files opened from the archive go with the window.
    if let Some(b) = base {
        let _ = fs::remove_dir_all(b);
    }
    code
}

fn run(path: Option<PathBuf>, add: Option<Vec<PathBuf>>, opener: Option<Opener>) -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
    let keymap = Keymap::load(&conn)?;
    let mut app = App::new(path, opener);
    if let Some(sources) = add {
        app.ask_add(sources);
    }

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
    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    let set_title = |name: &str| -> Res<()> {
        let title = if name.is_empty() { "Archive".to_owned() } else { format!("{name} - Archive") };
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8, title.as_bytes())?;
        Ok(())
    };
    set_title(&app.name)?;
    let mut titled = app.name.clone();
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
    // Closed while working: once it has stopped.
    let mut closing = false;
    loop {
        if closing && app.running.is_none() {
            break;
        }
        if titled != app.name {
            set_title(&app.name)?;
            titled = app.name.clone();
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
                    4 | 5 if matches!(app.dialog, Some(Dialog::Pick(_))) => {
                        if let Some(Dialog::Pick(p)) = &mut app.dialog {
                            let max = p.items.len().saturating_sub(PICK_VISIBLE) as isize;
                            p.scroll = (p.scroll as isize + if e.detail == 4 { -3 } else { 3 }).clamp(0, max) as usize;
                        }
                    }
                    1 => app.click(i32::from(e.event_x), i32::from(e.event_y), ctrl, shift, e.time),
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
    // Wiped as they're dropped.
    app.password = None;
    app.job = None;
    Ok(())
}

/// A click on picker row `i`: alone, or with Ctrl (one more) or Shift (a run).
fn pick_select(p: &mut Picker, i: usize, ctrl: bool, shift: bool) {
    if shift {
        let (a, b) = (p.anchor.min(i), p.anchor.max(i));
        if !ctrl {
            p.selected.iter_mut().for_each(|s| *s = false);
        }
        for s in &mut p.selected[a..=b] {
            *s = true;
        }
    } else if ctrl {
        p.selected[i] = !p.selected[i];
        p.anchor = i;
    } else {
        p.selected.iter_mut().for_each(|s| *s = false);
        p.selected[i] = true;
        p.anchor = i;
    }
    p.cursor = i;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(n: usize) -> Picker {
        Picker {
            dir: "/x".into(),
            shown: String::new(),
            items: (0..n).map(|i| PickItem { name: format!("f{i}"), is_dir: false, size: 0 }).collect(),
            selected: vec![false; n],
            cursor: 0,
            anchor: 0,
            scroll: 0,
            note: None,
        }
    }

    #[test]
    fn picking_like_windows() {
        let mut p = picker(6);
        pick_select(&mut p, 1, false, false);
        pick_select(&mut p, 4, false, true);
        assert_eq!(p.selected, [false, true, true, true, true, false]);
        pick_select(&mut p, 2, true, false);
        assert_eq!(p.selected, [false, true, false, true, true, false]);
        pick_select(&mut p, 5, false, false);
        assert_eq!(p.selected, [false, false, false, false, false, true]);
        assert_eq!(p.cursor, 5);
    }
}
