//! Raw system setup and teardown.

use std::ffi::CString;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

fn mount(source: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
    let _ = std::fs::create_dir_all(target);
    let (s, t, f, d) = (CString::new(source)?, CString::new(target)?, CString::new(fstype)?, CString::new(data)?);
    // SAFETY: all pointers are valid NUL-terminated strings for the call.
    let r = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), flags, d.as_ptr().cast()) };
    match r {
        0 => Ok(()),
        _ => match io::Error::last_os_error() {
            e if e.raw_os_error() == Some(libc::EBUSY) => Ok(()), // already mounted
            e => Err(e),
        },
    }
}

pub fn mount_early() {
    use libc::{MS_NODEV, MS_NOEXEC, MS_NOSUID};
    let mounts: [(&str, &str, &str, libc::c_ulong, &str); 9] = [
        ("proc", "/proc", "proc", MS_NOSUID | MS_NODEV | MS_NOEXEC, ""),
        ("sysfs", "/sys", "sysfs", MS_NOSUID | MS_NODEV | MS_NOEXEC, ""),
        ("devtmpfs", "/dev", "devtmpfs", MS_NOSUID, "mode=0755"),
        ("devpts", "/dev/pts", "devpts", MS_NOSUID | MS_NOEXEC, "mode=0620,ptmxmode=0666"),
        ("tmpfs", "/run", "tmpfs", MS_NOSUID | MS_NODEV, "mode=0755,size=4m"),
        ("tmpfs", "/tmp", "tmpfs", MS_NOSUID | MS_NODEV, "mode=1777,size=16m"),
        // Logs only ever live in RAM and vanish at power-off.
        ("tmpfs", "/var/log", "tmpfs", MS_NOSUID | MS_NODEV | MS_NOEXEC, "mode=0755,size=2m"),
        // Settings and documents: RAM while running, the encrypted vault on disk.
        ("tmpfs", "/ProgramData", "tmpfs", MS_NOSUID | MS_NODEV, "mode=0755,size=32m,uid=1000,gid=1000"),
        ("tmpfs", "/home", "tmpfs", MS_NOSUID | MS_NODEV, "mode=0755,size=32m,uid=1000,gid=1000"),
    ];
    for (source, target, fstype, flags, data) in mounts {
        if let Err(e) = mount(source, target, fstype, flags, data) {
            eprintln!("init: mount {target}: {e}");
        }
    }
    setup_cgroups();
}

/// cgroup v2 with memory and pids controllers, plus the `ferro` subtree
/// where ferro-run puts each sandboxed app (its RAM budget lives there).
fn setup_cgroups() {
    let root = "/sys/fs/cgroup";
    if let Err(e) = mount("cgroup2", root, "cgroup2", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "nsdelegate") {
        return eprintln!("init: cgroup2: {e} (no per-app RAM budgets)");
    }
    let enable = |dir: &str| std::fs::write(format!("{dir}/cgroup.subtree_control"), "+memory +pids");
    let apps = format!("{root}/ferro");
    let result = enable(root).and_then(|()| std::fs::create_dir_all(&apps)).and_then(|()| enable(&apps));
    if let Err(e) = result {
        eprintln!("init: cgroup setup: {e}");
    }
    // Delegate the `ferro` subtree to the desktop user: it holds the desktop
    // session and every sandboxed app, so the user's own tools can give apps
    // RAM budgets without root. (cgroup v2 delegation: own the directory and
    // its procs/control files.)
    let _ = std::fs::create_dir_all(format!("{apps}/desktop"));
    for p in ["", "/cgroup.procs", "/cgroup.subtree_control", "/cgroup.threads", "/desktop", "/desktop/cgroup.procs", "/desktop/cgroup.threads"] {
        give_to_user(&format!("{apps}{p}"));
    }
}

pub const USER_UID: u32 = 1000;
pub const USER_GID: u32 = 1000;

pub fn give_to_user(path: &str) {
    if let Ok(c) = CString::new(path) {
        // SAFETY: chown with a valid C path.
        unsafe { libc::chown(c.as_ptr(), USER_UID, USER_GID) };
    }
}

/// After drivers load: the desktop user gets the display and input devices
/// (and nothing else), plus a home folder. Physical RAM is recorded for the
/// memory meters, since only root can read it from /proc/iomem.
pub fn prepare_user_session() {
    for dir in ["/dev/dri", "/dev/input", "/dev/snd"] {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            give_to_user(&e.path().to_string_lossy());
        }
    }
    give_to_user("/dev/fb0");
    let _ = std::fs::create_dir_all("/home/user");
    give_to_user("/home/user");
    let _ = std::fs::create_dir_all("/ProgramData/ferro");
    give_to_user("/ProgramData/ferro");
    if let Some(kb) = ferro_sys::MemInfo::read().and_then(|m| m.physical_kb) {
        let _ = std::fs::write("/run/ferro/physical-kb", kb.to_string());
    }
}

