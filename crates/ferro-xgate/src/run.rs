//! Starting the programs the gate doesn't look at (trust.rs).
//!
//!   ferro-xgate run [--detach] PROGRAM [ARGUMENTS]
//!
//! asks the gate, over the display's own socket, to start PROGRAM; the
//! gate starts it as the person, and from then on knows that process for
//! certain. `run` stays until the program ends and ends as it did (so the
//! session lasts as long as its window manager); with `--detach` it comes
//! back at once. Where there's no gate (or it won't start that program),
//! `run` starts the program itself, as any other.
//!
//! The asker says how the program is to run (its arguments, environment,
//! folder, file mask); the gate takes from that only what can't make the
//! program do the asker's bidding: the arguments must be ones it's listed
//! with, and everything in the environment that loads code or names where
//! to find it is dropped. It gets no files from the asker: what it prints
//! goes nowhere, as with everything the desktop starts.

use std::ffi::CString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// What a request to start a program begins with, where a program's first
/// message to the display would begin with its byte order (`l` or `B`).
pub const MAGIC: &[u8; 12] = b"FERRO-XGATE\0";
/// Set for what the gate started (and what `run` started itself): the
/// program then doesn't ask again.
pub const STARTED: &str = "FERRO_XGATE_RUN";
/// The longest request taken.
pub const MOST: usize = 1 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub detach: bool,
    pub umask: u32,
    pub dir: String,
    /// The program, then its arguments.
    pub args: Vec<String>,
    /// NAME=value, as the asker has them.
    pub env: Vec<String>,
}

impl Request {
    /// As sent: each field ended by a 0 byte.
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = vec![u8::from(self.detach).to_string(), format!("{:o}", self.umask), self.dir.clone(), self.args.len().to_string()];
        fields.extend(self.args.iter().cloned());
        fields.extend(self.env.iter().cloned());
        let mut b = Vec::new();
        for f in fields {
            b.extend_from_slice(f.as_bytes());
            b.push(0);
        }
        b
    }

    pub fn parse(b: &[u8]) -> Option<Request> {
        let mut fields = b.strip_suffix(&[0])?.split(|&c| c == 0).map(|f| String::from_utf8(f.to_vec()).ok());
        let detach = fields.next()?? == "1";
        let umask = u32::from_str_radix(&fields.next()??, 8).ok().filter(|m| *m <= 0o777)?;
        let dir = fields.next()??;
        let count: usize = fields.next()??.parse().ok()?;
        let mut args = Vec::new();
        for _ in 0..count {
            args.push(fields.next()??);
        }
        let env = fields.collect::<Option<Vec<_>>>()?;
        (!args.is_empty()).then_some(Request { detach, umask, dir, args, env })
    }
}

/// What in the environment makes a program load code, or look for its
/// parts, where the asker says: the loader's own (LD_PRELOAD and the
/// rest) and the C library's.
fn loads_code(name: &str) -> bool {
    const NAMES: [&str; 11] = [
        "GCONV_PATH",
        "GETCONF_DIR",
        "GLIBC_TUNABLES",
        "HOSTALIASES",
        "LOCALDOMAIN",
        "LOCPATH",
        "NIS_PATH",
        "NLSPATH",
        "RESOLV_HOST_CONF",
        "RES_OPTIONS",
        "TZDIR",
    ];
    NAMES.contains(&name) || name.starts_with("LD_") || name.starts_with("MALLOC_")
}

/// The same of the graphics libraries, which a compositor draws with:
/// where they find their drivers.
fn loads_drivers(name: &str) -> bool {
    const STARTS: [&str; 13] =
        ["LIBGL_", "MESA_", "GALLIUM_", "GBM_", "EGL_", "__EGL_", "__GLX_", "__GL_", "__NV_", "VK_", "LIBVA_", "VDPAU_", "OCL_"];
    name == "DRIRC_CONFIGDIR" || STARTS.iter().any(|s| name.starts_with(s))
}

/// The asker's environment as the started program gets it. FerroOS's own
/// draw without the graphics libraries, and what the window manager has
/// every program it starts has: theirs keep the graphics settings.
pub fn clean_environment(env: &[String], graphics: bool) -> Vec<(String, String)> {
    let mut clean: Vec<(String, String)> = env
        .iter()
        .filter_map(|e| e.split_once('='))
        .filter(|(n, _)| !n.is_empty() && !loads_code(n) && !(graphics && loads_drivers(n)) && *n != STARTED)
        .map(|(n, v)| (n.into(), v.into()))
        .collect();
    clean.push((STARTED.into(), "1".into()));
    clean
}

/// How the started program differs from the gate (root's): whose it is,
/// and what the asker's own programs have that the gate's don't.
#[derive(Clone, Debug, Default)]
pub struct As {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    /// The asker's place among the system's sessions (its control group):
    /// what it starts belongs to the person's logon as the asker does, or
    /// they'd be asked for a password to use a USB stick.
    pub cgroup: Option<CString>,
    /// Its limits (open files and the rest), by number.
    pub limits: Vec<(u32, libc::rlimit)>,
}

