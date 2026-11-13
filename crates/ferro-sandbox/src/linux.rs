//! Linux implementation: Landlock, seccomp-BPF, cgroup v2.

use crate::{NetMode, Policy, CGROUP_ROOT};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

// ---- Landlock ----------------------------------------------------------------

const CREATE_RULESET_VERSION: u32 = 1 << 0;
const RULE_PATH_BENEATH: u32 = 1;

const FS_EXECUTE: u64 = 1 << 0;
const FS_WRITE_FILE: u64 = 1 << 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;
const FS_TRUNCATE: u64 = 1 << 14;
const FS_IOCTL_DEV: u64 = 1 << 15;
/// Rights that make sense on a single file (directory rights are EINVAL).
const FS_FILE_RIGHTS: u64 = FS_EXECUTE | FS_WRITE_FILE | FS_READ_FILE | FS_TRUNCATE | FS_IOCTL_DEV;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    /// ABI 6: what the app may not reach outside its own sandbox.
    scoped: u64,
}

/// ABI 6 scopes: abstract Unix sockets and signals stay inside the
/// sandbox, so an app can't talk to the desktop's sockets or kill (or
/// stop) the user's other programs, the lock screen among them.
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const SCOPE_SIGNAL: u64 = 1 << 1;

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Landlock ABI version supported by the running kernel (0 = unavailable).
pub fn landlock_abi() -> i64 {
    // SAFETY: the version query takes no attribute.
    let v = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0usize, CREATE_RULESET_VERSION) };
    v.max(0)
}

/// Every filesystem right this ABI knows, so anything not granted is denied.
fn handled_fs(abi: i64) -> u64 {
    let mut all = (1 << 13) - 1; // v1: execute .. make_sym
    if abi >= 2 {
        all |= 1 << 13; // refer
    }
    if abi >= 3 {
        all |= FS_TRUNCATE;
    }
    if abi >= 5 {
        all |= FS_IOCTL_DEV;
    }
    all
}

/// A Landlock ruleset built from a policy, applied later in the child.
pub struct Ruleset {
    fd: i32,
}

impl Ruleset {
    pub fn build(policy: &Policy) -> io::Result<Self> {
        let abi = landlock_abi();
        if abi < 1 {
            return Err(io::Error::other("Landlock is not enabled in this kernel"));
        }
        let handled = handled_fs(abi);
        // Network is gated by seccomp on socket() instead (see
        // install_net_gate): it covers UDP and ICMP too, and can ask the user.
        let scoped = if abi >= 6 { SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL } else { 0 };
        let attr = RulesetAttr { handled_access_fs: handled, handled_access_net: 0, scoped };
        let size = match abi {
            6.. => std::mem::size_of::<RulesetAttr>(),
            4..=5 => 2 * std::mem::size_of::<u64>(),
            _ => std::mem::size_of::<u64>(),
        };
        // SAFETY: attr outlives the call and `size` covers the fields this ABI reads.
        let fd = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, size, 0u32) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let rs = Self { fd: fd as i32 };
        let read = FS_READ_FILE | FS_READ_DIR;
        for (paths, rights) in [
            (&policy.read_exec, read | FS_EXECUTE),
            (&policy.read, read),
            (&policy.read_write, handled), // everything, within that tree
        ] {
            for p in paths {
                rs.allow(p, rights & handled)?;
            }
        }
        Ok(rs)
    }

    fn allow(&self, path: &str, rights: u64) -> io::Result<()> {
        let Ok(c) = CString::new(path) else { return Ok(()) };
        // SAFETY: valid C string; O_PATH only resolves, it doesn't read.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Ok(()); // a path that doesn't exist grants nothing
        }
        let is_dir = fs::metadata(path).is_ok_and(|m| m.is_dir());
        let attr = PathBeneathAttr { allowed_access: if is_dir { rights } else { rights & FS_FILE_RIGHTS }, parent_fd: fd };
        // SAFETY: attr is the packed struct the kernel expects.
        let r = unsafe { libc::syscall(libc::SYS_landlock_add_rule, self.fd, RULE_PATH_BENEATH, &attr as *const PathBeneathAttr, 0u32) };
        let err = io::Error::last_os_error();
        // SAFETY: closing the fd we opened.
        unsafe { libc::close(fd) };
        if r < 0 {
            return Err(io::Error::new(err.kind(), format!("landlock rule for {path}: {err}")));
        }
        Ok(())
    }

    /// Restricts the calling process (and everything it starts) for good.
    /// Requires `no_new_privs`, set by [`lock_privileges`].
    pub fn restrict_self(&self) -> io::Result<()> {
        // SAFETY: plain syscall on our ruleset fd.
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.fd, 0u32) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Ruleset {
    fn drop(&mut self) {
        // SAFETY: closing our own fd.
        unsafe { libc::close(self.fd) };
    }
}

