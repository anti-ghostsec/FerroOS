//! The FerroOS command interpreter: a small DOS-flavored shell.
//!
//! It is I/O-agnostic so one implementation serves three places: init's
//! rescue console on the serial line, the `ferro-cmd` binary behind the
//! desktop's Command Prompt (on a pty), and an in-process session in the
//! desktop preview. Every path argument goes through ferro-path, so
//! `dir C:\bin` just works.

use ferro_path::DriveTable;
use ferro_sys::{MemInfo, IDLE_BUDGET_KB};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

const HELP: &str = "\
  DIR [path]      list a directory          CD [path]    change directory
  TYPE <file>     print a file              DRIVES       show drive mappings
  MEM [/DETAIL]   memory vs idle budget     PS           list processes
  VER             version info              CLS          clear the screen
  ECHO <text>     print text                EXIT         close this prompt
  SHUTDOWN        power off                 REBOOT       restart
  <program> [args]  run a program in the sandbox (searched in C:\\bin)
  RUN [/NET | /NONET] [/MEM:64M] [/UNSAFE] <program> [args]
                  /NET or /NONET decide network now (otherwise you're asked),
                  /MEM sets the RAM budget (or NONE), /UNSAFE skips the sandbox
  PERMS [/FORGET <app> | /FORGET ALL]  show or erase remembered app choices
  VPN [ON | OFF | IMPORT <file>]      WireGuard VPN (import a provider's profile)
  TOR [ON | OFF]  Tor-only internet
  MEMTEST <MB> [/MEM:size]  use memory inside the sandbox to test RAM budgets";

/// Per-run sandbox options from RUN's switches.
#[derive(Default)]
struct RunOpts {
    unsafe_run: bool,
    network: Option<bool>,
    memory: Option<String>,
    /// App name shown in Task Manager and budget messages.
    name: Option<String>,
}

/// What the interpreter needs from wherever it runs.
pub trait Host {
    /// Runs a program in directory `cwd` (POSIX) and waits for it. Hosts
    /// whose children share the terminal ignore `out` (it is flushed first);
    /// in-process hosts copy the program's output into it.
    fn run(&mut self, path: &str, args: &[&str], cwd: &str, out: &mut dyn Write) -> io::Result<i32>;

    /// Handles SHUTDOWN/REBOOT. Returns a message if this host can't.
    fn shutdown(&mut self, reboot: bool) -> Option<String>;

    /// The sandbox launcher programs should be started through, if any.
    fn launcher(&self) -> Option<String> {
        None
    }
}

/// Runs a host-side child with inherited stdio.
pub struct ProcessHost;

impl Host for ProcessHost {
    fn run(&mut self, path: &str, args: &[&str], cwd: &str, _out: &mut dyn Write) -> io::Result<i32> {
        let mut cmd = std::process::Command::new(path);
        cmd.args(args).current_dir(cwd);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // ferro-cmd ignores Ctrl+C itself; the program should not.
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::signal(libc::SIGINT, libc::SIG_DFL);
                    Ok(())
                });
            }
        }
        let status = cmd.status()?;
        Ok(status.code().unwrap_or(-1))
    }

    fn shutdown(&mut self, _reboot: bool) -> Option<String> {
        Some("Use Shut Down... on the Start menu.".into())
    }

    fn launcher(&self) -> Option<String> {
        let path = "/bin/ferro-run";
        (cfg!(target_os = "linux") && Path::new(path).is_file()).then(|| path.to_owned())
    }
}

/// `MEMTEST` worker: allocates and touches memory 1 MB at a time, so the
/// RAM budget (not lazy allocation) is what stops it.
pub fn memtest(mb: usize, out: &mut dyn Write) -> io::Result<()> {
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    for i in 1..=mb {
        blocks.push(vec![0xA5; 1 << 20]);
        if i % 8 == 0 || i == mb {
            writeln!(out, "  {i} MB in use")?;
            out.flush()?;
        }
    }
    writeln!(out, "  done: {mb} MB fit within the budget")
}

