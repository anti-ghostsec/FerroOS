//! ferro-init: FerroOS's PID 1.
//!
//! Mounts the kernel filesystems, supervises ferro-shell (restarting it if it
//! crashes, powering off/rebooting when it asks to) and runs a DOS-flavored
//! rescue console on /dev/console.

#[cfg(target_os = "linux")]
mod console;
#[cfg(target_os = "linux")]
mod modules;
#[cfg(target_os = "linux")]
mod supervisor;
#[cfg(target_os = "linux")]
mod system;

/// The desktop service. Exit codes 100/101 mean power off/reboot.
#[cfg(target_os = "linux")]
const SHELL: &str = "/bin/ferro-shell";
/// Network: interfaces, DHCP, and the DNS-over-HTTPS resolver.
#[cfg(target_os = "linux")]
const NET: &str = "/bin/ferro-net";
/// The root broker: encrypted vault, kill switches, RAM budgets.
#[cfg(target_os = "linux")]
const SYSTEM: &str = "/bin/ferro-system";

#[cfg(target_os = "linux")]
pub fn main() {
    let pid1 = std::process::id() == 1;
    if pid1 {
        std::panic::set_hook(Box::new(|info| eprintln!("init: PANIC: {info}")));
        std::env::set_var("PATH", "/bin:/sbin");
        system::mount_early();
        system::harden();
        system::load_modules();
        system::prepare_user_session();
        system::set_hostname("ferro");
    } else {
        eprintln!("init: not PID 1, skipping mounts (test mode)");
    }
    println!("\nFerroOS {} starting", env!("CARGO_PKG_VERSION"));

    let mut drives = ferro_path::DriveTable::default();
    if std::path::Path::new("/mnt/d").is_dir() {
        drives.mount('D', "/mnt/d");
    }

    let sup = supervisor::Supervisor::new();
    sup.start_reaper();
    if pid1 && std::path::Path::new(SYSTEM).exists() {
        sup.spawn_service("ferro-system", SYSTEM, &[]);
    }
    if pid1 && std::path::Path::new(NET).exists() {
        sup.spawn_service("ferro-net", NET, &["daemon"]);
    }
    if pid1 && std::path::Path::new(SHELL).exists() {
        // The desktop is not root: everything it starts isn't either.
        sup.spawn_user_service("ferro-shell", SHELL, &[]);
    }
    console::run(&sup, &drives)
}

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("ferro-init is FerroOS's PID 1 and only runs on Linux. Build it with `cargo xtask build`.");
    std::process::exit(1);
}
