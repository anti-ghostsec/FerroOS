//! ferro-share: the Share box. A program opens it with what to share; the
//! user picks who gets it; the program picked gets a copy in memory. See
//! the library (`lib.rs`) for what it keeps private, and how.
//!
//!   ferro-share [--] FILE...      files (each read into memory)
//!   ferro-share --stdin NAME      what comes on stdin, as a file named NAME
//!   ferro-share --text TEXT       text (`-`: from stdin)
//!   ferro-share --url URL         a link
//!
//! It always ends with 0 once the box is closed, shared or not: the
//! program sharing isn't told.

use ferro_gfx::{color, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_share::{Item, Share, Target};
use ferro_ui::{button, default_button, dialog_answer, Key, Keymap, Res};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 360;
const H: i32 = 272;
const ROW_H: i32 = 18;
const ROWS: usize = 8;
const LIST: Rect = Rect::new(12, 30, W - 24, ROW_H * ROWS as i32 + 4);
const STRIP_BOX: Rect = Rect::new(12, 186, W - 24, 16);
const COPY_BTN: Rect = Rect::new(12, H - 36, 75, 24);
const SHARE_BTN: Rect = Rect::new(W - 172, H - 36, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 87, H - 36, 75, 24);

/// After the program picked closed without reading what it got.
const UNREAD: Duration = Duration::from_secs(10 * 60);
/// After it was read, with the program still open and nothing reading it.
const READ: Duration = Duration::from_secs(30 * 60);

struct State {
    share: Option<Share>,
    targets: Vec<Target>,
    selected: Option<usize>,
    scroll: usize,
    strip: bool,
    images: bool,
    note: Option<String>,
    last_click: Option<(usize, Instant)>,
}

impl State {
    fn new(share: Result<Share, String>) -> State {
        let (share, note) = match share {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let targets = share.as_ref().map(|s| ferro_share::targets(s, &entry_dirs())).unwrap_or_default();
        let images = matches!(&share, Some(Share::Files(items)) if items.iter().any(Item::strippable));
        let note = note.or_else(|| (share.is_some() && targets.is_empty()).then(|| "No program here takes this.".to_owned()));
        let selected = (!targets.is_empty()).then_some(0);
        State { share, targets, selected, scroll: 0, strip: true, images, note, last_click: None }
    }

    fn copyable(&self) -> Option<String> {
        match &self.share {
            Some(Share::Url(u)) => Some(u.clone()),
            Some(Share::Files(items)) if items.len() == 1 && items[0].name == "Shared text.txt" => {
                Some(String::from_utf8_lossy(&items[0].data).into_owned())
            }
            _ => None,
        }
    }

    fn select(&mut self, i: usize) {
        if self.targets.is_empty() {
            return;
        }
        let i = i.min(self.targets.len() - 1);
        self.selected = Some(i);
        if i < self.scroll {
            self.scroll = i;
        } else if i >= self.scroll + ROWS {
            self.scroll = i + 1 - ROWS;
        }
    }
}

/// The user's own `.desktop` entries by the home the system has for them,
/// not $HOME: whatever opened the box can't point it at programs of its own.
fn entry_dirs() -> Vec<PathBuf> {
    let mut dirs = ferro_share::entry_dirs();
    if let Some(home) = real_home() {
        dirs[0] = home.join(".local/share/applications");
    }
    dirs
}

fn real_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid's result is used before any other passwd call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
    }
}

/// Where shares go: the user's runtime folder (RAM) by their uid, not by
/// what the environment says.
fn share_root() -> Option<PathBuf> {
    // SAFETY: getuid has no preconditions.
    let own = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
    let runtime = if own.is_dir() { own } else { PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?) };
    ferro_share::in_memory(&runtime).then(|| runtime.join("ferro-share"))
}

/// Room left in memory there, in bytes.
fn room(dir: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return 0 };
    // SAFETY: a zeroed statvfs is a valid buffer; the path lives through the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    #[allow(clippy::unnecessary_cast)]
    let n = st.f_bavail as u64 * st.f_frsize as u64;
    n
}

fn read_limited(r: &mut dyn Read) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    r.take(ferro_share::MAX_BYTES + 1).read_to_end(&mut data).map_err(|e| format!("It couldn't be read: {e}"))?;
    if data.len() as u64 > ferro_share::MAX_BYTES {
        return Err("It's too big to share through memory (512 MB at most).".into());
    }
    Ok(data)
}