/// Ignores Ctrl+C (SIGINT) so it stops the running program, not the prompt.
pub fn ignore_interrupt() {
    #[cfg(unix)]
    // SAFETY: installing SIG_IGN has no preconditions.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
}

/// Reads commands until EOF or EXIT.
pub fn repl(input: &mut dyn BufRead, out: &mut dyn Write, host: &mut dyn Host, drives: &DriveTable) -> io::Result<()> {
    repl_from(None, input, out, host, drives)
}

/// Like [`repl`], but first runs `first` (as `cmd /K` does), echoing it
/// after the prompt so the transcript reads naturally.
pub fn repl_from(first: Option<&str>, input: &mut dyn BufRead, out: &mut dyn Write, host: &mut dyn Host, drives: &DriveTable) -> io::Result<()> {
    let mut sh = Interp { cwd: r"C:\".to_owned(), drives };
    if let Some(cmd) = first {
        writeln!(out, "{}>{cmd}", sh.cwd)?;
        if !sh.execute(cmd, out, host)? {
            return Ok(());
        }
    }
    let mut line = String::new();
    loop {
        write!(out, "{}>", sh.cwd)?;
        out.flush()?;
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if !sh.execute(line.trim_end_matches(['\r', '\n']), out, host)? {
            return Ok(());
        }
    }
}

pub fn banner(out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "\nFerroOS {} Command Prompt. Type HELP for commands.\n", env!("CARGO_PKG_VERSION"))
}

struct Interp<'a> {
    cwd: String,
    drives: &'a DriveTable,
}