impl As {
    /// What process `pid` (the asker; its user and group as the socket gave them) runs as.
    pub fn of(pid: u32, uid: u32, gid: u32) -> As {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let groups = status
            .lines()
            .find_map(|l| l.strip_prefix("Groups:"))
            .map(|l| l.split_whitespace().filter_map(|g| g.parse().ok()).collect())
            .unwrap_or_default();
        let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("0::").map(str::to_owned)))
            .filter(|p| p.starts_with('/') && !p.contains(".."))
            .and_then(|p| CString::new(format!("/sys/fs/cgroup{p}/cgroup.procs")).ok());
        let mut limits = Vec::new();
        for which in 0..16u32 {
            let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: prlimit only fills `l`.
            if unsafe { libc::prlimit(pid as i32, which as _, std::ptr::null(), &mut l) } == 0 {
                limits.push((which, l));
            }
        }
        As { uid, gid, groups, cgroup, limits }
    }
}

/// Starts `request`'s program as `who`; `graphics`: it draws with the
/// graphics libraries (see `clean_environment`). Its output goes nowhere.
pub fn start(request: &Request, who: As, graphics: bool) -> io::Result<Child> {
    let mut cmd = Command::new(&request.args[0]);
    cmd.args(&request.args[1..]).env_clear().envs(clean_environment(&request.env, graphics));
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let dir = CString::new(request.dir.as_str()).unwrap_or_default();
    let umask = request.umask;
    // SAFETY: between fork and exec only system calls, on what was made ready before.
    unsafe {
        cmd.pre_exec(move || {
            if let Some(procs) = &who.cgroup {
                let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                if fd >= 0 {
                    // "0": this process.
                    libc::write(fd, b"0".as_ptr().cast(), 1);
                    libc::close(fd);
                }
            }
            for (which, l) in &who.limits {
                libc::setrlimit(*which as _, l);
            }
            // Signals as a program finds them when a person starts it: none
            // ignored, none held back. The gate may have been started with
            // some ignored (a child's end, say), and what's ignored stays so
            // in every program started from there: one that then waits for
            // a program of its own is told there's none.
            for signal in 1..libc::SIGRTMAX() {
                if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            // Root's gate becomes the person, for good; a gate that's
            // theirs already (a test) has nothing to give up.
            if libc::geteuid() == 0
                && (libc::setgroups(who.groups.len(), who.groups.as_ptr()) != 0
                    || libc::setresgid(who.gid, who.gid, who.gid) != 0
                    || libc::setresuid(who.uid, who.uid, who.uid) != 0)
            {
                return Err(io::Error::last_os_error());
            }
            libc::umask(umask);
            if libc::chdir(dir.as_ptr()) != 0 {
                libc::chdir(c"/".as_ptr());
            }
            Ok(())
        });
    }
    cmd.spawn()
}

/// The exit status `run` ends with for a program that ended like this.
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// The display's socket, from $DISPLAY (`:N` or `:N.S`; a display on
/// another computer has no gate).
pub fn display_number(display: &str) -> Option<&str> {
    let n = display.strip_prefix(':')?;
    let n = n.split_once('.').map_or(n, |(n, _)| n);
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

fn connect(number: &str) -> io::Result<UnixStream> {
    let path = format!("/tmp/.X11-unix/X{number}");
    SocketAddr::from_abstract_name(path.as_bytes()).and_then(|a| UnixStream::connect_addr(&a)).or_else(|_| UnixStream::connect(&path))
}

/// Asks the gate; `None` if there's none there or it won't.
fn ask(request: &Request) -> Option<i32> {
    let display = std::env::var("DISPLAY").ok()?;
    let mut gate = connect(display_number(&display)?).ok()?;
    let body = request.encode();
    if body.len() > MOST {
        return None;
    }
    gate.write_all(MAGIC).ok()?;
    gate.write_all(&(body.len() as u32).to_le_bytes()).ok()?;
    gate.write_all(&body).ok()?;
    let mut lines = BufReader::new(gate).lines();
    // "ok PID", or why not (a display that isn't a gate just hangs up).
    if !lines.next()?.ok()?.starts_with("ok ") {
        return None;
    }
    if request.detach {
        return Some(0);
    }
    // "exit CODE" when it ends; the gate gone first: so is the display.
    Some(lines.next().and_then(|l| l.ok()).and_then(|l| l.strip_prefix("exit ")?.parse().ok()).unwrap_or(1))
}

/// `ferro-xgate run [--detach] PROGRAM [ARGUMENTS]`.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let detach = args.first().is_some_and(|a| a == "--detach");
    let args = &args[usize::from(detach)..];
    if args.is_empty() {
        return Err("usage: ferro-xgate run [--detach] PROGRAM [ARGUMENTS]".into());
    }
    if std::env::var_os(STARTED).is_none() {
        let request = Request {
            detach,
            // SAFETY: umask only swaps a number; it's put back at once.
            umask: unsafe {
                let m = libc::umask(0o077);
                libc::umask(m);
                m
            },
            dir: std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into()),
            args: args.to_vec(),
            env: std::env::vars_os().filter_map(|(n, v)| Some(format!("{}={}", n.to_str()?, v.to_str()?))).collect(),
        };
        if let Some(code) = ask(&request) {
            std::process::exit(code);
        }
    }
    // No gate: as any program is started.
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]).env(STARTED, "1");
    if detach {
        cmd.stdin(Stdio::null()).spawn()?;
        return Ok(());
    }
    Err(cmd.exec().into())
}