/// Lets the calling process (and whatever it starts) run only the programs
/// in `allowed` (files, or folders whose contents may run), and nothing
/// else; every other right is left alone. For programs that open files
/// from anywhere, such as the image viewer and the video player: a booby-
/// trapped file that takes one over still can't start a shell or a
/// downloader. The dynamic loader is always allowed, or no allowed program
/// could start (see [`LOADERS`] for what that leaves open). Files and
/// sockets are untouched: [`jail`] confines those too. Needs
/// `no_new_privs` ([`lock_privileges`]) first.
pub fn restrict_exec(allowed: &[&str]) -> io::Result<()> {
    let abi = landlock_abi();
    if abi < 1 {
        return Err(io::Error::other("Landlock is not enabled in this kernel"));
    }
    let attr = RulesetAttr { handled_access_fs: FS_EXECUTE, handled_access_net: 0, scoped: 0 };
    // SAFETY: attr outlives the call; the first field is all ABI 1 reads.
    let fd = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, std::mem::size_of::<u64>(), 0u32) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let rs = Ruleset { fd: fd as i32 };
    for path in allowed.iter().copied().chain(LOADERS) {
        rs.allow(path, FS_EXECUTE)?;
    }
    rs.restrict_self()
}

/// Where the dynamic loader is: the kernel opens it to start any program
/// that isn't statically linked, and Landlock asks for the right to execute
/// it. That right can't be told apart from running the loader by hand on
/// some other file (`ld-linux-x86-64.so.2 /bin/sh`), which only needs to
/// read that file: so the list of programs stops a program that was taken
/// over from simply starting another, not from loading another's code.
/// What holds either way is what the kernel refuses to everything in the
/// sandbox, however it was started: the files, the sockets, the syscalls.
const LOADERS: [&str; 2] = ["/lib64/ld-linux-x86-64.so.2", "/lib/ld-linux-aarch64.so.1"];

const FS_REMOVE_DIR: u64 = 1 << 4;
const FS_REMOVE_FILE: u64 = 1 << 5;
const FS_MAKE_DIR: u64 = 1 << 7;
const FS_MAKE_REG: u64 = 1 << 8;
const FS_MAKE_SYM: u64 = 1 << 12;
const FS_REFER: u64 = 1 << 13;

/// What a program that opens files from anywhere (see [`jail`]) may do
/// besides reading. Paths that don't exist grant nothing.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Jail {
    /// Programs it may start (files, or folders whose contents may run).
    pub exec: Vec<String>,
    /// Files it may change as they are; folders in which it may make,
    /// change and remove plain files (and so everything beneath them).
    pub change: Vec<String>,
    /// Folders it may take plain files out of: delete them, or move them to
    /// a folder that takes them in. It can't change them or make new ones.
    pub take_from: Vec<String>,
    /// Folders plain files may be moved into (and empty ones made in), but
    /// not written: the Recycle Bin, for files of the same drive.
    pub move_into: Vec<String>,
    /// Folders in which it may make, change and remove anything but
    /// devices and pipes: files, folders and links (unpacking an archive).
    pub tree: Vec<String>,
    /// Leaves other programs' abstract Unix sockets reachable: for a program
    /// that draws on a display server that listens on one. libxcb tries
    /// that socket first and gives up when it's refused, instead of going
    /// on to the one in /tmp/.X11-unix.
    pub abstract_sockets: bool,
}

/// The devices a program needs to draw, and the ones every program expects.
/// Not the rest of /dev: keyboards and mice (/dev/input), cameras, disks,
/// other programs' shared memory, and not /dev/pts: through it a program
/// could read what's typed in every terminal of yours (a sudo password)
/// and type into them. Its own terminal it already has open, and /dev/tty
/// is only that one. Sound: see `sound_devices`.
const DEVICES: [&str; 7] = ["/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", "/dev/dri"];

/// What in /dev/snd plays sound: everything but the recording devices
/// (pcmC*D*c), so a jailed player can't listen in.
fn sound_devices() -> Vec<String> {
    let names = fs::read_dir("/dev/snd").into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    names.filter(|n| !is_capture(n)).map(|n| format!("/dev/snd/{n}")).collect()
}

fn is_capture(name: &str) -> bool {
    name.starts_with("pcmC") && name.ends_with('c')
}