impl Interp<'_> {
    /// Returns `false` when the session should end.
    fn execute(&mut self, line: &str, out: &mut dyn Write, host: &mut dyn Host) -> io::Result<bool> {
        let words = split_words(line);
        let Some((cmd, rest)) = words.split_first() else { return Ok(true) };
        let cmd = cmd.as_str();
        let args: Vec<&str> = rest.iter().map(String::as_str).collect();
        let arg = args.first().copied();

        match cmd.to_ascii_lowercase().as_str() {
            "help" | "?" => writeln!(out, "{HELP}")?,
            "ver" => {
                let kernel = ferro_sys::kernel_release().unwrap_or_else(|| "n/a".into());
                writeln!(out, "\nFerroOS {} (Linux {kernel})\n", env!("CARGO_PKG_VERSION"))?;
            }
            "echo" => writeln!(out, "{}", line.trim_start()[cmd.len()..].trim_start())?,
            "mem" => mem(out, arg.is_some_and(|a| a.eq_ignore_ascii_case("/DETAIL")))?,
            "ps" | "tasklist" => {
                writeln!(out, "  PID   RSS KB  USER  NAME")?;
                for p in ferro_sys::processes() {
                    let user = if p.uid == 0 { "root" } else { "user" };
                    writeln!(out, "{:>5} {:>8}  {user:<4}  {}", p.pid, p.rss_kb, p.name)?;
                }
            }
            "drives" => {
                for (letter, root) in self.drives.drives() {
                    writeln!(out, "  {letter}:\\  ->  {root}")?;
                }
            }
            "dir" | "ls" => {
                if let Some((win, posix)) = self.resolve(arg.unwrap_or("."), out)? {
                    dir(&win, &posix, out)?;
                }
            }
            "cd" | "chdir" => match arg {
                None => writeln!(out, "{}", self.cwd)?,
                Some(a) => {
                    if let Some((win, posix)) = self.resolve(a, out)? {
                        // The interpreter tracks its own cwd rather than the
                        // process's, so it can share a process (preview).
                        if Path::new(&posix).is_dir() {
                            self.cwd = win;
                        } else {
                            writeln!(out, "Invalid directory - {win}")?;
                        }
                    }
                }
            },
            "type" | "cat" => match arg {
                None => writeln!(out, "Required parameter missing")?,
                Some(a) => {
                    if let Some((win, posix)) = self.resolve(a, out)? {
                        match fs::read(&posix) {
                            Ok(bytes) => writeln!(out, "{}", String::from_utf8_lossy(&bytes))?,
                            Err(e) => writeln!(out, "Cannot read {win}: {e}")?,
                        }
                    }
                }
            },
            "cls" | "clear" => write!(out, "\x1b[2J\x1b[H")?,
            "exit" => return Ok(false),
            "shutdown" | "poweroff" | "reboot" | "restart" => {
                let reboot = matches!(cmd.to_ascii_lowercase().as_str(), "reboot" | "restart");
                if let Some(msg) = host.shutdown(reboot) {
                    writeln!(out, "{msg}")?;
                }
            }
            "run" | "start" => {
                let mut opts = RunOpts::default();
                let mut rest = args.as_slice();
                while let Some(sw) = rest.first().filter(|a| a.starts_with('/') && a.len() > 1 && !a[1..].contains('/')) {
                    let up = sw.to_ascii_uppercase();
                    match up.as_str() {
                        "/NET" => opts.network = Some(true),
                        "/NONET" => opts.network = Some(false),
                        "/UNSAFE" => opts.unsafe_run = true,
                        _ if up.starts_with("/MEM:") => opts.memory = Some(sw[5..].to_owned()),
                        _ => break,
                    }
                    rest = &rest[1..];
                }
                match rest.split_first() {
                    Some((prog, prog_args)) => self.exec(prog, prog_args, out, host, &opts)?,
                    None => writeln!(out, "Required parameter missing")?,
                }
            }
            "perms" => perms(&args, out)?,
            "vpn" if arg.is_some_and(|a| a.eq_ignore_ascii_case("IMPORT")) => match args.get(1) {
                Some(f) => {
                    if let Some((_, posix)) = self.resolve(f, out)? {
                        vpn_import(&posix, out)?;
                    }
                }
                None => writeln!(out, "Usage: VPN IMPORT <WireGuard file>")?,
            },
            "vpn" | "tor" => tunnel(&cmd.to_ascii_lowercase(), arg, out)?,
            "memtest" => {
                let Some(mb) = arg.and_then(|a| a.parse::<usize>().ok()) else {
                    return writeln!(out, "Usage: MEMTEST <MB> [/MEM:size]").map(|_| true);
                };
                let memory = args.iter().find_map(|a| a.get(..5).filter(|p| p.eq_ignore_ascii_case("/MEM:")).map(|_| a[5..].to_owned()));
                // The installed name, not current_exe(): on FerroOS that is the
                // shared multicall binary, which picks its role from the name.
                let exe = if Path::new("/bin/ferro-cmd").exists() {
                    "/bin/ferro-cmd".to_owned()
                } else {
                    std::env::current_exe()?.to_string_lossy().into_owned()
                };
                let opts = RunOpts { memory, name: Some("memtest".into()), ..RunOpts::default() };
                let mb_arg = mb.to_string();
                self.launch(&exe, &["--memtest", &mb_arg], out, host, &opts)?;
            }
            _ => self.exec(cmd, &args, out, host, &RunOpts::default())?,
        }
        Ok(true)
    }

    /// Resolves `arg` against the cwd into (canonical Windows path, POSIX path).
    fn resolve(&self, arg: &str, out: &mut dyn Write) -> io::Result<Option<(String, String)>> {
        let has_drive = arg.as_bytes().get(1) == Some(&b':');
        let joined = if has_drive || arg.starts_with(['\\', '/']) { arg.to_owned() } else { ferro_path::join(&self.cwd, arg) };
        match self.drives.to_posix(&joined) {
            Ok(posix) => Ok(Some((self.drives.to_windows(&posix).unwrap_or(joined), posix))),
            Err(e) => {
                writeln!(out, "{e}")?;
                Ok(None)
            }
        }
    }

    fn exec(&self, cmd: &str, args: &[&str], out: &mut dyn Write, host: &mut dyn Host, opts: &RunOpts) -> io::Result<()> {
        let candidates = if cmd.contains(['\\', '/', ':']) {
            vec![cmd.to_owned()]
        } else {
            vec![ferro_path::join(&self.cwd, cmd), ferro_path::join(r"C:\bin", cmd)]
        };
        let found = candidates.iter().filter_map(|c| self.drives.to_posix(c).ok()).find(|p| Path::new(p).is_file());
        let Some(path) = found else { return writeln!(out, "Bad command or file name") };
        self.launch(&path, args, out, host, opts)
    }

    /// Starts `path` through the host's sandbox launcher unless /UNSAFE.
    fn launch(&self, path: &str, args: &[&str], out: &mut dyn Write, host: &mut dyn Host, opts: &RunOpts) -> io::Result<()> {
        let cwd = self.drives.to_posix(&self.cwd).unwrap_or_else(|_| "/".into());
        let mut argv: Vec<String> = Vec::new();
        let program = match host.launcher().filter(|_| !opts.unsafe_run) {
            Some(launcher) => {
                match opts.network {
                    Some(true) => argv.push("--net".into()),
                    Some(false) => argv.push("--no-net".into()),
                    None => {}
                }
                if let Some(m) = &opts.memory {
                    argv.extend(["--mem".into(), m.clone()]);
                }
                if let Some(n) = &opts.name {
                    argv.extend(["--name".into(), n.clone()]);
                }
                argv.extend(["--".into(), path.to_owned()]);
                launcher
            }
            None => {
                if opts.unsafe_run {
                    writeln!(out, "(running without the sandbox)")?;
                }
                path.to_owned()
            }
        };
        argv.extend(args.iter().map(|a| a.to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        out.flush()?;
        let name = opts.name.as_deref().unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or(path));
        match host.run(&program, &argv, &cwd, out) {
            Ok(0) => Ok(()),
            Ok(code) => writeln!(out, "({name} exited with code {code})"),
            Err(e) => writeln!(out, "Cannot run {name}: {e}"),
        }
    }
}

