//! `ferro-run`: starts a program inside the FerroOS sandbox.
//!
//! ```text
//! ferro-run [--mem 64M|none] [--net | --no-net] [--allow-read PATH]...
//!           [--allow-write PATH]... [--name NAME] -- PROGRAM [ARGS]...
//! ```
//!
//! It stays alive as a small supervisor: it answers the app's network
//! questions (asking the user the first time, remembering "always"/"never"),
//! forwards termination signals, reports when the app hit its RAM budget,
//! and removes the app's cgroup.

#[cfg(target_os = "linux")]
pub fn main() {
    linux_main::run()
}

#[cfg(not(target_os = "linux"))]
pub fn main() {
    // Desktop preview hosts have no Landlock/seccomp/cgroups: run plainly.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("usage: ferro-run [options] -- PROGRAM [ARGS]");
        std::process::exit(2);
    };
    eprintln!("ferro-run: sandboxing is only available on FerroOS; running unsandboxed");
    let status = std::process::Command::new(&args[sep + 1]).args(&args[sep + 2..]).status();
    std::process::exit(status.map_or(127, |s| s.code().unwrap_or(1)));
}

#[cfg(target_os = "linux")]
mod linux_main {
    use crate::linux::{self, AppCgroup, Ruleset};
    use crate::store::{NetChoice, Store, DEFAULT_PATH};
    use crate::{app_name, parse_size, NetMode, Policy};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicI32, Ordering};

    static CHILD: AtomicI32 = AtomicI32::new(0);

    extern "C" fn forward(sig: libc::c_int) {
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill is async-signal-safe.
            unsafe { libc::kill(pid, sig) };
        }
    }

    fn die(msg: &str) -> ! {
        eprintln!("ferro-run: {msg}");
        std::process::exit(126);
    }

    struct Args {
        memory: Option<Option<u64>>,
        network: Option<NetMode>,
        read: Vec<String>,
        write: Vec<String>,
        name: Option<String>,
        program: String,
        argv: Vec<String>,
    }

    fn parse() -> Args {
        let mut it = std::env::args().skip(1);
        let mut a = Args { memory: None, network: None, read: vec![], write: vec![], name: None, program: String::new(), argv: vec![] };
        while let Some(arg) = it.next() {
            let mut value = |flag: &str| it.next().unwrap_or_else(|| die(&format!("{flag} needs a value")));
            match arg.as_str() {
                "--mem" => a.memory = Some(parse_size(&value("--mem")).unwrap_or_else(|| die("bad --mem size"))),
                "--net" => a.network = Some(NetMode::Allow),
                "--no-net" => a.network = Some(NetMode::Deny),
                "--allow-read" => a.read.push(value("--allow-read")),
                "--allow-write" => a.write.push(value("--allow-write")),
                "--name" => a.name = Some(value("--name")),
                "--" => {
                    a.program = it.next().unwrap_or_else(|| die("no program given"));
                    a.argv = it.collect();
                    break;
                }
                other => die(&format!("unknown option {other}")),
            }
        }
        if a.program.is_empty() {
            die("usage: ferro-run [options] -- PROGRAM [ARGS]");
        }
        a
    }

    pub fn run() {
        let args = parse();
        let name = args.name.clone().unwrap_or_else(|| app_name(&args.program));
        let cwd = std::env::current_dir().map_or("/".into(), |p| p.to_string_lossy().into_owned());
        let data_dir = format!("/ProgramData/{name}");
        let _ = std::fs::create_dir_all(format!("{data_dir}/tmp"));

        // Explicit flags win; otherwise what the user chose before; otherwise ask.
        let remembered = Store::load(DEFAULT_PATH).app(&name);
        let mut policy = Policy::standard(&args.program, &cwd, &data_dir);
        policy.network = args.network.unwrap_or(match remembered.network {
            Some(NetChoice::Allow) => NetMode::Allow,
            Some(NetChoice::Deny) => NetMode::Deny,
            None => NetMode::Ask,
        });
        policy.read.extend(args.read);
        policy.read_write.extend(args.write);
        if let Some(m) = args.memory.or(remembered.memory) {
            policy.memory_max = m;
        }

        // Build everything that can fail before forking.
        let ruleset = Ruleset::build(&policy).unwrap_or_else(|e| die(&format!("sandbox unavailable: {e}")));
        let cgroup = match AppCgroup::create(&name, &policy) {
            Ok(cg) => Some(cg),
            Err(e) => {
                eprintln!("ferro-run: no RAM budget for {name} (cgroup: {e})");
                None
            }
        };
        let c_prog = CString::new(args.program.clone()).unwrap_or_else(|_| die("bad program path"));
        let c_args: Vec<CString> =
            std::iter::once(args.program.clone()).chain(args.argv).map(|s| CString::new(s).unwrap_or_else(|_| die("bad argument"))).collect();
        let mut argv: Vec<*const libc::c_char> = c_args.iter().map(|c| c.as_ptr()).collect();
        argv.push(std::ptr::null());
        // Carries the network-question listener from the child to us.
        let mut pair = [0i32; 2];
        // SAFETY: valid out-array.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0, pair.as_mut_ptr()) } != 0 {
            die("socketpair failed");
        }

        // SAFETY: single-threaded here, so the child may run arbitrary code.
        match unsafe { libc::fork() } {
            -1 => die(&format!("fork: {}", std::io::Error::last_os_error())),
            0 => child(&policy, cgroup.as_ref(), &ruleset, &data_dir, pair[1], &c_prog, &argv, &args.program),
            child => {
                CHILD.store(child, Ordering::SeqCst);
                // SAFETY: closing the child's end in the parent.
                unsafe { libc::close(pair[1]) };
                // Ctrl+C reaches the app through the terminal; we just wait.
                // SAFETY: installing handlers with valid function pointers.
                unsafe {
                    libc::signal(libc::SIGINT, libc::SIG_IGN);
                    libc::signal(libc::SIGQUIT, libc::SIG_IGN);
                    libc::signal(libc::SIGTERM, forward as *const () as libc::sighandler_t);
                    libc::signal(libc::SIGHUP, forward as *const () as libc::sighandler_t);
                }
                drop(ruleset);
                let listener = (policy.network == NetMode::Ask).then(|| linux::recv_fd(pair[0]).ok()).flatten();
                let status = supervise(child, listener, &name);
                if let Some(cg) = &cgroup {
                    if cg.oom_kills() > 0 {
                        let budget = policy.memory_max.map_or("its".into(), |b| format!("its {} budget", human(b)));
                        eprintln!("ferro-run: {name} was stopped because it used more than {budget} of memory");
                    }
                }
                drop(cgroup);
                let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) };
                std::process::exit(code);
            }
        }
    }

    fn human(bytes: u64) -> String {
        if bytes >= 1 << 30 && bytes.is_multiple_of(1 << 30) {
            format!("{} GB", bytes >> 30)
        } else {
            format!("{} MB", bytes >> 20)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn child(
        policy: &Policy,
        cgroup: Option<&AppCgroup>,
        ruleset: &Ruleset,
        data_dir: &str,
        sock: i32,
        prog: &CString,
        argv: &[*const libc::c_char],
        program: &str,
    ) -> ! {
        let fail = |what: &str, e: std::io::Error| -> ! {
            eprintln!("ferro-run: {what}: {e}");
            // SAFETY: _exit is always safe in a child.
            unsafe { libc::_exit(126) }
        };
        if let Some(cg) = cgroup {
            if let Err(e) = cg.enter() {
                fail("joining RAM budget", e);
            }
        }
        std::env::set_var("TMPDIR", format!("{data_dir}/tmp"));
        if let Err(e) = linux::drop_capabilities() {
            fail("dropping capabilities", e);
        }
        if let Err(e) = linux::lock_privileges() {
            fail("no_new_privs", e);
        }
        if let Err(e) = ruleset.restrict_self() {
            fail("landlock", e);
        }
        if let Err(e) = linux::install_seccomp() {
            fail("seccomp", e);
        }
        match linux::install_net_gate(policy.network) {
            Ok(Some(listener)) => {
                if let Err(e) = linux::send_fd(sock, listener) {
                    fail("network gate", e);
                }
                // SAFETY: the supervisor holds its own copy now.
                unsafe { libc::close(listener) };
            }
            Ok(None) => {}
            Err(e) => fail("network gate", e),
        }
        // SAFETY: argv is a NULL-terminated array of valid C strings.
        unsafe { libc::execvp(prog.as_ptr(), argv.as_ptr()) };
        eprintln!("ferro-run: cannot start {program}: {}", std::io::Error::last_os_error());
        // SAFETY: as above.
        unsafe { libc::_exit(127) }
    }

    /// Waits for the app while answering its network questions.
    fn supervise(child: i32, listener: Option<i32>, name: &str) -> i32 {
        let mut session: Option<bool> = None;
        let mut status = 0;
        loop {
            // SAFETY: non-blocking wait on our own child.
            if unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) } == child {
                return status;
            }
            let Some(fd) = listener else {
                // SAFETY: blocking wait; nothing else to do.
                while unsafe { libc::waitpid(child, &mut status, 0) } < 0 {}
                return status;
            };
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd; wake regularly to notice the exit.
            if unsafe { libc::poll(&mut pfd, 1, 250) } <= 0 || pfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let Some(req) = linux::recv_net_request(fd) else { continue };
            let allow = *session.get_or_insert_with(|| ask(name));
            linux::answer_net_request(fd, req, allow);
        }
    }

    /// Asks on the app's terminal; remembers "always"/"never" answers.
    fn ask(name: &str) -> bool {
        let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
            eprintln!("ferro-run: {name} wants the network but there is no terminal to ask; denied");
            return false;
        };
        let _ = write!(
            tty,
            "\r\n\x1b[1m[FerroOS] {name} wants to use the network.\x1b[0m\r\n  \
             Y = allow this time   A = always allow   N = deny this time   V = never allow\r\n  Choice: "
        );
        let mut line = String::new();
        let _ = BufReader::new(&tty).read_line(&mut line);
        let answer = line.trim().to_ascii_lowercase();
        let (allow, remember) = match answer.as_str() {
            "a" | "always" => (true, Some(NetChoice::Allow)),
            "y" | "yes" => (true, None),
            "v" | "never" => (false, Some(NetChoice::Deny)),
            _ => (false, None),
        };
        if let Some(choice) = remember {
            let mut store = Store::load(DEFAULT_PATH);
            store.set_network(name, Some(choice));
            if let Err(e) = store.save() {
                let _ = writeln!(tty, "  (couldn't remember this: {e})\r");
            }
        }
        let _ = writeln!(tty, "  {}\r", if allow { "Network allowed." } else { "Network denied." });
        allow
    }
}