/// What the command line asks to share, read into memory.
fn what(args: &[String]) -> Result<Share, String> {
    match args.first().map(String::as_str) {
        Some("--stdin") => {
            let name = args.get(1).ok_or("--stdin needs a name")?;
            Ok(Share::Files(vec![Item::new(name, read_limited(&mut std::io::stdin())?)]))
        }
        Some("--text") => {
            let text = match args.get(1).map(String::as_str) {
                Some("-") | None => String::from_utf8_lossy(&read_limited(&mut std::io::stdin())?).into_owned(),
                Some(t) => t.to_owned(),
            };
            Ok(Share::text(&text))
        }
        Some("--url") => Share::url(args.get(1).ok_or("--url needs a link")?),
        _ => {
            let paths: Vec<&String> = args.iter().filter(|a| a.as_str() != "--").collect();
            if paths.is_empty() {
                return Err("Nothing was given to share.".into());
            }
            let mut items = Vec::new();
            let mut total = 0u64;
            for p in paths {
                let path = Path::new(p);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if path.is_dir() {
                    return Err(format!("'{name}' is a folder: only files can be shared."));
                }
                let mut f = std::fs::File::open(path).map_err(|e| format!("'{name}' couldn't be opened: {e}"))?;
                let data = read_limited(&mut f)?;
                total += data.len() as u64;
                if total > ferro_share::MAX_BYTES {
                    return Err("Together they're too big to share through memory (512 MB at most).".into());
                }
                items.push(Item::new(&name, data));
            }
            Ok(Share::Files(items))
        }
    }
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

fn row_rect(i: usize) -> Rect {
    Rect::new(LIST.x + 2, LIST.y + 2 + i as i32 * ROW_H, LIST.w - 4, ROW_H)
}

fn draw(s: &mut Surface, st: &State) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let room = ((W - 24) / GLYPH_W).max(1) as usize;
    let what = st.share.as_ref().map_or_else(|| "this".to_owned(), Share::describe);
    s.text(12, 14, &ferro_gfx::ellipsize(&format!("Share {what} with:"), room), color::BLACK);

    s.fill(LIST, color::WHITE);
    s.bevel(LIST, Bevel::Sunken);
    for (row, i) in (st.scroll..st.targets.len()).take(ROWS).enumerate() {
        let r = row_rect(row);
        let on = st.selected == Some(i);
        if on {
            s.fill(r, color::NAVY);
        }
        let name = ferro_gfx::ellipsize(&st.targets[i].name, ((r.w - 12) / GLYPH_W).max(1) as usize);
        s.text(r.x + 6, r.y + (ROW_H - GLYPH_H) / 2, &name, if on { color::WHITE } else { color::BLACK });
    }
    if st.targets.len() > ROWS {
        let more = format!("{}-{} of {}", st.scroll + 1, (st.scroll + ROWS).min(st.targets.len()), st.targets.len());
        s.text(LIST.right() - ferro_gfx::text_width(&more) - 6, LIST.bottom() + 4, &more, color::SHADOW);
    }

    if st.images {
        checkbox(s, STRIP_BOX, "Remove location and camera details", st.strip);
    }
    match &st.note {
        Some(n) => {
            s.text(12, 212, &ferro_gfx::ellipsize(n, room), color::RED);
        }
        None => {
            s.text(12, 212, &ferro_gfx::ellipsize("It gets a copy, kept in memory only.", room), color::SHADOW);
        }
    }

    if st.copyable().is_some() {
        button(s, COPY_BTN, "Copy", true);
    }
    default_button(s, SHARE_BTN, "Share", st.selected.is_some());
    button(s, CANCEL_BTN, "Cancel", true);
}

/// The environment the program picked starts with: the desktop's, not the
/// sharer's (no LD_PRELOAD or PATH of its choosing).
fn clean_env(cmd: &mut Command) {
    cmd.env_clear();
    for k in ["DISPLAY", "XAUTHORITY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "LANG", "LC_ALL", "LANGUAGE", "USER", "LOGNAME", "SHELL"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    let home = real_home().unwrap_or_else(|| PathBuf::from("/"));
    cmd.env("HOME", &home).current_dir(&home);
    cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin");
}

fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    ["/usr/local/bin", "/usr/bin", "/bin", "/usr/local/sbin", "/usr/sbin", "/sbin"].iter().any(|d| Path::new(d).join(program).is_file())
}

/// What's handed over once a program is picked: the folder of copies (none
/// for a link) and the command lines that start the program with them.
struct Plan {
    dir: Option<PathBuf>,
    commands: Vec<Vec<String>>,
}