/// Confines the calling process, and everything it starts, for good:
///
/// * every file can be read, as before (it's given files from anywhere);
/// * nothing can be written, made, removed, renamed or linked, except as
///   `jail` says, and the devices for the screen and the sound;
/// * only the programs in `jail.exec` can be started (see [`LOADERS`]);
/// * from Landlock ABI 6 (Linux 6.12): no signals to processes outside,
///   and no connecting to other programs' abstract Unix sockets (unless
///   `jail.abstract_sockets`).
///
/// Not covered: connecting to a Unix socket that has a path (the display
/// server's, the session bus's). No Landlock up to ABI 6 can refuse that,
/// whatever rights the path is given, so a jailed program reaches whatever
/// listens there as the same user. Sockets inherited or made with
/// socketpair() work as before too.
///
/// Returns the Landlock ABI in use. Fails on a kernel without Landlock:
/// the caller must not run the program then. Needs `no_new_privs`
/// ([`lock_privileges`]) first.
pub fn jail(jail: &Jail) -> io::Result<i64> {
    let abi = landlock_abi();
    if abi < 1 {
        return Err(io::Error::other("Landlock is not enabled in this kernel"));
    }
    let handled = handled_fs(abi);
    let scoped = match (abi >= 6, jail.abstract_sockets) {
        (false, _) => 0,
        (true, false) => SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL,
        (true, true) => SCOPE_SIGNAL,
    };
    let attr = RulesetAttr { handled_access_fs: handled, handled_access_net: 0, scoped };
    let size = match abi {
        6.. => std::mem::size_of::<RulesetAttr>(),
        4..=5 => 2 * std::mem::size_of::<u64>(),
        _ => std::mem::size_of::<u64>(),
    };
    // SAFETY: attr outlives the call and `size` covers the fields this ABI reads.
    let fd = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, size, 0u32) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let rs = Ruleset { fd: fd as i32 };
    let read = FS_READ_FILE | FS_READ_DIR;
    rs.allow("/", read)?;
    for path in jail.exec.iter().map(String::as_str).chain(LOADERS) {
        rs.allow(path, (read | FS_EXECUTE) & handled)?;
    }
    for path in DEVICES.iter().map(|d| (*d).to_owned()).chain(graphics_driver_devices()).chain(sound_devices()) {
        rs.allow(&path, (read | FS_WRITE_FILE | FS_IOCTL_DEV) & handled)?;
    }
    let take = FS_REMOVE_FILE | FS_REFER;
    for (paths, rights) in [
        (&jail.change, read | take | FS_MAKE_REG | FS_WRITE_FILE | FS_TRUNCATE),
        (&jail.take_from, read | take),
        (&jail.move_into, read | take | FS_MAKE_REG),
        (&jail.tree, read | take | FS_MAKE_REG | FS_MAKE_DIR | FS_MAKE_SYM | FS_REMOVE_DIR | FS_WRITE_FILE | FS_TRUNCATE),
    ] {
        for path in paths {
            rs.allow(path, rights & handled)?;
        }
    }
    rs.restrict_self()?;
    Ok(abi)
}

/// FerroOS's settings shared by every account: which programs may reach
/// the network, the kill switches.
pub const SETTINGS: &str = "/ProgramData/ferro";

/// Why a jailed program mustn't be let change files in a folder (and so in
/// everything beneath it): what it left there would run, or decide, outside.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TooWide {
    /// The home folder, or a folder that holds it: its .bashrc and .profile.
    Home,
    /// A hidden folder in the home folder: settings (which program opens a
    /// file), what starts at logon (.config/autostart), the Start menu's
    /// entries, the shell's and the sound system's files.
    Hidden,
    /// FerroOS's shared settings, or a folder that holds them.
    Settings,
}

/// Whether rights to change files in `folder` reach too far (see
/// [`TooWide`]). `folder` and `home` are as the kernel sees them (links
/// followed: canonical).
pub fn too_wide(folder: &Path, home: Option<&Path>) -> Option<TooWide> {
    let settings = Path::new(SETTINGS);
    if folder.starts_with(settings) || settings.starts_with(folder) {
        return Some(TooWide::Settings);
    }
    let Some(home) = home else { return (folder == Path::new("/")).then_some(TooWide::Home) };
    if home.starts_with(folder) {
        return Some(TooWide::Home);
    }
    let first = folder.strip_prefix(home).ok().and_then(|rest| rest.components().next());
    first.filter(|c| c.as_os_str().as_encoded_bytes().starts_with(b".")).map(|_| TooWide::Hidden)
}

/// The home folder, as the kernel sees it: from the account (not the
/// environment, which whoever starts the program chooses), else $HOME.
pub fn home_folder() -> Option<PathBuf> {
    // SAFETY: getuid has no preconditions; getpwuid's answer is read at once.
    let from_account = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (!pw.is_null() && !(*pw).pw_dir.is_null())
            .then(|| PathBuf::from(std::ffi::OsStr::from_bytes(std::ffi::CStr::from_ptr((*pw).pw_dir).to_bytes())))
    };
    from_account.or_else(|| std::env::var_os("HOME").map(PathBuf::from)).filter(|p| p.is_absolute()).and_then(|p| p.canonicalize().ok())
}

