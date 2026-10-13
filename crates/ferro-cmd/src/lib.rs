//! The FerroOS command interpreter: a small DOS-flavored shell.
//!
//! It is I/O-agnostic so one implementation serves three places: init's
//! rescue console on the serial line, the `ferro-cmd` binary behind the
//! desktop's Command Prompt (on a pty), and an in-process session in the
//! desktop preview. Every path argument goes through ferro-path, so
//! `dir C:\bin` just works.

use ferro_path::DriveTable;
use ferro_sys::{MemInfo, HEAVY_IDLE_BUDGET_KB, IDLE_BUDGET_KB};
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
  SLEEP           encrypted sleep (evicts keys, clears caches)
  <program> [args]  run a program in the sandbox (searched in C:\\bin)
  RUN [/NET | /NONET] [/MEM:64M] [/UNSAFE] <program> [args]
                  /NET or /NONET decide network now (otherwise you're asked),
                  /MEM sets the RAM budget (or NONE), /UNSAFE skips the sandbox
  PERMS [/FORGET <app> | /FORGET ALL]  show or erase remembered app choices
  VPN [ON | OFF | IMPORT <file>]      WireGuard VPN (import a provider's profile)
  TOR [ON [PERSIST] | OFF | BRIDGE <cmd>]  Tor-only internet (PERSIST runs on reboot)
  WIFI [LIST | JOIN <network> | OFF]  Wi-Fi (JOIN asks for the password)
  DEBIAN <command>  run a command as root inside Debian, e.g.
                  DEBIAN apt update   then   DEBIAN apt install -y <package>
                  DEBIAN /RESET forgets everything installed, at the next start
  BRIGHTNESS [0-100]  adjust or display screen brightness
  BATTERY             display battery and power status
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

    /// Handles SLEEP (encrypted suspend). Returns a message if this host can't.
    fn sleep(&mut self) -> Option<String> {
        None
    }

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

    fn sleep(&mut self) -> Option<String> {
        #[cfg(unix)]
        {
            if Path::new("/bin/systemctl").exists() || Path::new("/usr/bin/systemctl").exists() {
                let _ = std::process::Command::new("systemctl").arg("suspend").status();
                return None;
            }
            let _ = std::fs::write("/proc/sys/vm/drop_caches", "3");
            let _ = std::fs::write("/proc/sys/vm/compact_memory", "1");
            let _ = std::fs::write("/sys/power/mem_sleep", "deep");
            if std::fs::write("/sys/power/state", "mem").is_ok() {
                return None;
            }
        }
        Some("Encrypted sleep is not supported on this host.".into())
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
    repl_in(r"C:\", first, input, out, host, drives)
}

/// Like [`repl_from`], starting in the folder `start` (a Windows-style path).
pub fn repl_in(
    start: &str,
    first: Option<&str>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    host: &mut dyn Host,
    drives: &DriveTable,
) -> io::Result<()> {
    let mut sh = Interp { cwd: start.to_owned(), drives, asking: None };
    if let Some(cmd) = first {
        writeln!(out, "{}>{cmd}", sh.cwd)?;
        if !sh.execute(cmd, out, host)? {
            return Ok(());
        }
    }
    let mut line = String::new();
    loop {
        // A command waiting for the vault password has asked for it already.
        if sh.asking.is_none() {
            write!(out, "{}>", sh.cwd)?;
        }
        out.flush()?;
        line.clear();
        if input.read_line(&mut line)? == 0 {
            if sh.asking.is_some() {
                echo(true);
            }
            return Ok(());
        }
        let typed = line.trim_end_matches(['\r', '\n']);
        if let Some(request) = sh.asking.take() {
            echo(true);
            writeln!(out)?;
            if let Some(rest) = request.strip_prefix("WIFI-JOIN-HEAVY ") {
                let name = rest.trim_end_matches(&format!(" {PASSWORD_SLOT}"));
                wifi_join_heavy(name, typed, out)?;
            } else {
                let request = request.replace(PASSWORD_SLOT, &ferro_system::to_hex(typed));
                if request.starts_with("WIFI") {
                    wifi_send(&request, out)?;
                } else {
                    debian_send(&request, out)?;
                }
            }
            continue;
        }
        if !sh.execute(typed, out, host)? {
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
    /// A request to ferro-system that needs the vault password: the next
    /// line typed is it.
    asking: Option<String>,
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
                    dir(&win, &posix, self.drives, out)?;
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
            "shutdown" | "poweroff" | "reboot" | "restart" | "sleep" | "standby" | "suspend" => {
                let lower = cmd.to_ascii_lowercase();
                if matches!(lower.as_str(), "sleep" | "standby" | "suspend") {
                    writeln!(out, "Entering encrypted sleep (evicting keys, purging memory caches)...")?;
                    if let Some(msg) = host.sleep() {
                        writeln!(out, "{msg}")?;
                    }
                } else {
                    let reboot = matches!(lower.as_str(), "reboot" | "restart");
                    if let Some(msg) = host.shutdown(reboot) {
                        writeln!(out, "{msg}")?;
                    }
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
            "vpn" | "tor" => tunnel(&cmd.to_ascii_lowercase(), &args, out)?,
            "debian" => self.asking = debian(line.trim_start()[cmd.len()..].trim(), out)?,
            "wifi" => self.asking = wifi(line.trim_start()[cmd.len()..].trim(), out)?,
            "battery" | "power" => power(out)?,
            "brightness" => brightness(arg, out)?,
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
fn tunnel(cmd: &str, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    use ferro_sandbox::store::{Store, DEFAULT_PATH};
    let vpn = cmd == "vpn";
    let mut store = Store::load(DEFAULT_PATH);
    let arg = args.first().copied();

    if !vpn && arg.is_some_and(|a| a.eq_ignore_ascii_case("BRIDGE")) {
        match args.get(1).map(|s| s.to_ascii_uppercase()).as_deref() {
            Some("ADD") => {
                let line = args[2..].join(" ");
                if line.is_empty() {
                    return writeln!(out, "Usage: TOR BRIDGE ADD <bridge-line>");
                }
                store.bridges.push(line);
                if let Err(e) = store.save() {
                    return writeln!(out, "Couldn't save bridge: {e}");
                }
                return writeln!(out, "Bridge added. Restart Tor for changes to take effect.");
            }
            Some("LIST") => {
                if store.bridges.is_empty() {
                    return writeln!(out, "No Tor bridges configured.");
                }
                writeln!(out, "Configured Tor bridges:")?;
                for (i, b) in store.bridges.iter().enumerate() {
                    writeln!(out, "  {}: {b}", i + 1)?;
                }
                return Ok(());
            }
            Some("CLEAR") => {
                store.bridges.clear();
                if let Err(e) = store.save() {
                    return writeln!(out, "Couldn't clear bridges: {e}");
                }
                return writeln!(out, "All Tor bridges cleared. Restart Tor for changes to take effect.");
            }
            _ => return writeln!(out, "Usage: TOR BRIDGE [ADD <line> | LIST | CLEAR]"),
        }
    }

    if let Some(a) = arg {
        let (on, persist) = match a.to_ascii_uppercase().as_str() {
            "ON" => {
                let p = args.get(1).is_some_and(|s| s.eq_ignore_ascii_case("PERSIST") || s.eq_ignore_ascii_case("/PERSIST"));
                (true, p)
            }
            "OFF" => (false, false),
            "STATUS" => (store.switches.tor, false),
            _ => return writeln!(out, "Usage: {} [ON [PERSIST] | OFF]", cmd.to_ascii_uppercase()),
        };
        if !a.eq_ignore_ascii_case("STATUS") {
            if vpn {
                store.switches.vpn = on;
                if let Err(e) = store.save() {
                    return writeln!(out, "Couldn't save the setting: {e}");
                }
            } else {
                // If persist is requested: write tor=on to choices.conf.
                // If turning ON without persist: keep choices.conf with tor=off so reboot starts OFF!
                // If turning OFF: write tor=off to choices.conf.
                store.switches.tor = persist;
                if let Err(e) = store.save() {
                    return writeln!(out, "Couldn't save the setting: {e}");
                }
            }
            let sw = store.switches;
            let b = |v: bool| if v { 1 } else { 0 };
            let tor_active = if vpn { sw.tor } else { on };
            let line = format!("SWITCHES {} {} {} {} {}", b(sw.network), b(sw.microphone), b(sw.camera), b(sw.vpn), b(tor_active));
            if let Err(e) = system_request(&line) {
                return writeln!(out, "Saved, but couldn't apply it now: {e}");
            }
            if vpn {
                writeln!(out, "The VPN is {}.", if on { "on" } else { "off" })?;
            } else if on {
                if persist {
                    writeln!(out, "Tor is on and will persist across reboots.")?;
                } else {
                    writeln!(out, "Tor is on for this session only. (Use 'TOR ON PERSIST' to enable on reboot).")?;
                }
            } else {
                writeln!(out, "Tor is off.")?;
            }
            std::thread::sleep(std::time::Duration::from_secs(3)); // let it report
        }
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
        let tor = read("/run/ferro/tor-status");
        if get(&tor, "state") == "off" || (!sw.tor && !Path::new(ferro_net::TOR_FILE).exists()) {
            if !store.bridges.is_empty() {
                writeln!(out, "Tor: off ({} bridge(s) configured).", store.bridges.len())?;
            } else {
                writeln!(out, "Tor: off.")?;
            }
            return Ok(());
        }
        let via = if sw.vpn { " through the VPN" } else { "" };
        let mode = if sw.tor { " (persistent on reboot)" } else { " (session only)" };
        match get(&tor, "state").as_str() {
            "ready" => writeln!(out, "Tor: connected{via}{mode}. Only Tor reaches the internet; apps use SOCKS 127.0.0.1:9150.")?,
            "error" => writeln!(out, "Tor: {}{mode}", get(&tor, "detail"))?,
            _ => writeln!(out, "Tor: starting{via}{mode} ({})", get(&tor, "detail"))?,
        }
        if args.is_empty() {
            writeln!(out, "\nCommands:")?;
            writeln!(out, "  TOR ON           - Enable for this session only")?;
            writeln!(out, "  TOR ON PERSIST   - Enable and remember on reboot")?;
            writeln!(out, "  TOR OFF          - Disable Tor")?;
            writeln!(out, "  TOR BRIDGE LIST  - Show configured bridges")?;
            writeln!(out, "  TOR BRIDGE ADD   - Add a pluggable transport bridge")?;
            writeln!(out, "  TOR BRIDGE CLEAR - Remove all bridges")?;
        }
        Ok(())
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

/// WIFI: shows, lists, joins or leaves Wi-Fi networks through ferro-system
/// (on FerroOS Lite) or through NetworkManager/nmcli (on FerroOS Heavy).
/// Returns the request to repeat with the network's password, for JOIN.
fn wifi(args: &str, out: &mut dyn Write) -> io::Result<Option<String>> {
    if is_heavy() {
        return wifi_heavy(args, out);
    }
    let (verb, name) = args.split_once(' ').unwrap_or((args, ""));
    let name = name.trim().trim_matches('"');
    match verb.to_ascii_uppercase().as_str() {
        "" => match system_request("WIFI") {
            Ok(line) => writeln!(out, "{line}")?,
            Err(e) => writeln!(out, "{e}")?,
        },
        "LIST" => match system_request("WIFI-SCAN") {
            Ok(list) if list.is_empty() => writeln!(out, "No networks in range.")?,
            Ok(list) => {
                for (name, signal) in list.split(';').filter_map(|n| n.split_once('|')) {
                    writeln!(out, "  {signal:>4} dBm  {name}")?;
                }
            }
            Err(e) => writeln!(out, "{e}")?,
        },
        "JOIN" if !name.is_empty() => {
            write!(out, "Password for {name} (Enter for none): ")?;
            out.flush()?;
            echo(false);
            return Ok(Some(format!("WIFI-JOIN {} {PASSWORD_SLOT}", ferro_system::to_hex(name))));
        }
        "OFF" => match system_request("WIFI-OFF") {
            Ok(_) => writeln!(out, "Wi-Fi is off and the network is forgotten.")?,
            Err(e) => writeln!(out, "{e}")?,
        },
        _ => writeln!(out, "Usage: WIFI [LIST | JOIN <network> | OFF]")?,
    }
    Ok(None)
}

/// Sends a WIFI-JOIN that now carries its password.
fn wifi_send(request: &str, out: &mut dyn Write) -> io::Result<()> {
    match system_request(request) {
        Ok(_) => writeln!(out, "Saved. Connecting; WIFI shows how it's going."),
        Err(e) => writeln!(out, "{e}"),
    }
}

fn is_heavy() -> bool {
    fs::read_to_string("/etc/os-release").is_ok_and(|r| r.lines().any(|l| l == "ID=ferroos-heavy"))
}

fn wifi_heavy(args: &str, out: &mut dyn Write) -> io::Result<Option<String>> {
    let (verb, name) = args.split_once(' ').unwrap_or((args, ""));
    let name = name.trim().trim_matches('"');
    match verb.to_ascii_uppercase().as_str() {
        "" => {
            let output = std::process::Command::new("nmcli").args(["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "dev", "status"]).output();
            match output {
                Ok(o) => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    let mut found = false;
                    for line in text.lines() {
                        let mut parts = line.split(':');
                        let (dev, kind, state, conn) =
                            (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                        if kind == "wifi" {
                            found = true;
                            if state == "connected" {
                                writeln!(out, "Wi-Fi ({dev}): connected to {conn}")?;
                            } else {
                                writeln!(out, "Wi-Fi ({dev}): {state}. WIFI LIST shows networks, WIFI JOIN <name> connects.")?;
                            }
                        }
                    }
                    if !found {
                        writeln!(out, "No Wi-Fi card was found.")?;
                    }
                }
                Err(e) => writeln!(out, "nmcli: {e}")?,
            }
        }
        "LIST" => {
            let output = std::process::Command::new("nmcli").args(["-t", "-f", "SIGNAL,SSID", "dev", "wifi", "list"]).output();
            match output {
                Ok(o) => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    let mut count = 0;
                    for line in text.lines() {
                        if let Some((signal, ssid)) = line.split_once(':') {
                            if !ssid.is_empty() {
                                writeln!(out, "  {signal:>4} %  {ssid}")?;
                                count += 1;
                            }
                        }
                    }
                    if count == 0 {
                        writeln!(out, "No networks in range.")?;
                    }
                }
                Err(e) => writeln!(out, "nmcli: {e}")?,
            }
        }
        "JOIN" if !name.is_empty() => {
            write!(out, "Password for {name} (Enter for none): ")?;
            out.flush()?;
            echo(false);
            return Ok(Some(format!("WIFI-JOIN-HEAVY {name} {PASSWORD_SLOT}")));
        }
        "OFF" => {
            let _ = std::process::Command::new("nmcli").args(["radio", "wifi", "off"]).status();
            writeln!(out, "Wi-Fi is off.")?;
        }
        "ON" => {
            let _ = std::process::Command::new("nmcli").args(["radio", "wifi", "on"]).status();
            writeln!(out, "Wi-Fi is on.")?;
        }
        _ => writeln!(out, "Usage: WIFI [LIST | JOIN <network> | ON | OFF]")?,
    }
    Ok(None)
}

fn wifi_join_heavy(name: &str, password: &str, out: &mut dyn Write) -> io::Result<()> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["dev", "wifi", "connect", name]);
    if !password.is_empty() {
        cmd.args(["password", password]);
    }
    match cmd.status() {
        Ok(s) if s.success() => writeln!(out, "Connected to {name}."),
        Ok(_) => writeln!(out, "Could not connect to {name} (wrong password or out of range)."),
        Err(e) => writeln!(out, "nmcli: {e}"),
    }
}

fn power(out: &mut dyn Write) -> io::Result<()> {
    let ps_dir = Path::new("/sys/class/power_supply");
    if !ps_dir.exists() {
        return writeln!(out, "Power: AC connected (no battery subsystem)");
    }
    let mut found = false;
    if let Ok(entries) = fs::read_dir(ps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default().trim().to_owned();
            let status = fs::read_to_string(path.join("status")).unwrap_or_default().trim().to_owned();
            if kind == "Battery" {
                found = true;
                let cap = fs::read_to_string(path.join("capacity")).unwrap_or_default().trim().to_owned();
                let status_str = if status.is_empty() { "Unknown" } else { &status };
                writeln!(out, "  Battery {name}: {cap}% ({status_str})")?;
            } else if kind == "Mains" {
                let online = fs::read_to_string(path.join("online")).unwrap_or_default().trim().to_owned();
                let state = if online == "1" { "Connected" } else { "Disconnected" };
                writeln!(out, "  AC Power ({name}): {state}")?;
            }
        }
    }
    if !found {
        writeln!(out, "  AC power connected (no battery present)")?;
    }
    Ok(())
}

fn brightness(arg: Option<&str>, out: &mut dyn Write) -> io::Result<()> {
    let bl_dir = Path::new("/sys/class/backlight");
    if !bl_dir.exists() {
        return writeln!(out, "Brightness: no backlight devices found");
    }
    let mut entries_found = Vec::new();
    if let Ok(entries) = fs::read_dir(bl_dir) {
        for entry in entries.flatten() {
            entries_found.push(entry.path());
        }
    }
    if entries_found.is_empty() {
        return writeln!(out, "Brightness: no backlight devices found");
    }
    for dev in entries_found {
        let name = dev.file_name().unwrap_or_default().to_string_lossy();
        let max: u64 = fs::read_to_string(dev.join("max_brightness")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(100);
        let curr: u64 = fs::read_to_string(dev.join("brightness")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if let Some(val_str) = arg {
            if let Ok(pct) = val_str.parse::<u64>() {
                let pct = pct.min(100);
                let target = (pct * max) / 100;
                match fs::write(dev.join("brightness"), target.to_string()) {
                    Ok(_) => writeln!(out, "  {name}: brightness set to {pct}%")?,
                    Err(e) => writeln!(out, "  {name}: failed to set brightness: {e} (run with sudo)")?,
                }
            } else {
                writeln!(out, "Usage: BRIGHTNESS [0-100]")?;
                return Ok(());
            }
        } else {
            let pct = (curr * 100).checked_div(max).unwrap_or(0);
            writeln!(out, "  {name}: {pct}% (level {curr}/{max})")?;
        }
    }
    Ok(())
}

/// Stands in for the vault password in a request that's waiting for it.
const PASSWORD_SLOT: &str = "<password>";

/// Shows or hides what's typed at the terminal (for passwords).
fn echo(on: bool) {
    #[cfg(unix)]
    // SAFETY: termios calls on stdin with a zeroed, then filled, struct;
    // they fail harmlessly when stdin isn't a terminal.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            if on {
                t.c_lflag |= libc::ECHO;
            } else {
                t.c_lflag &= !libc::ECHO;
            }
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
    #[cfg(not(unix))]
    let _ = on;
}

/// DEBIAN: has ferro-system run a command as root inside Debian, and shows
/// its output as it comes. Returns the request to repeat with the vault
/// password, when ferro-system wants it.
fn debian(command: &str, out: &mut dyn Write) -> io::Result<Option<String>> {
    if command.is_empty() {
        writeln!(out, "Usage: DEBIAN <command>, e.g. DEBIAN apt install -y nano")?;
        return Ok(None);
    }
    let reset = command.eq_ignore_ascii_case("/RESET");
    let request = if reset { "DEBIAN-RESET".to_owned() } else { format!("DEBIAN {command}") };
    match system_request(&request) {
        Err(e) if e.to_string() == ferro_system::layer::WRONG_PASSWORD => {
            write!(out, "Vault password: ")?;
            out.flush()?;
            echo(false);
            Ok(Some(if reset { format!("DEBIAN-RESET {PASSWORD_SLOT}") } else { format!("DEBIAN-AS {PASSWORD_SLOT} {command}") }))
        }
        Err(e) => writeln!(out, "{e}").map(|_| None),
        Ok(_) if reset => writeln!(out, "Debian will be as it shipped at the next start.").map(|_| None),
        Ok(_) => debian_follow(out).map(|_| None),
    }
}

/// Sends a request that carries the vault password.
fn debian_send(request: &str, out: &mut dyn Write) -> io::Result<()> {
    match system_request(request) {
        Err(e) if e.to_string() == ferro_system::layer::WRONG_PASSWORD => writeln!(out, "Wrong password."),
        Err(e) => writeln!(out, "{e}"),
        Ok(_) if request.starts_with("DEBIAN-RESET") => writeln!(out, "Debian will be as it shipped at the next start."),
        Ok(_) => debian_follow(out),
    }
}

/// Shows a running DEBIAN command's output until it ends.
fn debian_follow(out: &mut dyn Write) -> io::Result<()> {
    let mut shown = 0;
    loop {
        let status = fs::read_to_string(ferro_system::layer::STATUS).unwrap_or_default();
        let log = fs::read(ferro_system::layer::LOG).unwrap_or_default();
        if log.len() > shown {
            out.write_all(&log[shown..])?;
            out.flush()?;
            shown = log.len();
        }
        if !status.contains("state=working") {
            if let Some(code) = status.lines().find_map(|l| l.strip_prefix("code=")).filter(|c| *c != "0") {
                writeln!(out, "(exited with code {code})")?;
            }
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
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

fn dir(win: &str, posix: &str, drives: &DriveTable, out: &mut dyn Write) -> io::Result<()> {
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
            let name = e.file_name().to_string_lossy().into_owned();
            (is_dir, drives.shown_name(posix, &name).to_owned(), md.map_or(0, |m| m.len()))
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
    let budget_kb = if is_heavy() { HEAVY_IDLE_BUDGET_KB } else { IDLE_BUDGET_KB };
    let verdict = if footprint <= budget_kb { "within" } else { "OVER" };
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
    writeln!(out, "  Total footprint:     {footprint:>7} KB  ({verdict} the {} MB idle budget)\n", budget_kb / 1024)?;
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
/// The drive table for the system this is running on: FerroOS Heavy shows
/// `/home` as `C:\Users`.
pub fn system_drives() -> ferro_path::DriveTable {
    let heavy = fs::read_to_string("/etc/os-release").is_ok_and(|r| r.lines().any(|l| l == "ID=ferroos-heavy"));
    if heavy {
        ferro_path::DriveTable::heavy()
    } else {
        ferro_path::DriveTable::default()
    }
}

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
        Some(k) if k == "/K" || k == "/C" || k == "-C" => Some(args[1..].join(" ")),
        _ => None,
    };

    let mut drives = system_drives();
    if cfg!(windows) {
        drives.mount('C', "C:/");
    } else if std::path::Path::new("/mnt/d").is_dir() {
        drives.mount('D', "/mnt/d");
    }
    // On FerroOS Heavy the prompt opens in your own folder, as cmd does.
    let heavy = drives.to_windows("/home").is_some_and(|w| w.ends_with("Users"));
    let start = std::env::var("HOME").ok().filter(|_| heavy).and_then(|home| drives.to_windows(&home)).unwrap_or_else(|| r"C:\".to_owned());
    crate::ignore_interrupt();
    let _ = crate::banner(&mut out);
    let result = crate::repl_in(&start, first.as_deref(), &mut stdin.lock(), &mut out, &mut crate::ProcessHost, &drives);
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
        fn sleep(&mut self) -> Option<String> {
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

    #[test]
    fn power_and_brightness_commands_run() {
        let out = session("power\nbrightness\nexit\n", &DriveTable::default());
        assert!(out.contains("Power") || out.contains("power") || out.contains("AC"), "{out}");
        assert!(out.contains("Brightness"), "{out}");
    }

    #[test]
    fn encrypted_sleep_command() {
        let out = session("sleep\nexit\n", &DriveTable::default());
        assert!(out.contains("encrypted sleep"), "{out}");
    }
}
