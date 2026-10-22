//! The FerroOS app sandbox.
//!
//! Every program the user starts goes through `ferro-run`, which applies
//! four layers before `exec`, then waits so it can report and clean up:
//!
//! 1. **RAM budget** (cgroup v2): `memory.max`, no swap, and `pids.max` so a
//!    fork bomb can't take the machine down.
//! 2. **Filesystem** (Landlock): the program sees only what its [`Policy`]
//!    grants.
//! 3. **Syscalls** (seccomp): the kernel attack surface that no desktop
//!    program needs (ptrace, module loading, mount, bpf, io_uring, new
//!    namespaces, ...) returns EPERM. A second filter gates internet sockets
//!    (TCP, UDP, ICMP): allowed, denied, or asked at first use.
//! 4. **No privileges**: every capability dropped, plus `no_new_privs`.
//!
//! Everything here is unprivileged except creating the cgroup.

/// Where per-app cgroups live (created by ferro-init).
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup/ferro";

/// How the sandbox treats an app's network access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetMode {
    Allow,
    Deny,
    /// Ask the user the first time the app opens an internet socket.
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Read + execute (programs, libraries).
    pub read_exec: Vec<String>,
    /// Read only.
    pub read: Vec<String>,
    /// Read, write, create and delete.
    pub read_write: Vec<String>,
    /// Internet access (TCP, UDP, ICMP over IPv4/IPv6).
    pub network: NetMode,
    /// RAM limit in bytes, `None` for unlimited.
    pub memory_max: Option<u64>,
    /// Process/thread limit.
    pub pids_max: Option<u32>,
}

/// Default RAM budget for a sandboxed program.
pub const DEFAULT_MEMORY_MB: u64 = 256;

impl Policy {
    /// The default profile: system files readable, the program's own folder
    /// executable, a private data folder writable, the terminal it was
    /// started from usable, no network. The working directory is writable
    /// too, unless that would hand over something that controls what runs
    /// later (see [`cwd_writable`]); then it's only readable.
    pub fn standard(program: &str, cwd: &str, data_dir: &str, home: &str, ttys: &[String]) -> Self {
        // Granting "/" (C:\) would grant everything, so the root is never a
        // grant target: a program started from C:\ gets no working-dir access.
        let not_root = |p: &str| (!p.trim_end_matches('/').is_empty()).then(|| p.to_owned());
        let program_dir = std::path::Path::new(program).parent().and_then(|p| not_root(&p.to_string_lossy()));

        let mut read_exec: Vec<String> = vec!["/bin".into(), "/lib".into(), "/usr".into()];
        read_exec.extend(program_dir);
        let mut read: Vec<String> = ["/etc", "/proc", "/sys", "/dev/urandom"].map(String::from).to_vec();
        let mut read_write: Vec<String> = Vec::new();
        match not_root(cwd) {
            Some(dir) if cwd_writable(&dir, home) => read_write.push(dir),
            Some(dir) => read.push(dir),
            None => {}
        }
        read_write.extend([data_dir, "/dev/null", "/dev/zero", "/dev/tty"].map(String::from));
        // Only the terminal(s) it was started on: with all of /dev/pts it
        // could read the keystrokes typed into the user's other terminals.
        read_write.extend(ttys.iter().filter(|t| t.starts_with("/dev/pts/") || t.starts_with("/dev/tty")).cloned());
        Self { read_exec, read, read_write, network: NetMode::Ask, memory_max: Some(DEFAULT_MEMORY_MB << 20), pids_max: Some(256) }
    }
}

/// Whether an app may write in `dir`, its working directory. Not when
/// that would reach the home folder itself (`.profile`, `.bashrc` and
/// `.config` run things at the next logon, outside any sandbox), a hidden
/// folder, FerroOS's own settings, or the system.
pub fn cwd_writable(dir: &str, home: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    let within = |base: &str| {
        let base = base.trim_end_matches('/');
        !base.is_empty() && (dir == base || dir.starts_with(&format!("{base}/")))
    };
    // An ancestor of something protected contains it.
    let contains = |protected: &str| protected.trim_end_matches('/') == dir || protected.starts_with(&format!("{dir}/"));
    let system = ["/bin", "/sbin", "/lib", "/lib64", "/usr", "/etc", "/boot", "/dev", "/proc", "/sys", "/run", "/var", "/ProgramData/ferro"];
    !dir.is_empty()
        && !dir.split('/').any(|c| c.starts_with('.'))
        && !contains(home)
        && !contains("/ProgramData/ferro")
        && !system.iter().any(|s| within(s))
}

/// Parses sizes like `64M`, `1G`, `512K`, `100000` (bytes), or `none`.
pub fn parse_size(s: &str) -> Option<Option<u64>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("max") {
        return Some(None);
    }
    let (num, mult) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    Some(Some(num.trim().parse::<u64>().ok()?.checked_mul(mult)?))
}

