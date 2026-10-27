//! ferro-jail: runs a program that opens files from anywhere (the video
//! player, the thumbnailer) so that a booby-trapped file that takes it over
//! can do little with it:
//!
//! * no internet: sockets for IPv4, IPv6 and raw packets are refused
//!   (seccomp), so nothing can be sent anywhere or fetched;
//! * no other programs: only the program itself and those named with
//!   `--exec` may be started (Landlock), so no shell, no downloader;
//! * none of the kernel's riskier corners (ptrace, io_uring, bpf, mounts,
//!   new namespaces...), and no gaining privileges through setuid programs.
//!
//! The screen, the sound and the files it was given work as before.
//!
//!     ferro-jail [--exec PATH]... [--] PROGRAM [ARGS...]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut exec_allowed: Vec<String> = Vec::new();
    let program = loop {
        match args.next() {
            Some(a) if a == "--exec" => exec_allowed.extend(args.next()),
            Some(a) if a == "--" => break args.next(),
            Some(a) => break Some(a),
            None => break None,
        }
    };
    let Some(program) = program else {
        eprintln!("usage: ferro-jail [--exec PATH]... [--] PROGRAM [ARGS...]");
        exit(2);
    };
    let Some(path) = resolve(&program) else {
        eprintln!("ferro-jail: {program}: not found");
        exit(127);
    };
    let path_text = path.to_string_lossy().into_owned();
    exec_allowed.push(path_text.clone());
    let allowed: Vec<&str> = exec_allowed.iter().map(String::as_str).collect();
    if let Err(e) = lock_down(&allowed) {
        // Never run it unprotected: that's what it was asked not to do.
        eprintln!("ferro-jail: can't protect {program}: {e}");
        exit(126);
    }
    let err = Command::new(&path).args(args).exec();
    eprintln!("ferro-jail: {program}: {err}");
    exit(126);
}

fn lock_down(allowed: &[&str]) -> std::io::Result<()> {
    use ferro_sandbox::linux;
    linux::lock_privileges()?;
    linux::install_seccomp()?;
    linux::install_net_gate(ferro_sandbox::NetMode::Deny)?;
    match linux::restrict_exec(allowed) {
        Ok(()) => Ok(()),
        // A kernel without Landlock: the rest still holds.
        Err(e) if linux::landlock_abi() < 1 => {
            eprintln!("ferro-jail: {e}; other programs can't be stopped from starting");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The program's full path: as given if it has a slash, else from $PATH.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).is_file().then(|| PathBuf::from(program));
    }
    let paths = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())
}