/// The sockets a jailed program could otherwise reach, since Landlock can't
/// refuse connecting to a socket that has a path: through them it would be
/// out of the jail at once. The person's service manager (`systemd --user`)
/// starts any command it's sent; the session bus asks it to as well, and
/// so do the system bus's password-free powers (switching off, mounting,
/// the network) and the agents (gpg, ssh) in the session's folder.
pub fn escape_sockets(uid: u32) -> Vec<PathBuf> {
    let run = PathBuf::from(format!("/run/user/{uid}"));
    [run.join("systemd/private"), run.join("bus"), PathBuf::from("/run/dbus/system_bus_socket")]
        .into_iter()
        .filter(|p| p.symlink_metadata().is_ok())
        .collect()
}

/// Puts the calling process in namespaces of its own (user, mounts,
/// network), where the session's folder in memory (`/run/user/UID`, and
/// `runtime` if it's elsewhere) and the system bus's (`/run/dbus`) are
/// empty: the sockets of [`escape_sockets`] aren't there to connect to.
/// What's in `keep` (inside those folders: a sound server's socket, a
/// scratch folder) is put back as it was. The network namespace has no
/// network and none of the abstract sockets outside; the display's socket
/// in /tmp/.X11-unix is a file, and reachable as before.
///
/// Must be called while the process has one thread, before
/// [`install_seccomp`] (which refuses namespaces and mounts from then on).
/// The process keeps its own user and groups; after `exec` it has no
/// privileges in the namespace either.
pub fn hide_sessions_sockets(runtime: Option<&Path>, keep: &[PathBuf]) -> io::Result<()> {
    // SAFETY: getuid and getgid have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut over: Vec<PathBuf> = vec![PathBuf::from(format!("/run/user/{uid}")), PathBuf::from("/run/dbus")];
    if let Some(r) = runtime.filter(|r| !over.iter().any(|o| o == r)) {
        over.push(r.to_path_buf());
    }
    over.retain(|o| o.symlink_metadata().is_ok_and(|m| m.is_dir()));
    // What's to be put back: files and folders (not links) in those folders.
    let keep: Vec<(&PathBuf, bool)> = keep
        .iter()
        .filter(|k| over.iter().any(|o| k.starts_with(o) && *k != o))
        .filter_map(|k| k.symlink_metadata().ok().filter(|m| !m.file_type().is_symlink()).map(|m| (k, m.is_dir())))
        .collect();
    // SAFETY: unshare with flags only; the caller has one thread.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWNET) } != 0 {
        return Err(step("namespaces", io::Error::last_os_error()));
    }
    fs::write("/proc/self/setgroups", "deny").map_err(|e| step("setgroups", e))?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1")).map_err(|e| step("uid_map", e))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1")).map_err(|e| step("gid_map", e))?;
    // Nothing done here goes back out to the rest of the computer.
    mount(None, Path::new("/"), None, libc::MS_REC | libc::MS_PRIVATE, None).map_err(|e| step("private /", e))?;
    // A copy of each, taken before it's covered (a mount from before the
    // namespaces can't be bound here; one cloned in it can be moved).
    let mut kept = Vec::new();
    for (path, dir) in keep {
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: c is a NUL-terminated path that outlives the call.
        let fd = unsafe {
            libc::syscall(libc::SYS_open_tree, libc::AT_FDCWD, c.as_ptr(), OPEN_TREE_CLONE | libc::O_CLOEXEC as u32 | libc::AT_RECURSIVE as u32)
        };
        if fd < 0 {
            return Err(step(&format!("{} copied", path.display()), io::Error::last_os_error()));
        }
        // SAFETY: open_tree returned a new descriptor that's ours alone.
        kept.push((path, unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) }, dir));
    }
    let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    for o in &over {
        mount(Some("ferro-jail"), o, Some("tmpfs"), flags, Some("mode=0700,size=64k")).map_err(|e| step(&format!("tmpfs on {}", o.display()), e))?;
    }
    for (path, tree, dir) in &kept {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if *dir {
            fs::create_dir(path)?;
        } else {
            fs::File::create(path)?;
        }
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let empty = c"";
        // SAFETY: the descriptor is open and both strings outlive the call.
        let r = unsafe { libc::syscall(libc::SYS_move_mount, tree.as_raw_fd(), empty.as_ptr(), libc::AT_FDCWD, c.as_ptr(), MOVE_MOUNT_F_EMPTY_PATH) };
        if r != 0 {
            return Err(step(&format!("{} put back", path.display()), io::Error::last_os_error()));
        }
    }
    Ok(())
}

