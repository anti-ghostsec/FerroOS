//! Linux implementation: Landlock, seccomp-BPF, cgroup v2.

use crate::{NetMode, Policy, CGROUP_ROOT};
use std::ffi::CString;
use std::fs;
use std::io;
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
}

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
        let attr = RulesetAttr { handled_access_fs: handled, handled_access_net: 0 };
        let size = if abi >= 4 { std::mem::size_of::<RulesetAttr>() } else { std::mem::size_of::<u64>() };
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

    #[test]
    fn seccomp_program_shape() {
        let p = seccomp_program();
        assert!(p.len() < 4096, "BPF programs are limited to 4096 instructions");
        assert_eq!(p.last().unwrap().k, RET_ALLOW);
        // Every jump lands inside the program.
        for (i, ins) in p.iter().enumerate() {
            if ins.code & 0x07 == 0x05 {
                assert!(i + 1 + (ins.jt.max(ins.jf) as usize) < p.len());
            }
        }
    }
}