/// VPN and TOR: show the state, or switch them (remembered like the tray's
/// kill switches, enforced by ferro-system and ferro-net).
fn tunnel(cmd: &str, arg: Option<&str>, out: &mut dyn Write) -> io::Result<()> {
    use ferro_sandbox::store::{Store, DEFAULT_PATH};
    let vpn = cmd == "vpn";
    let mut store = Store::load(DEFAULT_PATH);
    if let Some(a) = arg {
        let on = match a.to_ascii_uppercase().as_str() {
            "ON" => true,
            "OFF" => false,
            _ => return writeln!(out, "Usage: {} [ON | OFF]", cmd.to_ascii_uppercase()),
        };
        if vpn {
            store.switches.vpn = on;
        } else {
            store.switches.tor = on;
        }
        if let Err(e) = store.save() {
            return writeln!(out, "Couldn't save the setting: {e}");
        }
        let sw = store.switches;
        let b = |v: bool| if v { 1 } else { 0 };
        let line = format!("SWITCHES {} {} {} {} {}", b(sw.network), b(sw.microphone), b(sw.camera), b(sw.vpn), b(sw.tor));
        if let Err(e) = system_request(&line) {
            return writeln!(out, "Saved, but couldn't apply it now: {e}");
        }
        writeln!(out, "{} is {}.", if vpn { "The VPN" } else { "Tor" }, if on { "on" } else { "off" })?;
        std::thread::sleep(std::time::Duration::from_secs(3)); // let it report
    }
    let read = |p: &str| fs::read_to_string(p).unwrap_or_default();
    let get = |text: &str, k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap_or("").to_owned();
    let net = read("/run/ferro/net-status");
    let sw = store.switches;
    if vpn {
        if !sw.vpn {
            return match fs::read_to_string(ferro_net::VPN_CONFIG).ok().and_then(|t| ferro_net::wg::parse(&t).ok()) {
                Some(cfg) => writeln!(out, "VPN: off (profile for {} saved; VPN ON to use it)", cfg.peers[0].endpoint),
                None => writeln!(out, "VPN: off. Get your provider's WireGuard file, then VPN IMPORT <file> and VPN ON."),
            };
        }
        match get(&net, "vpn").as_str() {
            "connected" => writeln!(
                out,
                "VPN: connected to {} (handshake {} s ago, {} KB in, {} KB out)",
                get(&net, "endpoint"),
                get(&net, "handshake"),
                get(&net, "rx").parse::<u64>().unwrap_or(0) / 1024,
                get(&net, "tx").parse::<u64>().unwrap_or(0) / 1024
            ),
            "connecting" => writeln!(out, "VPN: connecting to {}; nothing goes out until it answers", get(&net, "endpoint")),
            "stalled" => writeln!(out, "VPN: {} hasn't answered for {} s; nothing goes out meanwhile", get(&net, "endpoint"), get(&net, "silent")),
            "blocked" => writeln!(out, "VPN: internet blocked: {}", get(&net, "detail")),
            _ => writeln!(out, "VPN: starting"),
        }
    } else {
        if !sw.tor {
            return writeln!(out, "Tor: off.");
        }
        let tor = read("/run/ferro/tor-status");
        let via = if sw.vpn { " through the VPN" } else { "" };
        match get(&tor, "state").as_str() {
            "ready" => writeln!(out, "Tor: connected{via}. Only Tor reaches the internet; apps use SOCKS 127.0.0.1:9150."),
            "error" => writeln!(out, "Tor: {}", get(&tor, "detail")),
            _ => writeln!(out, "Tor: starting{via} ({})", get(&tor, "detail")),
        }
    }
}