const OPEN_TREE_CLONE: u32 = 1;
const MOVE_MOUNT_F_EMPTY_PATH: u32 = 4;

fn step(what: &str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

fn mount(source: Option<&str>, target: &Path, fstype: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> io::Result<()> {
    let c = |s: &str| CString::new(s).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
    let (source, fstype, data) = (source.map(c).transpose()?, fstype.map(c).transpose()?, data.map(c).transpose()?);
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    // SAFETY: every pointer is null or a NUL-terminated string that outlives the call.
    let r = unsafe { libc::mount(ptr(&source), target.as_ptr(), ptr(&fstype), flags, ptr(&data).cast()) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// NVIDIA's own driver talks through device files right in /dev
/// (nvidiactl, nvidia0, nvidia-uvm...), not /dev/dri.
fn graphics_driver_devices() -> Vec<String> {
    let names = fs::read_dir("/dev").into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    names.filter(|n| n.starts_with("nvidia")).map(|n| format!("/dev/{n}")).collect()
}

/// `no_new_privs`: setuid/setcap binaries run without gaining anything.
pub fn lock_privileges() -> io::Result<()> {
    // SAFETY: prctl with integer arguments.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ---- seccomp -------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const JGE: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
const JSET: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
const RET: u16 = 0x06; // BPF_RET | BPF_K
const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const RET_ALLOW: u32 = 0x7FFF_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const X32_BIT: u32 = 0x4000_0000;

/// Syscalls no sandboxed desktop program needs; they return EPERM.
pub const BLOCKED: &[libc::c_long] = &[
    // Inspecting or tampering with other processes
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_pidfd_getfd,
    libc::SYS_process_madvise,
    libc::SYS_kcmp,
    libc::SYS_move_pages,
    // Changing the kernel
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_iopl,
    libc::SYS_ioperm,
    libc::SYS_uselib,
    // Mounts and namespaces (sandbox escapes)
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    // System-wide state
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_adjtimex,
    libc::SYS_clock_adjtime,
    libc::SYS_acct,
    libc::SYS_quotactl,
    libc::SYS_syslog,
    libc::SYS_vhangup,
    libc::SYS_fanotify_init,
    // Large, frequently exploited kernel attack surface
    libc::SYS_userfaultfd,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
];

const CLONE_NS_FLAGS: u32 = (libc::CLONE_NEWNS
    | libc::CLONE_NEWCGROUP
    | libc::CLONE_NEWUTS
    | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUSER
    | libc::CLONE_NEWPID
    | libc::CLONE_NEWNET) as u32;

fn insn(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// The filter program. Kept as data so it can be unit-tested.
fn seccomp_program() -> Vec<SockFilter> {
    let mut p = vec![
        // Only native x86-64 syscalls; anything else (i386, x32) is killed.
        insn(LD_W_ABS, 4, 0, 0),
        insn(JEQ, AUDIT_ARCH_X86_64, 1, 0),
        insn(RET, RET_KILL_PROCESS, 0, 0),
        insn(LD_W_ABS, 0, 0, 0),
        insn(JGE, X32_BIT, 0, 1),
        insn(RET, RET_KILL_PROCESS, 0, 0),
    ];
    for &nr in BLOCKED {
        p.push(insn(JEQ, nr as u32, 0, 1));
        p.push(insn(RET, RET_ERRNO | libc::EPERM as u32, 0, 0));
    }
    // clone3 passes flags in memory BPF can't read: make libc fall back to clone.
    p.push(insn(JEQ, libc::SYS_clone3 as u32, 0, 1));
    p.push(insn(RET, RET_ERRNO | libc::ENOSYS as u32, 0, 0));
    // clone with namespace flags is a sandbox escape attempt.
    p.push(insn(JEQ, libc::SYS_clone as u32, 0, 3));
    p.push(insn(LD_W_ABS, 16, 0, 0)); // args[0], low 32 bits
    p.push(insn(JSET, CLONE_NS_FLAGS, 0, 1));
    p.push(insn(RET, RET_ERRNO | libc::EPERM as u32, 0, 0));
    // Sockets of four kinds only: local (Unix), the internet's (gated
    // further by the network gate) and the kernel's routing answers
    // (netlink, for looking up addresses). Every other kind is refused:
    // ones loaded on demand (SMC and RDS fall back to plain TCP, Bluetooth,
    // VM sockets) would be a way out past the gate, and the kernel's crypto
    // API (AF_ALG) is for LUKS2. Last, since it replaces the syscall number
    // being compared.
    p.push(insn(JEQ, libc::SYS_socket as u32, 0, 7));
    p.push(insn(LD_W_ABS, 16, 0, 0)); // args[0]: domain
    p.push(insn(JEQ, libc::AF_UNIX as u32, 4, 0));
    p.push(insn(JEQ, libc::AF_INET as u32, 3, 0));
    p.push(insn(JEQ, libc::AF_INET6 as u32, 2, 0));
    p.push(insn(JEQ, libc::AF_NETLINK as u32, 1, 0));
    p.push(insn(RET, RET_ERRNO | libc::EAFNOSUPPORT as u32, 0, 0));
    p.push(insn(RET, RET_ALLOW, 0, 0));
    p.push(insn(RET, RET_ALLOW, 0, 0));
    p
}

/// Installs the syscall filter on the calling thread (inherited on exec).
pub fn install_seccomp() -> io::Result<()> {
    let prog = seccomp_program();
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    // SAFETY: fprog points at a live program for the duration of the call.
    if unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &fprog as *const SockFprog, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ---- network gate --------------------------------------------------------------------

const RET_USER_NOTIF: u32 = 0x7FC0_0000;
const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;
const SECCOMP_FILTER_FLAG_NEW_LISTENER: libc::c_ulong = 1 << 3;

/// Gates `socket(AF_INET|AF_INET6, ...)`, which covers TCP, UDP and ICMP.
/// Raw packet sockets are always refused. Unix sockets are untouched.
fn net_program(action: u32) -> Vec<SockFilter> {
    vec![
        insn(LD_W_ABS, 0, 0, 0),
        insn(JEQ, libc::SYS_socket as u32, 0, 7),
        insn(LD_W_ABS, 16, 0, 0), // args[0]: domain
        insn(JEQ, libc::AF_INET as u32, 3, 0),
        insn(JEQ, libc::AF_INET6 as u32, 2, 0),
        insn(JEQ, libc::AF_PACKET as u32, 0, 3),
        insn(RET, RET_ERRNO | libc::EPERM as u32, 0, 0),
        insn(RET, action, 0, 0),
        insn(RET, RET_ALLOW, 0, 0),
        insn(RET, RET_ALLOW, 0, 0),
    ]
}

/// Installs the network gate. In [`NetMode::Ask`] mode returns the
/// listener fd on which the supervisor receives the questions.
pub fn install_net_gate(mode: NetMode) -> io::Result<Option<i32>> {
    let action = match mode {
        NetMode::Allow => return Ok(None),
        NetMode::Deny => RET_ERRNO | libc::EACCES as u32,
        NetMode::Ask => RET_USER_NOTIF,
    };
    let prog = net_program(action);
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    let flags = if mode == NetMode::Ask { SECCOMP_FILTER_FLAG_NEW_LISTENER } else { 0 };
    // SAFETY: fprog points at a live program for the duration of the call.
    let r = unsafe { libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, flags, &fprog as *const SockFprog) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((mode == NetMode::Ask).then_some(r as i32))
}

#[repr(C)]
#[derive(Default)]
struct SeccompData {
    nr: i32,
    arch: u32,
    ip: u64,
    args: [u64; 6],
}

#[repr(C)]
#[derive(Default)]
struct SeccompNotif {
    id: u64,
    pid: u32,
    flags: u32,
    data: SeccompData,
}

#[repr(C)]
#[derive(Default)]
struct SeccompNotifResp {
    id: u64,
    val: i64,
    error: i32,
    flags: u32,
}

const fn seccomp_iowr(nr: u32, size: usize) -> libc::c_ulong {
    ((3 << 30) | ((size as u32) << 16) | (0x21 << 8) | nr) as libc::c_ulong
}
const NOTIF_RECV: libc::c_ulong = seccomp_iowr(0, std::mem::size_of::<SeccompNotif>());
const NOTIF_SEND: libc::c_ulong = seccomp_iowr(1, std::mem::size_of::<SeccompNotifResp>());
const USER_NOTIF_FLAG_CONTINUE: u32 = 1;

/// One pending "may this app use the network?" question.
pub struct NetRequest {
    id: u64,
}

/// Receives the next question, or `None` if the app went away.
pub fn recv_net_request(listener: i32) -> Option<NetRequest> {
    let mut n = SeccompNotif::default();
    // SAFETY: NOTIF_RECV fills a seccomp_notif.
    (unsafe { libc::ioctl(listener, NOTIF_RECV as _, &mut n) } == 0).then_some(NetRequest { id: n.id })
}

/// Lets the blocked `socket()` call proceed, or fails it with EACCES.
pub fn answer_net_request(listener: i32, req: NetRequest, allow: bool) {
    let mut resp = SeccompNotifResp {
        id: req.id,
        val: 0,
        error: if allow { 0 } else { -libc::EACCES },
        flags: if allow { USER_NOTIF_FLAG_CONTINUE } else { 0 },
    };
    // SAFETY: NOTIF_SEND reads a seccomp_notif_resp.
    unsafe { libc::ioctl(listener, NOTIF_SEND as _, &mut resp) };
}

// ---- capabilities ------------------------------------------------------------------

/// Drops every capability for good: even a root-owned app can't change the
/// network, load drivers, read other users' files or regain power on exec.
pub fn drop_capabilities() -> io::Result<()> {
    const SECBIT_NOROOT: libc::c_ulong = 1 << 0;
    const SECBIT_NOROOT_LOCKED: libc::c_ulong = 1 << 1;
    const SECBIT_NO_SETUID_FIXUP: libc::c_ulong = 1 << 2;
    const SECBIT_NO_SETUID_FIXUP_LOCKED: libc::c_ulong = 1 << 3;
    // SAFETY: plain prctl/capset calls on the current process.
    unsafe {
        // Must happen while we still hold CAP_SETPCAP; harmless if not root.
        libc::prctl(libc::PR_SET_SECUREBITS, SECBIT_NOROOT | SECBIT_NOROOT_LOCKED | SECBIT_NO_SETUID_FIXUP | SECBIT_NO_SETUID_FIXUP_LOCKED);
        for cap in 0..64 {
            libc::prctl(libc::PR_CAPBSET_DROP, cap);
        }
        libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0);
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        let mut hdr = Header { version: 0x2008_0522, pid: 0 };
        let data = [Data { effective: 0, permitted: 0, inheritable: 0 }; 2];
        if libc::syscall(libc::SYS_capset, &mut hdr as *mut Header, data.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

// ---- passing the listener fd from child to supervisor --------------------------------

/// Sends an fd over a Unix socket (SCM_RIGHTS).
pub fn send_fd(sock: i32, fd: i32) -> io::Result<()> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let mut cbuf = [0u8; 64];
    // SAFETY: standard SCM_RIGHTS construction into a correctly sized buffer.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>(), fd);
        if libc::sendmsg(sock, &msg, 0) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn recv_fd(sock: i32) -> io::Result<i32> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let mut cbuf = [0u8; 64];
    // SAFETY: mirrors send_fd; the kernel fills the control buffer.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = cbuf.len() as _;
        if libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) <= 0 {
            return Err(io::Error::last_os_error());
        }
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() || (*cmsg).cmsg_type != libc::SCM_RIGHTS {
            return Err(io::Error::other("no fd received"));
        }
        Ok(std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>()))
    }
}

// ---- cgroup v2 RAM budget ----------------------------------------------------------

/// A per-app cgroup under [`CGROUP_ROOT`]; removed when dropped.
pub struct AppCgroup {
    pub dir: PathBuf,
}

impl AppCgroup {
    pub fn create(name: &str, policy: &Policy) -> io::Result<Self> {
        let dir = Path::new(CGROUP_ROOT).join(format!("{name}.{}", std::process::id()));
        fs::create_dir(&dir)?;
        let cg = Self { dir };
        cg.set_memory_max(policy.memory_max)?;
        let _ = fs::write(cg.dir.join("memory.swap.max"), "0");
        // On OOM, stop the whole app rather than one random thread of it.
        let _ = fs::write(cg.dir.join("memory.oom.group"), "1");
        if let Some(n) = policy.pids_max {
            let _ = fs::write(cg.dir.join("pids.max"), n.to_string());
        }
        Ok(cg)
    }

    pub fn set_memory_max(&self, bytes: Option<u64>) -> io::Result<()> {
        set_memory_max(&self.dir, bytes)
    }

    /// Moves the calling process into this cgroup ("0" means "me").
    pub fn enter(&self) -> io::Result<()> {
        fs::write(self.dir.join("cgroup.procs"), "0")
    }

    /// How many times the kernel had to kill the app for exceeding its budget.
    pub fn oom_kills(&self) -> u64 {
        fs::read_to_string(self.dir.join("memory.events"))
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("oom_kill ")?.trim().parse().ok()))
            .unwrap_or(0)
    }
}

impl Drop for AppCgroup {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir); // only succeeds once empty
    }
}

pub fn set_memory_max(dir: &Path, bytes: Option<u64>) -> io::Result<()> {
    let value = bytes.map_or("max".to_owned(), |b| b.to_string());
    fs::write(dir.join("memory.max"), value)
}

/// The cgroup v2 path of a process, e.g. `/ferro/ferro-cmd.42`.
pub fn cgroup_of(pid: u32) -> Option<String> {
    let text = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("0::")).map(str::to_owned)
}