/// A cgroup-safe name for a program: `ferro-cmd` -> `ferro-cmd`, spaces and
/// slashes become `_`.
pub fn app_name(program: &str) -> String {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let clean: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    // Each app's data folder is /ProgramData/<name>: "ferro" would be
    // FerroOS's own settings (VPN profile, remembered choices), and "." or
    // ".." would be /ProgramData or / itself.
    if clean.is_empty() {
        "app".into()
    } else if clean.starts_with('.') || clean.eq_ignore_ascii_case("ferro") {
        format!("app-{clean}")
    } else {
        clean
    }
}

pub mod store;

#[cfg(target_os = "linux")]
pub mod linux;

/// Parses a RAM budget as people type it: `5G`, `5 GB`, `50M`, `50mb`,
/// `512` (bare numbers are MB), or `none`/`no limit`. Returns `Some(None)`
/// for "no budget".
pub fn parse_budget(s: &str) -> Option<Option<u64>> {
    let t: String = s.trim().to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if matches!(t.as_str(), "none" | "nolimit" | "max" | "unlimited" | "0") {
        return Some(None);
    }
    let t = t.strip_suffix('b').unwrap_or(&t);
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return t.parse::<u64>().ok()?.checked_mul(1 << 20).map(Some);
    }
    // Decimal amounts like 1.5G.
    let (num, unit) = t.split_at(t.find(|c: char| c.is_ascii_alphabetic())?);
    let mult = match unit {
        "k" => 1u64 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return None,
    };
    let value: f64 = num.parse().ok().filter(|v: &f64| *v > 0.0 && v.is_finite())?;
    Some(Some((value * mult as f64) as u64))
}

/// The command-line entry point.
pub mod cli;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("64M"), Some(Some(64 << 20)));
        assert_eq!(parse_size("1g"), Some(Some(1 << 30)));
        assert_eq!(parse_size("4096"), Some(Some(4096)));
        assert_eq!(parse_size("none"), Some(None));
        assert_eq!(parse_size("lots"), None);
    }

    #[test]
    fn names_and_policy() {
        assert_eq!(app_name("/bin/ferro-cmd"), "ferro-cmd");
        assert_eq!(app_name("C:\\Games\\My Game.exe"), "My_Game.exe");
        assert_eq!(app_name("/opt/evil/ferro"), "app-ferro");
        assert_eq!(app_name(".."), "app-..");
        let ttys = vec!["/dev/pts/3".to_owned()];
        let p = Policy::standard("/opt/tool/run", "/home/me/Documents", "/ProgramData/run", "/home/me", &ttys);
        assert!(p.read_exec.contains(&"/opt/tool".to_owned()));
        assert!(p.read_write.contains(&"/home/me/Documents".to_owned()) && p.network == NetMode::Ask);
        assert!(p.read_write.contains(&"/dev/pts/3".to_owned()) && !p.read_write.contains(&"/dev/pts".to_owned()));
    }

    #[test]
    fn home_and_settings_are_never_writable_as_cwd() {
        let p = Policy::standard("/opt/tool/run", "/home/me", "/ProgramData/run", "/home/me", &[]);
        assert!(!p.read_write.contains(&"/home/me".to_owned()) && p.read.contains(&"/home/me".to_owned()));
        for dir in
            ["/home", "/home/me", "/home/me/.config", "/home/me/.ssh/x", "/ProgramData", "/ProgramData/ferro", "/etc", "/usr/bin", "/sys/fs/cgroup"]
        {
            assert!(!cwd_writable(dir, "/home/me"), "{dir}");
        }
        for dir in ["/home/me/Documents", "/home/me/Projects/game", "/mnt/usb", "/ProgramData/myapp"] {
            assert!(cwd_writable(dir, "/home/me"), "{dir}");
        }
    }

    #[test]
    fn budgets_as_typed() {
        assert_eq!(parse_budget("5G"), Some(Some(5 << 30)));
        assert_eq!(parse_budget("5 gb"), Some(Some(5 << 30)));
        assert_eq!(parse_budget("50MB"), Some(Some(50 << 20)));
        assert_eq!(parse_budget("512"), Some(Some(512 << 20)));
        assert_eq!(parse_budget("1.5g"), Some(Some(3 << 29)));
        assert_eq!(parse_budget("No Limit"), Some(None));
        assert_eq!(parse_budget("lots"), None);
        assert_eq!(parse_budget("-5g"), None);
    }

    #[test]
    fn root_is_never_granted() {
        let p = Policy::standard("/prog", "/", "/ProgramData/prog", "/home/me", &[]);
        let all: Vec<&String> = p.read_exec.iter().chain(&p.read).chain(&p.read_write).collect();
        assert!(all.iter().all(|s| s.as_str() != "/"), "{all:?}");
    }
}
