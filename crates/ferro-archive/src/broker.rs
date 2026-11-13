//! The jobs that change files (extracting, making and adding to archives,
//! taking out a file to open, the settings), each in a process of its own
//! that may change files in the one folder it works in, and nowhere else.
//!
//! The window can't: it reads archives from anywhere, and one made to take
//! it over must find nothing to write to (not .bashrc, not the Start menu,
//! not which program opens a file). Rights can't be given to a process
//! after it's locked down, only taken away, so they come from the broker:
//! a process forked before the window locks itself down, which starts a
//! worker for each job and does nothing else. It never reads what the
//! window sends. The worker reads the job, makes the folder it needs, and
//! checks that it isn't the home folder, one of its hidden folders or
//! FerroOS's settings (that's refused, with the reason); then it locks
//! itself into that folder, and only then opens the archive.
//!
//! A job is a socket pair: the window keeps one end and passes the other to
//! the broker, which hands it to the worker. Down it go the job, then
//! progress (and Cancel) both ways, then how it ended.

use crate::{Done, Secret};
use ferro_archive::create::{add_to, create, Level, NewFormat};
use ferro_archive::{extract, extract_here, human_size, open, safe_path, Error, Limits, Progress};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeroize::Zeroizing;

/// A job, as the window asks for it.
pub enum Request {
    /// Into `dest` (`shown` as the person sees it): `which` entries, or all.
    To {
        archive: PathBuf,
        password: Option<Secret>,
        dest: PathBuf,
        shown: String,
        which: Option<Vec<usize>>,
    },
    Here {
        archive: PathBuf,
        password: Option<Secret>,
    },
    /// Entry `index`, into the private folder's folder number `n`.
    Open {
        archive: PathBuf,
        password: Option<Secret>,
        index: usize,
        n: u64,
    },
    AddTo {
        archive: PathBuf,
        sources: Vec<PathBuf>,
        level: Level,
        password: Option<Secret>,
    },
    Add {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        format: NewFormat,
        level: Level,
        password: Option<Secret>,
    },
    Settings {
        format: NewFormat,
        level: Level,
    },
}

pub struct Broker {
    ctl: UnixStream,
}

impl Broker {
    /// Forks the broker. Must run before any thread is started and before
    /// the window locks itself down. `base` is the private folder for files
    /// taken out to open (removed when the window's gone).
    pub fn start(base: Option<PathBuf>) -> Option<Broker> {
        let (ours, theirs) = UnixStream::pair().ok()?;
        // SAFETY: no threads exist yet (this runs first in main), so the
        // child is a whole copy; it only takes sockets and forks.
        match unsafe { libc::fork() } {
            -1 => None,
            0 => {
                drop(ours);
                broker_loop(&theirs, base.as_deref())
            }
            _ => {
                drop(theirs);
                Some(Broker { ctl: ours })
            }
        }
    }