/// Gives any process a RAM budget: reuses its app cgroup if it has one,
/// otherwise moves it into a new `manual.<pid>` cgroup.
pub fn limit_process(pid: u32, bytes: Option<u64>) -> io::Result<()> {
    if let Some(cg) = cgroup_of(pid).filter(|c| c.starts_with("/ferro/")) {
        return set_memory_max(&Path::new("/sys/fs/cgroup").join(cg.trim_start_matches('/')), bytes);
    }
    let dir = Path::new(CGROUP_ROOT).join(format!("manual.{pid}"));
    fs::create_dir_all(&dir)?;
    set_memory_max(&dir, bytes)?;
    let _ = fs::write(dir.join("memory.oom.group"), "1");
    fs::write(dir.join("cgroup.procs"), pid.to_string())
}

/// Removes `manual.*` cgroups whose process has exited.
pub fn reap_manual_cgroups() {
    for entry in fs::read_dir(CGROUP_ROOT).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("manual.") {
            let _ = fs::remove_dir(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a filter on a syscall from this machine's architecture, as the
    /// kernel would: what it answers.
    fn run(p: &[SockFilter], nr: u32, arg0: u32) -> u32 {
        let data = |off: u32| match off {
            0 => nr,
            4 => AUDIT_ARCH_X86_64,
            16 => arg0,
            _ => 0,
        };
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let i = &p[pc];
            match i.code & 0x07 {
                0x00 => acc = data(i.k),
                0x05 => {
                    let taken = match i.code & 0xf0 {
                        0x10 => acc == i.k,
                        0x20 => acc > i.k,
                        0x30 => acc >= i.k,
                        0x40 => acc & i.k != 0,
                        0x00 => {
                            pc += 1 + i.k as usize;
                            continue;
                        }
                        op => panic!("jump {op:#x}"),
                    };
                    pc += 1 + if taken { i.jt } else { i.jf } as usize;
                    continue;
                }
                0x06 => return i.k,
                c => panic!("class {c:#x}"),
            }
            pc += 1;
        }
    }

    #[test]
    fn seccomp_program_shape() {
        let p = seccomp_program();
        assert!(p.len() < 4096, "BPF programs are limited to 4096 instructions");
        assert_eq!(p.last().unwrap().k, RET_ALLOW);
        // Sockets: the four kinds allowed, every other refused.
        let socket = |domain: i32| run(&p, libc::SYS_socket as u32, domain as u32);
        for ok in [libc::AF_UNIX, libc::AF_INET, libc::AF_INET6, libc::AF_NETLINK] {
            assert_eq!(socket(ok), RET_ALLOW, "domain {ok}");
        }
        for no in [libc::AF_ALG, 43 /* AF_SMC */, libc::AF_RDS, libc::AF_BLUETOOTH, libc::AF_VSOCK, libc::AF_PACKET, libc::AF_TIPC] {
            assert_eq!(socket(no), RET_ERRNO | libc::EAFNOSUPPORT as u32, "domain {no}");
        }
        assert_eq!(run(&p, libc::SYS_read as u32, 0), RET_ALLOW);
        // Every jump lands inside the program.
        for (i, ins) in p.iter().enumerate() {
            if ins.code & 0x07 == 0x05 {
                assert!(i + 1 + (ins.jt.max(ins.jf) as usize) < p.len());
            }
        }
    }

    #[test]
    fn rights_never_reach_the_home_folder_its_settings_or_ferroos() {
        let home = Some(Path::new("/home/you"));
        let w = |f: &str| too_wide(Path::new(f), home);
        assert_eq!(w("/home/you/Pictures"), None);
        assert_eq!(w("/home/you/Pictures/.thumbs"), None, "hidden, but not settings of the home folder's");
        assert_eq!(w("/media/stick"), None);
        assert_eq!(w("/home/you"), Some(TooWide::Home));
        assert_eq!(w("/home"), Some(TooWide::Home));
        assert_eq!(w("/"), Some(TooWide::Settings), "holds FerroOS's settings too");
        assert_eq!(w("/home/you/.config/autostart"), Some(TooWide::Hidden));
        assert_eq!(w("/home/you/.local/share/Trash"), Some(TooWide::Hidden));
        assert_eq!(w("/ProgramData/ferro"), Some(TooWide::Settings));
        assert_eq!(w("/ProgramData"), Some(TooWide::Settings));
        assert_eq!(w("/ProgramData/app"), None);
        assert_eq!(too_wide(Path::new("/srv"), None), None);
        assert_eq!(too_wide(Path::new("/"), None), Some(TooWide::Settings));
    }
}