/// VPN IMPORT: checks a provider's WireGuard file and installs it as the
/// VPN profile, readable only by you (it holds your private key).
fn vpn_import(path: &str, out: &mut dyn Write) -> io::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return writeln!(out, "Can't read it: {e}"),
    };
    let cfg = match ferro_net::wg::parse(&text) {
        Ok(c) => c,
        Err(e) => return writeln!(out, "Not a usable WireGuard profile: {e}"),
    };
    let dest = std::path::Path::new(ferro_net::VPN_CONFIG);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(dest, &text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o600))?;
    }
    writeln!(out, "VPN profile saved (server {}). Turn it on with VPN ON.", cfg.peers[0].endpoint)
}

/// One request to ferro-system (see its protocol).
fn system_request(line: &str) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader};
        let mut s = std::os::unix::net::UnixStream::connect("/run/ferro/system.sock")?;
        writeln!(s, "{line}")?;
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply)?;
        let reply = reply.trim().to_owned();
        match reply.strip_prefix("ERR ") {
            Some(e) => Err(io::Error::other(e.to_owned())),
            None => Ok(reply),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = line;
        Err(io::Error::other("FerroOS's system service isn't on this host"))
    }
}

/// PERMS: lists remembered per-app choices, or erases them.
fn perms(args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    use ferro_sandbox::store::{NetChoice, Store, DEFAULT_PATH};
    let mut store = Store::load(DEFAULT_PATH);
    if args.first().is_some_and(|a| a.eq_ignore_ascii_case("/FORGET")) {
        match args.get(1) {
            Some(a) if a.eq_ignore_ascii_case("ALL") => store.forget_all(),
            Some(app) => store.forget(app),
            None => return writeln!(out, "Usage: PERMS /FORGET <app> | /FORGET ALL"),
        }
        return match store.save() {
            Ok(()) => writeln!(out, "Forgotten."),
            Err(e) => writeln!(out, "Couldn't save: {e}"),
        };
    }
    if store.apps.is_empty() {
        return writeln!(out, "No remembered choices. Apps will ask before using the network.");
    }
    writeln!(out, "\n  {:<24} {:<10} RAM budget", "App", "Network")?;
    for (name, c) in &store.apps {
        let net = match c.network {
            Some(NetChoice::Allow) => "always",
            Some(NetChoice::Deny) => "never",
            None => "ask",
        };
        let mem = match c.memory {
            Some(Some(b)) if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
            Some(Some(b)) => format!("{} MB", b >> 20),
            Some(None) => "none".into(),
            None => "default".into(),
        };
        writeln!(out, "  {name:<24} {net:<10} {mem}")?;
    }
    writeln!(out)
}