/// The copies made (in memory, pictures cleaned if ticked), while the box
/// is still up to say what went wrong.
fn prepare(st: &State, target: &Target) -> Result<Plan, String> {
    let share = st.share.as_ref().ok_or("Nothing to share.")?;
    let (dir, args) = match share {
        Share::Url(u) => (None, vec![u.clone()]),
        Share::Files(items) => {
            let root = share_root().ok_or("There's no folder in memory to share through, so nothing was shared.")?;
            let mut items = items.clone();
            if st.strip {
                for it in &mut items {
                    if let Ok(clean) = ferro_meta::strip(&it.data) {
                        it.data = clean.data;
                    }
                }
            }
            let need: u64 = items.iter().map(|i| i.data.len() as u64).sum::<u64>() + 64 * 1024 * 1024;
            let _ = std::fs::create_dir_all(&root);
            if room(&root) < need {
                return Err("There isn't enough free memory to share that now.".into());
            }
            ferro_share::sweep(&root);
            let (dir, paths) = ferro_share::stage(&root, &items).map_err(|e| format!("It couldn't be put in memory: {e}"))?;
            (Some(dir), paths.iter().map(|p| p.to_string_lossy().into_owned()).collect())
        }
    };
    let commands: Vec<Vec<String>> = target.commands(&args).into_iter().filter(|c| !c.is_empty()).collect();
    if commands.is_empty() || !commands.iter().all(|c| on_path(&c[0])) {
        if let Some(d) = &dir {
            ferro_share::remove(d);
        }
        return Err(format!("{} couldn't be started.", target.name));
    }
    Ok(Plan { dir, commands })
}

/// Forked off, so whatever opened the box goes on: the program started,
/// and the copies deleted when they've been read.
fn hand_over(plan: Plan) {
    let Plan { dir, commands } = plan;
    // SAFETY: one thread (the X connection is closed by now); the child
    // only spawns, waits and deletes.
    match unsafe { libc::fork() } {
        -1 => {
            if let Some(d) = &dir {
                ferro_share::remove(d);
            }
            eprintln!("ferro-share: it couldn't be handed over");
        }
        0 => {
            // SAFETY: setsid has no preconditions; out of the sharer's session.
            unsafe { libc::setsid() };
            let children: Vec<Child> = commands
                .iter()
                .filter_map(|c| {
                    let mut cmd = Command::new(&c[0]);
                    cmd.args(&c[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    clean_env(&mut cmd);
                    cmd.spawn().ok()
                })
                .collect();
            if let Some(d) = dir {
                watch_then_remove(&d, children);
            }
            std::process::exit(0);
        }
        _ => {}
    }
}

/// Waits until what was shared has been read and nothing is reading it
/// (and the program has closed, or long enough has gone by), then deletes
/// it. Unread, it goes ten minutes after the program closes.
fn watch_then_remove(dir: &Path, mut children: Vec<Child>) {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: plain inotify calls on a descriptor of our own.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    let watched = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok().is_some_and(|c| {
        fd >= 0 && unsafe { libc::inotify_add_watch(fd, c.as_ptr(), libc::IN_OPEN | libc::IN_CLOSE_NOWRITE | libc::IN_CLOSE_WRITE) } >= 0
    });
    let (mut opens, mut closes) = (0u64, 0u64);
    let mut last_close = Instant::now();
    let mut gone_at: Option<Instant> = None;
    let mut buf = [0u8; 4096];
    loop {
        if watched {
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one live pollfd.
            unsafe { libc::poll(&mut p, 1, 1000) };
            loop {
                // SAFETY: reads into a live buffer of the size given; non-blocking.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                let mut off = 0usize;
                while off + 16 <= n as usize {
                    let mask = u32::from_ne_bytes(buf[off + 4..off + 8].try_into().unwrap_or_default());
                    let len = u32::from_ne_bytes(buf[off + 12..off + 16].try_into().unwrap_or_default()) as usize;
                    if mask & libc::IN_ISDIR == 0 && len > 0 {
                        if mask & libc::IN_OPEN != 0 {
                            opens += 1;
                        }
                        if mask & (libc::IN_CLOSE_NOWRITE | libc::IN_CLOSE_WRITE) != 0 {
                            closes += 1;
                            last_close = Instant::now();
                        }
                    }
                    off += 16 + len;
                }
            }
        } else {
            std::thread::sleep(Duration::from_secs(1));
        }
        children.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_)) | Err(_)));
        if children.is_empty() && gone_at.is_none() {
            gone_at = Some(Instant::now());
        }
        let quiet = opens > 0 && closes >= opens;
        let done = match gone_at {
            Some(t) => quiet || t.elapsed() > UNREAD || !watched,
            None => quiet && last_close.elapsed() > READ,
        };
        if done {
            break;
        }
    }
    ferro_share::remove(dir);
}