    /// Runs a job in a worker, keeping `progress` up to date (and passing
    /// its Cancel on). How it ended, or why it couldn't start.
    pub fn run(&self, request: &Request, progress: &Arc<Progress>) -> Result<Done, Error> {
        let lost = || Error::Io("The part of FerroOS Archive that does this has stopped. Close the window and open it again.".into());
        let (mut ours, theirs) = UnixStream::pair().map_err(|e| Error::Io(e.to_string()))?;
        ferro_sandbox::linux::send_fd(self.ctl.as_raw_fd(), theirs.as_raw_fd()).map_err(|_| lost())?;
        drop(theirs);
        send(&mut ours, b'Q', &encode(request)).map_err(|_| lost())?;
        // Cancel, passed on while the job runs.
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(mut w) = ours.try_clone() {
            let (p, done) = (progress.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if p.cancel.load(Ordering::Relaxed) {
                        let _ = send(&mut w, b'C', &[]);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
        }
        let outcome = loop {
            let Ok((tag, fields)) = recv(&mut ours) else { break Err(lost()) };
            match tag {
                b'P' => {
                    let num = |i: usize| fields.get(i).and_then(|f| std::str::from_utf8(f).ok()?.parse().ok()).unwrap_or(0);
                    progress.done.store(num(0), Ordering::Relaxed);
                    progress.total.store(num(1), Ordering::Relaxed);
                    if let (Some(c), Ok(mut cur)) = (fields.get(2), progress.current.lock()) {
                        *cur = String::from_utf8_lossy(c).into_owned();
                    }
                }
                b'R' => break decode_outcome(&fields),
                _ => break Err(lost()),
            }
        };
        done.store(true, Ordering::Relaxed);
        outcome
    }
}

// ---- the broker and its workers ---------------------------------------------

fn broker_loop(ctl: &UnixStream, base: Option<&Path>) -> ! {
    // Workers are reaped by the kernel.
    // SAFETY: setting a signal's disposition to SIG_IGN.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
    let protected = protect_broker();
    while let Ok(fd) = ferro_sandbox::linux::recv_fd(ctl.as_raw_fd()) {
        // SAFETY: the broker has one thread: the child is a whole copy.
        match unsafe { libc::fork() } {
            0 => {
                // SAFETY: fd was just received, and is ours alone.
                let job = unsafe { UnixStream::from_raw_fd(fd) };
                worker(job, base, protected)
            }
            _ => {
                // SAFETY: the worker has its own copy.
                unsafe { libc::close(fd) };
            }
        }
    }
    // The window's gone: what it opened from archives goes with it.
    if let Some(b) = base {
        let _ = std::fs::remove_dir_all(b);
    }
    std::process::exit(0)
}

/// The broker's own walls, which its workers keep: none of the session's
/// sockets in sight (see ferro-sandbox), no internet, no programs, none of
/// the kernel's riskier corners. It (and so a worker, until it locks itself
/// into its folder) may still change files: what it's for. Whether all that
/// held: if not, no archive is opened.
fn protect_broker() -> Result<(), String> {
    use ferro_sandbox::linux;
    // SAFETY: getuid has no preconditions.
    let found = linux::escape_sockets(unsafe { libc::getuid() });
    if let Err(e) = linux::hide_sessions_sockets(None, &[]) {
        if !found.is_empty() {
            return Err(format!("the session's sockets can't be hidden ({e})"));
        }
    }
    linux::lock_privileges().map_err(|e| e.to_string())?;
    linux::install_seccomp().map_err(|e| e.to_string())?;
    linux::install_net_gate(ferro_sandbox::NetMode::Deny).map_err(|e| e.to_string())?;
    if let Err(e) = linux::restrict_exec(&[]) {
        // Only a kernel without Landlock; the rest still holds.
        if linux::landlock_abi() >= 1 {
            return Err(e.to_string());
        }
    }
    Ok(())
}

fn worker(job: UnixStream, base: Option<&Path>, protected: Result<(), String>) -> ! {
    let writer = Arc::new(Mutex::new(job.try_clone().unwrap_or_else(|_| std::process::exit(1))));
    let mut reader = job;
    let outcome = match recv(&mut reader) {
        Ok((b'Q', fields)) => match decode(&fields) {
            Some(request) => protected.map_err(|e| Error::Io(format!("FerroOS Archive can't protect itself, so it won't: {e}."))).and_then(|()| {
                let progress = Arc::new(Progress::default());
                run_locked(request, base, &progress, &mut reader, &writer)
            }),
            None => Err(Error::Io("That request made no sense.".into())),
        },
        _ => std::process::exit(1),
    };
    if let Ok(mut w) = writer.lock() {
        let _ = send(&mut *w, b'R', &encode_outcome(&outcome));
    }
    std::process::exit(0)
}

/// The folder a job changes files in, made if it's to be, and the checks.
fn prepare(request: &Request, base: Option<&Path>) -> Result<PathBuf, Error> {
    let io = |e: io::Error| Error::Io(e.to_string());
    let parent = |p: &Path| p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    let folder = match request {
        Request::To { dest, .. } => {
            std::fs::create_dir_all(dest).map_err(|e| Error::Io(format!("Couldn't make the folder: {e}")))?;
            dest.clone()
        }
        Request::Here { archive, .. } | Request::AddTo { archive, .. } => parent(archive),
        Request::Add { dest, .. } => parent(dest),
        Request::Open { n, .. } => {
            let base = base.ok_or(Error::Cancelled)?;
            crate::private_dir(base, *n).map_err(io)?
        }
        Request::Settings { .. } => unreachable!("settings are written before"),
    };
    let folder = folder.canonicalize().map_err(io)?;
    let homes = [ferro_sandbox::linux::home_folder(), std::env::var_os("HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok())];
    // The private folder in memory is ours by construction (and under
    // /run/user, not the home folder).
    if !matches!(request, Request::Open { .. }) {
        for home in &homes {
            if let Some(why) = ferro_sandbox::linux::too_wide(&folder, home.as_deref()) {
                return Err(Error::Io(refusal(request, why)));
            }
        }
    }
    Ok(folder)
}

/// Why a folder was refused, and what to do instead.
fn refusal(request: &Request, why: ferro_sandbox::linux::TooWide) -> String {
    use ferro_sandbox::linux::TooWide;
    let place = match why {
        TooWide::Home => "right in the home folder",
        TooWide::Hidden => "in the home folder's hidden folders (settings, what starts at logon)",
        TooWide::Settings => "with FerroOS's own settings (C:\\ProgramData\\ferro)",
    };
    let instead = match request {
        Request::To { .. } => "Choose a folder inside it (Documents, Downloads...).",
        Request::Here { .. } => "Use Extract and choose a folder (Documents, Downloads...), or move the archive into one.",
        _ => "Move it into a folder (Documents, Downloads...) first.",
    };
    format!("FerroOS Archive doesn't change files {place}: what an archive put there could run later. {instead}")
}

/// The job, locked into its folder.
fn run_locked(
    request: Request,
    base: Option<&Path>,
    progress: &Arc<Progress>,
    reader: &mut UnixStream,
    writer: &Arc<Mutex<UnixStream>>,
) -> Result<Done, Error> {
    if let Request::Settings { format, level } = request {
        crate::save_settings(format, level);
        return Ok(Done::Said(String::new()));
    }
    let folder = prepare(&request, base)?;
    let jail = ferro_sandbox::linux::Jail { tree: vec![folder.to_string_lossy().into_owned()], ..Default::default() };
    if let Err(e) = ferro_sandbox::linux::jail(&jail) {
        if ferro_sandbox::linux::landlock_abi() >= 1 {
            return Err(Error::Io(format!("FerroOS Archive can't protect itself, so it won't: {e}.")));
        }
    }
    // Cancel from the window; progress to it.
    if let Ok(mut r) = reader.try_clone() {
        let p = progress.clone();
        std::thread::spawn(move || {
            while let Ok((tag, _)) = recv(&mut r) {
                if tag == b'C' {
                    p.cancel.store(true, Ordering::Relaxed);
                }
            }
        });
    }
    {
        let (p, w) = (progress.clone(), writer.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(150));
            let current = p.current.lock().map(|c| c.clone()).unwrap_or_default();
            let (done, total) = (p.done.load(Ordering::Relaxed).to_string(), p.total.load(Ordering::Relaxed).to_string());
            let Ok(mut w) = w.lock() else { return };
            if send(&mut *w, b'P', &[done.as_bytes(), total.as_bytes(), current.as_bytes()].map(|b| Zeroizing::new(b.to_vec()))).is_err() {
                return;
            }
        });
    }
    let pw = |p: &Option<Secret>| p.as_ref().map(|s| s.to_string());
    match request {
        Request::To { archive, password, dest, shown, which } => {
            let pw = pw(&password);
            let listing = open(&archive, pw.as_deref())?;
            extract(&archive, &listing, &dest, which.as_deref(), pw.as_deref(), Limits::for_dest(&dest), progress)
                .map(|r| Done::Said(r.summary_into(Some(&shown))))
        }
        Request::Here { archive, password } => extract_here(&archive, pw(&password).as_deref(), progress).map(|(said, _)| Done::Said(said)),
        Request::Open { archive, password, index, .. } => {
            let pw = pw(&password);
            let listing = open(&archive, pw.as_deref())?;
            let entry = listing.entries.get(index).ok_or(Error::Cancelled)?;
            let rel = safe_path(&entry.name).ok_or_else(|| Error::Io("That name isn't safe to open.".into()))?;
            extract(&archive, &listing, &folder, Some(&[index]), pw.as_deref(), Limits::for_dest(&folder), progress)?;
            Ok(Done::Opened(folder.join(rel)))
        }
        Request::AddTo { archive, sources, level, password } => add_to(&archive, &sources, level, pw(&password).as_deref(), progress).map(|m| {
            let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let said = format!("Added {} file(s), {}, to '{name}'.", m.files, human_size(m.bytes));
            Done::Made(if m.skipped > 0 { format!("{said} Left out {} link(s).", m.skipped) } else { said }, archive)
        }),
        Request::Add { sources, dest, format, level, password } => {
            create(&sources, &dest, format, level, pw(&password).as_deref(), progress).map(|m| {
                let made = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let said = format!("Made '{made}': {} file(s), {}.", m.files, human_size(m.bytes));
                Done::Made(if m.skipped > 0 { format!("{said} Left out {} link(s).", m.skipped) } else { said }, dest)
            })
        }
        Request::Settings { .. } => unreachable!("handled above"),
    }
}

// ---- on the wire -------------------------------------------------------------

type Field = Zeroizing<Vec<u8>>;

/// The most a message may hold: what a window could ask for, many times over.
const MOST_FIELDS: usize = 1 << 16;
const MOST_BYTES: usize = 64 << 20;

fn send(w: &mut impl Write, tag: u8, fields: &[Field]) -> io::Result<()> {
    let mut out = Zeroizing::new(Vec::with_capacity(5 + fields.iter().map(|f| f.len() + 4).sum::<usize>()));
    out.push(tag);
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for f in fields {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f);
    }
    w.write_all(&out)
}

fn recv(r: &mut impl Read) -> io::Result<(u8, Vec<Field>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let count = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if count > MOST_FIELDS {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let (mut fields, mut total) = (Vec::with_capacity(count.min(64)), 0usize);
    for _ in 0..count {
        let mut len = [0u8; 4];
        r.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        total += len;
        if total > MOST_BYTES {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let mut f = Zeroizing::new(vec![0u8; len]);
        r.read_exact(&mut f)?;
        fields.push(f);
    }
    Ok((head[0], fields))
}

fn field(b: &[u8]) -> Field {
    Zeroizing::new(b.to_vec())
}

fn path_field(p: &Path) -> Field {
    field(p.as_os_str().as_bytes())
}

fn password_field(p: &Option<Secret>) -> Field {
    match p {
        Some(s) => {
            let mut f = Zeroizing::new(Vec::with_capacity(s.len() + 1));
            f.push(b'+');
            f.extend_from_slice(s.as_bytes());
            f
        }
        None => field(b"-"),
    }
}

fn level_name(l: Level) -> &'static [u8] {
    match l {
        Level::Store => b"store",
        Level::Normal => b"normal",
        Level::Maximum => b"maximum",
    }
}

fn encode(r: &Request) -> Vec<Field> {
    let mut f = Vec::new();
    match r {
        Request::To { archive, password, dest, shown, which } => {
            let which = which.as_ref().map(|w| w.iter().map(usize::to_string).collect::<Vec<_>>().join(",")).unwrap_or_else(|| "all".into());
            f.extend([
                field(b"to"),
                path_field(archive),
                password_field(password),
                path_field(dest),
                field(shown.as_bytes()),
                field(which.as_bytes()),
            ]);
        }
        Request::Here { archive, password } => f.extend([field(b"here"), path_field(archive), password_field(password)]),
        Request::Open { archive, password, index, n } => {
            f.extend([
                field(b"open"),
                path_field(archive),
                password_field(password),
                field(index.to_string().as_bytes()),
                field(n.to_string().as_bytes()),
            ]);
        }
        Request::AddTo { archive, sources, level, password } => {
            f.extend([field(b"addto"), path_field(archive), password_field(password), field(level_name(*level))]);
            f.extend(sources.iter().map(|s| path_field(s)));
        }
        Request::Add { sources, dest, format, level, password } => {
            f.extend([field(b"add"), path_field(dest), password_field(password), field(format.ext().as_bytes()), field(level_name(*level))]);
            f.extend(sources.iter().map(|s| path_field(s)));
        }
        Request::Settings { format, level } => f.extend([field(b"settings"), field(format.ext().as_bytes()), field(level_name(*level))]),
    }
    f
}

fn decode(f: &[Field]) -> Option<Request> {
    let path = |i: usize| f.get(i).filter(|b| !b.is_empty() && !b.contains(&0)).map(|b| PathBuf::from(std::ffi::OsString::from_vec(b.to_vec())));
    let password = |i: usize| -> Option<Option<Secret>> {
        let b = f.get(i)?;
        match b.first()? {
            b'-' => Some(None),
            b'+' => Some(Some(Zeroizing::new(String::from_utf8(b[1..].to_vec()).ok()?))),
            _ => None,
        }
    };
    let text = |i: usize| f.get(i).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_owned);
    let level = |i: usize| match f.get(i)?.as_slice() {
        b"store" => Some(Level::Store),
        b"normal" => Some(Level::Normal),
        b"maximum" => Some(Level::Maximum),
        _ => None,
    };
    let format = |i: usize| match f.get(i)?.as_slice() {
        b"7z" => Some(NewFormat::SevenZ),
        b"zip" => Some(NewFormat::Zip),
        _ => None,
    };
    let sources = |from: usize| (from..f.len()).map(path).collect::<Option<Vec<_>>>().filter(|s| !s.is_empty());
    Some(match f.first()?.as_slice() {
        b"to" => {
            let which = match text(5)?.as_str() {
                "all" => None,
                w => Some(w.split(',').map(|n| n.parse().ok()).collect::<Option<Vec<usize>>>()?),
            };
            Request::To { archive: path(1)?, password: password(2)?, dest: path(3)?, shown: text(4)?, which }
        }
        b"here" => Request::Here { archive: path(1)?, password: password(2)? },
        b"open" => Request::Open { archive: path(1)?, password: password(2)?, index: text(3)?.parse().ok()?, n: text(4)?.parse().ok()? },
        b"addto" => Request::AddTo { archive: path(1)?, password: password(2)?, level: level(3)?, sources: sources(4)? },
        b"add" => Request::Add { dest: path(1)?, password: password(2)?, format: format(3)?, level: level(4)?, sources: sources(5)? },
        b"settings" => Request::Settings { format: format(1)?, level: level(2)? },
        _ => return None,
    })
}

fn encode_outcome(o: &Result<Done, Error>) -> Vec<Field> {
    let (kind, text, path): (&[u8], String, Option<&Path>) = match o {
        Ok(Done::Said(s)) => (b"said", s.clone(), None),
        Ok(Done::Opened(p)) => (b"opened", String::new(), Some(p)),
        Ok(Done::Made(s, p)) => (b"made", s.clone(), Some(p)),
        Err(Error::NeedPassword) => (b"needpassword", String::new(), None),
        Err(Error::WrongPassword) => (b"wrongpassword", String::new(), None),
        Err(Error::Cancelled) => (b"cancelled", String::new(), None),
        Err(Error::TooBig(s)) => (b"toobig", s.clone(), None),
        Err(Error::Unsupported(s)) => (b"unsupported", s.clone(), None),
        Err(Error::Damaged(s)) => (b"damaged", s.clone(), None),
        Err(Error::Io(s)) => (b"io", s.clone(), None),
    };
    vec![field(kind), field(text.as_bytes()), path.map(path_field).unwrap_or_else(|| field(b""))]
}

fn decode_outcome(f: &[Field]) -> Result<Done, Error> {
    let text = f.get(1).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    let path = f.get(2).filter(|b| !b.is_empty()).map(|b| PathBuf::from(std::ffi::OsString::from_vec(b.to_vec())));
    let bad = || Error::Io("The answer made no sense.".into());
    match f.first().map(|k| k.as_slice()) {
        Some(b"said") => Ok(Done::Said(text)),
        Some(b"opened") => Ok(Done::Opened(path.ok_or_else(bad)?)),
        Some(b"made") => Ok(Done::Made(text, path.ok_or_else(bad)?)),
        Some(b"needpassword") => Err(Error::NeedPassword),
        Some(b"wrongpassword") => Err(Error::WrongPassword),
        Some(b"cancelled") => Err(Error::Cancelled),
        Some(b"toobig") => Err(Error::TooBig(text)),
        Some(b"unsupported") => Err(Error::Unsupported(text)),
        Some(b"damaged") => Err(Error::Damaged(text)),
        Some(b"io") => Err(Error::Io(text)),
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(r: &Request) -> Request {
        let mut buf = Vec::new();
        send(&mut buf, b'Q', &encode(r)).unwrap();
        let (tag, fields) = recv(&mut buf.as_slice()).unwrap();
        assert_eq!(tag, b'Q');
        decode(&fields).expect("decodes")
    }

    #[test]
    fn jobs_cross_over_whole() {
        let pw = Some(Zeroizing::new("p\u{e4}ss,word".to_owned()));
        let r = round_trip(&Request::To {
            archive: "/a/b c.zip".into(),
            password: pw.clone(),
            dest: "/d".into(),
            shown: "C:\\d".into(),
            which: Some(vec![3, 1]),
        });
        let Request::To { archive, password, dest, shown, which } = r else { panic!() };
        assert_eq!(
            (archive, password.as_deref().map(String::as_str), dest, shown, which),
            ("/a/b c.zip".into(), Some("p\u{e4}ss,word"), "/d".into(), "C:\\d".to_owned(), Some(vec![3, 1]))
        );
        let r = round_trip(&Request::Add {
            sources: vec!["/x".into(), "/y".into()],
            dest: "/o.7z".into(),
            format: NewFormat::SevenZ,
            level: Level::Maximum,
            password: None,
        });
        let Request::Add { sources, dest, format, level, password } = r else { panic!() };
        assert_eq!((sources.len(), dest, format, level, password.is_none()), (2, "/o.7z".into(), NewFormat::SevenZ, Level::Maximum, true));
        assert!(matches!(
            round_trip(&Request::Settings { format: NewFormat::Zip, level: Level::Store }),
            Request::Settings { format: NewFormat::Zip, level: Level::Store }
        ));
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(decode(&[field(b"rm"), field(b"/")]).is_none());
        assert!(decode(&[field(b"here"), field(b""), field(b"-")]).is_none(), "no path");
        assert!(decode(&[field(b"here"), field(b"/a\0b"), field(b"-")]).is_none());
        assert!(decode(&[field(b"to"), field(b"/a"), field(b"-"), field(b"/d"), field(b"d"), field(b"1,x")]).is_none());
        let mut huge = vec![b'Q'];
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(recv(&mut huge.as_slice()).is_err());
    }

    #[test]
    fn how_it_ended_crosses_over() {
        for o in [Ok(Done::Said("x".into())), Ok(Done::Made("m".into(), "/a.zip".into())), Err(Error::WrongPassword), Err(Error::Io("why".into()))] {
            let back = decode_outcome(&encode_outcome(&o));
            assert_eq!(format!("{back:?}"), format!("{o:?}"));
        }
    }

    #[test]
    fn the_home_folder_and_its_settings_are_refused() {
        use ferro_sandbox::linux::TooWide;
        let r = Request::Here { archive: "/home/a/x.zip".into(), password: None };
        assert!(refusal(&r, TooWide::Home).contains("Use Extract"));
        assert!(refusal(&Request::Settings { format: NewFormat::Zip, level: Level::Store }, TooWide::Hidden).contains("hidden folders"));
    }
}