/// Splits a command line on whitespace, keeping "quoted parts" together
/// (so `"C:\My Games\run"` is one argument).
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut cur, mut quoted, mut any) = (String::new(), false, false);
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(cur);
    }
    words
}

fn dir(win: &str, posix: &str, out: &mut dyn Write) -> io::Result<()> {
    let entries = match fs::read_dir(posix) {
        Ok(d) => d,
        Err(e) => return writeln!(out, "Cannot list {win}: {e}"),
    };
    writeln!(out, "\n Directory of {win}\n")?;
    let mut rows: Vec<(bool, String, u64)> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let md = e.metadata().ok();
            let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
            (is_dir, e.file_name().to_string_lossy().into_owned(), md.map_or(0, |m| m.len()))
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let (mut files, mut bytes) = (0, 0);
    for (is_dir, name, len) in &rows {
        if *is_dir {
            writeln!(out, "{name:<32} <DIR>")?;
        } else {
            files += 1;
            bytes += len;
            writeln!(out, "{name:<32} {len:>12}")?;
        }
    }
    writeln!(out, "{files:>10} file(s) {bytes:>14} bytes\n{:>10} dir(s)\n", rows.len() - files)
}

fn mem(out: &mut dyn Write, detail: bool) -> io::Result<()> {
    let Some(m) = MemInfo::read() else { return writeln!(out, "/proc/meminfo unavailable on this host") };
    let procs = ferro_sys::processes();
    // Private memory only: shared program code is already in the root file
    // system's share of the footprint.
    let apps_kb: u64 = procs.iter().map(|p| p.private_kb).sum();
    let footprint = m.footprint_kb();
    let verdict = if footprint <= IDLE_BUDGET_KB { "within" } else { "OVER" };
    writeln!(out)?;
    if let Some(phys) = m.physical_kb {
        writeln!(out, "  Physical RAM:        {phys:>7} KB")?;
        writeln!(out, "  Kernel reserved:     {:>7} KB  (image + boot, not in MemTotal)", m.kernel_reserved_kb().unwrap_or(0))?;
        writeln!(out, "    page tracking:     {:>7} KB  (1.6% of installed RAM; not counted below)", m.page_tracking_kb())?;
    }
    writeln!(out, "  Kernel-managed:      {:>7} KB  (MemTotal)", m.total_kb)?;
    writeln!(out, "  Available:           {:>7} KB", m.available_kb)?;
    writeln!(out, "  Free, kept aside:    {:>7} KB  (kernel reserve + per-CPU caches; not counted below)", m.reserve_kb)?;
    writeln!(out, "  FerroOS processes:   {apps_kb:>7} KB  ({} running, private memory)", procs.len())?;
    writeln!(out, "  Total footprint:     {footprint:>7} KB  ({verdict} the {} MB idle budget)\n", IDLE_BUDGET_KB / 1024)?;
    if detail {
        // Where the kernel-managed memory goes, straight from /proc/meminfo.
        let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let keys = [
            ("AnonPages", "app heaps and stacks"),
            ("Shmem", "RAM-backed files (root fs, tmpfs, display buffer)"),
            ("Cached", "page cache (includes Shmem)"),
            ("Buffers", "block device buffers"),
            ("SUnreclaim", "kernel objects (slab)"),
            ("SReclaimable", "kernel caches (reclaimable)"),
            ("KernelStack", "kernel stacks"),
            ("PageTables", "page tables"),
            ("Percpu", "per-CPU areas"),
            ("VmallocUsed", "vmalloc (modules, buffers)"),
        ];
        for (k, what) in keys {
            if let Some(v) = text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(':')).and_then(|v| v.split_whitespace().next()) {
                writeln!(out, "  {k:<13} {v:>7} KB  {what}")?;
            }
        }
        // Shmem split by RAM disk; the rest is GPU buffers.
        #[cfg(unix)]
        for line in fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
            let mut f = line.split_whitespace();
            let (Some(dev), Some(dir), Some(kind)) = (f.next(), f.next(), f.next()) else { continue };
            if !matches!(kind, "tmpfs" | "rootfs" | "ramfs" | "devtmpfs") {
                continue;
            }
            let Ok(c) = std::ffi::CString::new(dir) else { continue };
            // SAFETY: valid NUL-terminated path and a zeroed out-struct.
            let mut st: libc::statfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statfs(c.as_ptr(), &mut st) } == 0 {
                let used = (st.f_blocks - st.f_bfree) as u64 * st.f_bsize as u64 / 1024;
                writeln!(out, "    {:<11} {used:>7} KB  {dev} {dir}", "")?;
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

/// `ferro-cmd`: the command interpreter behind the desktop's Command Prompt.
///
/// ```text
/// ferro-cmd                interactive prompt
/// ferro-cmd /K <command>   run <command> first, then stay interactive
/// ferro-cmd --memtest <MB> MEMTEST's worker (runs inside the sandbox)
/// ```
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    let mut out = stdout.lock();

    if args.first().map(String::as_str) == Some("--memtest") {
        let mb = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(16);
        let _ = crate::memtest(mb, &mut out);
        return;
    }
    let first = match args.first().map(|a| a.to_ascii_uppercase()) {
        Some(k) if k == "/K" => Some(args[1..].join(" ")),
        _ => None,
    };

    let mut drives = ferro_path::DriveTable::default();
    if cfg!(windows) {
        drives.mount('C', "C:/");
    } else if std::path::Path::new("/mnt/d").is_dir() {
        drives.mount('D', "/mnt/d");
    }
    crate::ignore_interrupt();
    let _ = crate::banner(&mut out);
    let result = crate::repl_from(first.as_deref(), &mut stdin.lock(), &mut out, &mut crate::ProcessHost, &drives);
    if let Err(e) = result {
        eprintln!("ferro-cmd: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost;
    impl Host for NoHost {
        fn run(&mut self, _: &str, _: &[&str], _: &str, _: &mut dyn Write) -> io::Result<i32> {
            Ok(0)
        }
        fn shutdown(&mut self, _: bool) -> Option<String> {
            Some("no".into())
        }
    }

    fn session(script: &str, drives: &DriveTable) -> String {
        let mut out = Vec::new();
        repl(&mut script.as_bytes(), &mut out, &mut NoHost, drives).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn echo_cd_dir_and_exit() {
        let dir = std::env::temp_dir().join(format!("ferro-cmd-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("hello.txt"), "hi").unwrap();
        let mut drives = DriveTable::empty();
        drives.mount('C', &dir.to_string_lossy().replace('\\', "/"));

        let out = session("echo  Hello   World\ncd sub\ncd ..\ndir\nexit\necho unreachable\n", &drives);
        assert!(out.contains("Hello   World"), "{out}");
        assert!(out.contains(r"C:\sub>"), "{out}");
        assert!(out.contains("hello.txt") && out.contains("sub") && out.contains("<DIR>"), "{out}");
        assert!(!out.contains("unreachable"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn quoted_words() {
        assert_eq!(split_words(r#"run /NET "C:\My Games\go" a  b"#), ["run", "/NET", r"C:\My Games\go", "a", "b"]);
        assert_eq!(split_words(r#"echo """#), ["echo", ""]);
    }

    #[test]
    fn unknown_command() {
        let out = session("frobnicate\n", &DriveTable::default());
        assert!(out.contains("Bad command or file name"));
    }
}