fn copy_to_clipboard(text: &str) -> bool {
    let Ok(mut child) = Command::new("xclip").args(["-selection", "clipboard", "-in"]).stdin(Stdio::piped()).spawn() else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!("Usage: ferro-share [--] FILE... | --stdin NAME | --text TEXT|- | --url URL");
        return Ok(());
    }
    let mut st = State::new(what(&args));

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
    let atom = |name: &[u8]| -> Res<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let (wtype, wtype_dialog) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?);
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Share")?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, b"Share")?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-share\0ferro-share\0")?;
    conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[wtype_dialog])?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut surface = Surface::new(W as usize, H as usize);
    let paint = |s: &mut Surface, st: &State| -> Res<()> {
        draw(s, st);
        ferro_ui::show(&conn, win, gc, screen.root_depth, s, W, H)
    };
    paint(&mut surface, &st)?;

    // What closing the box does: the program picked, or nothing.
    let mut plan: Option<Plan> = None;
    loop {
        let mut go = false;
        match conn.wait_for_event()? {
            Event::Expose(_) => {}
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => break,
            Event::ButtonPress(bp) => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                match bp.detail {
                    4 => st.scroll = st.scroll.saturating_sub(1),
                    5 => st.scroll = (st.scroll + 1).min(st.targets.len().saturating_sub(ROWS)),
                    1 if LIST.contains(x, y) => {
                        let i = st.scroll + ((y - LIST.y - 2).max(0) / ROW_H) as usize;
                        if i < st.targets.len() {
                            let double = st.last_click.is_some_and(|(j, t)| j == i && t.elapsed() < Duration::from_millis(450));
                            st.select(i);
                            st.last_click = Some((i, Instant::now()));
                            go = double;
                        }
                    }
                    1 if st.images && STRIP_BOX.contains(x, y) => st.strip = !st.strip,
                    1 if SHARE_BTN.contains(x, y) => go = true,
                    1 if CANCEL_BTN.contains(x, y) => break,
                    1 if COPY_BTN.contains(x, y) => {
                        if let Some(text) = st.copyable() {
                            if copy_to_clipboard(&text) {
                                break;
                            }
                            st.note = Some("It couldn't be copied.".into());
                            ferro_ui::sound("error");
                        }
                    }
                    _ => {}
                }
            }
            Event::KeyPress(kp) => {
                let k = keymap.typed(&kp);
                match k {
                    _ if dialog_answer(k) == Some(true) => go = true,
                    _ if dialog_answer(k) == Some(false) => break,
                    Key::Up => st.select(st.selected.map_or(0, |i| i.saturating_sub(1))),
                    Key::Down => st.select(st.selected.map_or(0, |i| i + 1)),
                    Key::Home => st.select(0),
                    Key::End => st.select(usize::MAX),
                    Key::Char(' ') if st.images => st.strip = !st.strip,
                    _ => {}
                }
            }
            _ => {}
        }
        if let Some(i) = st.selected.filter(|_| go) {
            match prepare(&st, &st.targets[i]) {
                Ok(p) => {
                    plan = Some(p);
                    break;
                }
                Err(why) => {
                    st.note = Some(why);
                    ferro_ui::sound("error");
                }
            }
        }
        paint(&mut surface, &st)?;
    }

    let Some(plan) = plan else { return Ok(()) };
    // The box goes first: the program picked shouldn't open behind it.
    conn.destroy_window(win)?;
    conn.flush()?;
    drop(conn);
    hand_over(plan);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_go_once_read_and_the_program_is_done() {
        let root = share_root().unwrap_or_else(|| std::env::temp_dir().join("ferro-share-test"));
        let (dir, paths) = ferro_share::stage(&root.join(format!("test-{}", std::process::id())), &[Item::new("a.txt", b"hello".to_vec())]).unwrap();
        let out = Command::new("cat").arg(&paths[0]).stdout(Stdio::null()).spawn().unwrap();
        let started = Instant::now();
        watch_then_remove(&dir, vec![out]);
        assert!(!dir.exists());
        assert!(started.elapsed() < Duration::from_secs(5), "gone as soon as it was read");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn the_box_takes_what_it_is_given() {
        assert!(matches!(what(&["--url".into(), "https://example.org".into()]), Ok(Share::Url(_))));
        assert!(what(&["--url".into(), "-x".into()]).is_err());
        assert!(what(&["--".into(), "/etc".into()]).unwrap_err().contains("folder"));
        assert!(what(&[]).is_err());
        assert_eq!(what(&["--text".into(), "hi".into()]), Ok(Share::text("hi")));
    }
}
