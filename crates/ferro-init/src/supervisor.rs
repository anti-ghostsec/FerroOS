//! Child process supervision and zombie reaping.
//!
//! PID 1 inherits every orphan, so one thread reaps *all* children. It must
//! not race `std::process::Command`, which reaps a child itself when `exec`
//! fails. So the reaper first *peeks* with `waitid(WNOWAIT)`, then takes the
//! state lock (held by spawners across `spawn()`), and only then reaps.

use crate::system;
use std::collections::{HashMap, HashSet};
use std::io;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const EXIT_POWEROFF: i32 = 100;
const EXIT_REBOOT: i32 = 101;

#[derive(Clone)]
struct Service {
    name: String,
    path: String,
    args: Vec<String>,
    /// Run as the unprivileged desktop user, in the delegated cgroup.
    as_user: bool,
    started: Instant,
    /// Consecutive quick crashes, for restart backoff.
    failures: u32,
}

#[derive(Default)]
struct State {
    services: HashMap<i32, Service>,
    foreground: HashSet<i32>,
    finished: HashMap<i32, i32>,
}

pub struct Supervisor {
    state: Mutex<State>,
    done: Condvar,
}

impl Supervisor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { state: Mutex::default(), done: Condvar::new() })
    }

    pub fn start_reaper(self: &Arc<Self>) {
        let me = Arc::clone(self);
        thread::Builder::new().name("reaper".into()).spawn(move || me.reap_forever()).expect("spawn reaper");
    }

    pub fn spawn_service(&self, name: &str, path: &str, args: &[&str]) {
        let args = args.iter().map(|a| a.to_string()).collect();
        self.start(Service { name: name.into(), path: path.into(), args, as_user: false, started: Instant::now(), failures: 0 });
    }

    /// Starts a service as the desktop user (uid 1000), not root.
    pub fn spawn_user_service(&self, name: &str, path: &str, args: &[&str]) {
        let args = args.iter().map(|a| a.to_string()).collect();
        self.start(Service { name: name.into(), path: path.into(), args, as_user: true, started: Instant::now(), failures: 0 });
    }

    fn start(&self, svc: Service) {
        let mut st = self.state.lock().unwrap();
        let mut cmd = Command::new(&svc.path);
        cmd.args(&svc.args);
        if svc.as_user {
            use std::os::unix::process::CommandExt;
            cmd.env("HOME", "/home/user").env("USER", "user").current_dir("/home/user");
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    // Join the delegated cgroup while still root (moving out of
                    // the root cgroup needs root), then drop to the user for good.
                    let procs = c"/sys/fs/cgroup/ferro/desktop/cgroup.procs";
                    let fd = libc::open(procs.as_ptr(), libc::O_WRONLY);
                    if fd >= 0 {
                        libc::write(fd, b"0".as_ptr().cast(), 1);
                        libc::close(fd);
                    }
                    if libc::setgroups(0, std::ptr::null()) != 0
                        || libc::setgid(crate::system::USER_GID) != 0
                        || libc::setuid(crate::system::USER_UID) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id() as i32;
                println!("init: started {} (pid {pid})", svc.name);
                st.services.insert(pid, Service { started: Instant::now(), ..svc });
            }
            Err(e) => eprintln!("init: cannot start {}: {e}", svc.name),
        }
    }

    /// Runs a program attached to the console and waits for it.
    pub fn run_foreground(&self, path: &str, args: &[&str], cwd: &str) -> io::Result<i32> {
        let pid = {
            let mut st = self.state.lock().unwrap();
            let pid = Command::new(path).args(args).current_dir(cwd).spawn()?.id() as i32;
            st.foreground.insert(pid);
            pid
        };
        let mut st = self.state.lock().unwrap();
        loop {
            if let Some(code) = st.finished.remove(&pid) {
                return Ok(code);
            }
            st = self.done.wait(st).unwrap();
        }
    }

    fn reap_forever(self: Arc<Self>) {
        loop {
            // SAFETY: zeroed siginfo_t is a valid out-parameter.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            if unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOWAIT) } != 0 {
                thread::sleep(Duration::from_millis(200)); // ECHILD: no children yet
                continue;
            }
            let mut st = self.state.lock().unwrap();
            // SAFETY: waitid filled in a SIGCHLD siginfo.
            let pid = unsafe { info.si_pid() };
            let mut status = 0;
            if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } != pid {
                continue; // already reaped by std after a failed exec
            }
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                -1
            };
            if st.foreground.remove(&pid) {
                st.finished.insert(pid, code);
                self.done.notify_all();
            } else if let Some(svc) = st.services.remove(&pid) {
                drop(st);
                self.on_service_exit(svc, code);
            }
            // Anything else was an orphan; reaping it is all PID 1 owes it.
        }
    }

    fn on_service_exit(self: &Arc<Self>, svc: Service, code: i32) {
        if system::shutting_down() {
            return;
        }
        match code {
            EXIT_POWEROFF => system::shutdown(false),
            EXIT_REBOOT => system::shutdown(true),
            _ => {
                let failures = if svc.started.elapsed() < Duration::from_secs(10) { svc.failures + 1 } else { 0 };
                let delay = Duration::from_secs(u64::from(failures.min(10)));
                eprintln!("init: {} exited ({code}); restarting in {}s", svc.name, delay.as_secs());
                let me = Arc::clone(self);
                thread::spawn(move || {
                    thread::sleep(delay);
                    if !system::shutting_down() {
                        me.start(Service { failures, ..svc });
                    }
                });
            }
        }
    }
}