/// Reads a request's body: its length (4 bytes), then that many bytes.
pub fn read_request(from: &mut impl Read) -> Option<Request> {
    let mut len = [0u8; 4];
    from.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MOST {
        return None;
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body).ok()?;
    Request::parse(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_request_there_and_back() {
        let r = Request {
            detach: true,
            umask: 0o22,
            dir: "/home/me".into(),
            args: strings(&["/usr/bin/picom", "--config", "/etc/xdg/picom.conf"]),
            env: strings(&["A=1", "B=two words"]),
        };
        let mut sent = (r.encode().len() as u32).to_le_bytes().to_vec();
        sent.extend(r.encode());
        assert_eq!(read_request(&mut sent.as_slice()), Some(r.clone()));
        assert_eq!(Request::parse(b"0\x0022\0/\x000\0"), None, "no program");
        assert_eq!(Request::parse(b"0\x0022\0/\x003\0a\0"), None, "fewer arguments than it says");
        assert_eq!(Request::parse(b"0\x00999\0/\x001\0a\0"), None, "not a file mask");
        assert_eq!(Request::parse(b""), None);
        let mut long = vec![0xff, 0xff, 0xff, 0x7f];
        long.extend_from_slice(b"x");
        assert_eq!(read_request(&mut long.as_slice()), None, "too long to be one");
    }

    #[test]
    fn nothing_that_loads_code_gets_through() {
        let env = strings(&[
            "HOME=/home/me",
            "LD_PRELOAD=/tmp/x.so",
            "LD_LIBRARY_PATH=/tmp",
            "LD_AUDIT=/tmp/a.so",
            "LIBGL_DRIVERS_PATH=/tmp/dri",
            "GBM_BACKENDS_PATH=/tmp",
            "__EGL_VENDOR_LIBRARY_DIRS=/tmp",
            "VK_ICD_FILENAMES=/tmp/icd.json",
            "GCONV_PATH=/tmp",
            "FERRO_XGATE_RUN=no",
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus",
            "=odd",
            "nothing",
        ]);
        let clean = clean_environment(&env, true);
        let names: Vec<&str> = clean.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["HOME", "DBUS_SESSION_BUS_ADDRESS", STARTED]);
        assert_eq!(clean[2].1, "1");
        // The window manager's goes on to every program: their graphics settings stay.
        let names: Vec<String> = clean_environment(&env, false).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["HOME", "LIBGL_DRIVERS_PATH", "GBM_BACKENDS_PATH", "__EGL_VENDOR_LIBRARY_DIRS", "VK_ICD_FILENAMES", "DBUS_SESSION_BUS_ADDRESS", STARTED]
        );
    }

    #[test]
    fn which_display() {
        assert_eq!(display_number(":0"), Some("0"));
        assert_eq!(display_number(":7.0"), Some("7"));
        assert_eq!(display_number("elsewhere:0"), None);
        assert_eq!(display_number(":"), None);
        assert_eq!(display_number(":../x"), None);
    }

    #[test]
    fn starts_it_and_says_how_it_ended() {
        // SAFETY: getuid and getgid only read this process's own.
        let me = As { uid: unsafe { libc::getuid() }, gid: unsafe { libc::getgid() }, ..Default::default() };
        let request = Request {
            umask: 0o22,
            dir: "/".into(),
            args: strings(&["/bin/sh", "-c", "[ \"$FERRO_XGATE_RUN\" = 1 ] && [ -z \"$LD_PRELOAD\" ] && exit 7"]),
            env: strings(&["LD_PRELOAD=/nonexistent.so"]),
            ..Default::default()
        };
        let mut child = start(&request, me, true).unwrap();
        assert_eq!(exit_code(child.wait().unwrap()), 7);
    }

    #[test]
    fn what_the_gate_ignores_the_program_doesnt() {
        // SAFETY: getuid and getgid only read this process's own; ignoring
        // SIGUSR2 changes nothing for the other tests, which don't send it.
        let me = As { uid: unsafe { libc::getuid() }, gid: unsafe { libc::getgid() }, ..Default::default() };
        unsafe { libc::signal(libc::SIGUSR2, libc::SIG_IGN) };
        // SigIgn in /proc/PID/status: a bit for each ignored signal (SIGUSR2 is 12, bit 11).
        let script = "i=$(sed -n 's/^SigIgn:[[:space:]]*//p' /proc/$$/status); [ $((0x$i & 0x800)) = 0 ] && exit 7";
        let request = Request { umask: 0o22, dir: "/".into(), args: strings(&["/bin/sh", "-c", script]), ..Default::default() };
        let mut child = start(&request, me, true).unwrap();
        assert_eq!(exit_code(child.wait().unwrap()), 7);
    }
}