/// Kernel knobs that cost nothing and close common leaks and attack paths.
const SYSCTLS: &[(&str, &str)] = &[
    ("kernel/kptr_restrict", "2"),     // hide kernel addresses
    ("kernel/dmesg_restrict", "1"),    // kernel log is admin-only
    ("kernel/yama/ptrace_scope", "2"), // only admins may attach debuggers
    ("kernel/unprivileged_bpf_disabled", "1"),
    ("kernel/perf_event_paranoid", "3"),
    ("kernel/kexec_load_disabled", "1"),
    ("kernel/sysrq", "0"),
    ("fs/protected_symlinks", "1"),
    ("fs/protected_hardlinks", "1"),
    ("fs/protected_fifos", "2"),
    ("fs/protected_regular", "2"),
    ("fs/suid_dumpable", "0"), // no memory dumps of privileged programs
    ("net/core/bpf_jit_harden", "2"),
    ("net/ipv4/tcp_timestamps", "0"), // timestamps reveal uptime
    ("net/ipv4/conf/all/accept_redirects", "0"),
    ("net/ipv4/conf/all/send_redirects", "0"),
    ("net/ipv4/conf/all/accept_source_route", "0"),
    ("net/ipv4/icmp_echo_ignore_broadcasts", "1"),
    ("vm/swappiness", "0"),
];

/// Applies [`SYSCTLS`] and disables core dumps for everything init starts:
/// a crashing program's memory is never written to disk.
pub fn harden() {
    for (key, value) in SYSCTLS {
        // Some knobs don't exist in every kernel build; that's fine.
        let _ = std::fs::write(format!("/proc/sys/{key}"), value);
    }
    let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: setrlimit with a valid struct; inherited by every child.
    unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    let _ = std::fs::create_dir_all("/run/ferro");
}

/// Loads one module file (compressed ones are decompressed by the kernel),
/// passing `module.param=` options from the kernel command line. Already
/// loaded counts as success.
pub fn finit(path: &std::path::Path, cmdline: &str) -> io::Result<()> {
    const MODULE_INIT_COMPRESSED_FILE: libc::c_int = 4;
    let file = std::fs::File::open(path)?;
    let fname = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = fname.split(".ko").next().unwrap_or(&fname);
    let params = CString::new(module_params(cmdline, name)).unwrap_or_default();
    let flags = if fname.ends_with(".ko") { 0 } else { MODULE_INIT_COMPRESSED_FILE };
    use std::os::unix::io::AsRawFd;
    // SAFETY: finit_module(fd, params, flags) with a valid fd and C string.
    let r = unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), params.as_ptr(), flags) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EEXIST) {
        Ok(())
    } else {
        Err(e)
    }
}

/// Loads drivers. With a module tree (our kernel), drivers are matched to
/// the hardware actually present and unneeded files are freed; with a
/// fixed /etc/modules list (the prebuilt quick-start kernel), that list.
pub fn load_modules() {
    if let Some(mut m) = crate::modules::Modules::open() {
        let events = crate::modules::uevent_socket();
        m.load_for_present_hardware();
        m.prune();
        if let Err(e) = events.and_then(|fd| crate::modules::hotplug_listener(fd, m)) {
            eprintln!("init: hotplug: {e}");
        }
        return;
    }
    let Ok(list) = std::fs::read_to_string("/etc/modules") else { return };
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    for path in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        match finit(std::path::Path::new(path), &cmdline) {
            // The kernel has its own copy now; the file in the RAM-backed
            // root filesystem would only waste memory.
            Ok(()) => {
                let _ = std::fs::remove_file(path);
            }
            Err(e) => eprintln!("init: module {path}: {e}"),
        }
    }
}

/// `module.param=value` words from the kernel command line, as modprobe
/// would pass them; e.g. `drm_kms_helper.fbdev_emulation=0`. Module names
/// treat `-` and `_` alike.
fn module_params(cmdline: &str, module: &str) -> String {
    let module = module.replace('-', "_");
    cmdline
        .split_whitespace()
        .filter_map(|w| {
            let (m, p) = w.split_once('.')?;
            (m.replace('-', "_") == module && p.contains('=')).then_some(p)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn set_hostname(name: &str) {
    // SAFETY: pointer/length describe `name`.
    if unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) } != 0 {
        eprintln!("init: sethostname: {}", io::Error::last_os_error());
    }
}

/// Stops every process, syncs disks and powers off or reboots. As a non-PID-1
/// test run it just exits.
pub fn shutdown(reboot: bool) -> ! {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    if std::process::id() != 1 {
        std::process::exit(0);
    }
    println!("init: {}...", if reboot { "rebooting" } else { "powering off" });
    // SAFETY: plain syscalls; kill(-1) from PID 1 spares PID 1 itself.
    unsafe {
        libc::kill(-1, libc::SIGTERM);
        // Up to 3 s for a clean exit (the vault saves on SIGTERM); kill(-1, 0)
        // fails with ESRCH once nothing but init is left.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            if libc::kill(-1, 0) != 0 {
                break;
            }
        }
        libc::kill(-1, libc::SIGKILL);
        libc::sync();
    }
    // Free the page cache and slab caches. With init_on_free, every freed
    // page is zeroed, so file contents and app data don't linger in RAM.
    let _ = std::fs::write("/proc/sys/vm/drop_caches", "3");
    let _ = std::fs::write("/proc/sys/vm/compact_memory", "1");
    // SAFETY: plain syscalls.
    unsafe {
        libc::reboot(if reboot { libc::RB_AUTOBOOT } else { libc::RB_POWER_OFF });
    }
    eprintln!("init: reboot(2) failed: {}", io::Error::last_os_error());
    loop {
        std::thread::park();
    }
}
