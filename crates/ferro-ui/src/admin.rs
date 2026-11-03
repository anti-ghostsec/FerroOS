//! Running FerroOS's root helpers (ferro-tor, ferro-vpn,
//! ferro-switch-hardware) from the desktop. Switching a protection on
//! needs no password: sudo has a rule for each of those exact commands.
//! Switching one off (Tor, the VPN, its kill switch; the microphone, camera
//! or network back on) asks for the password, in FerroOS's password box,
//! every time: no program running as the person can do it behind their back.

use std::process::{Command, Stdio};

const SUDO: &str = "/usr/bin/sudo";
const ASKPASS: &str = "/usr/local/bin/ferro-askpass";

/// `sudo` for `tool args`: without a password (`-n`) when `asking` is
/// None, else asking for it, the box naming `asking` (what wants it).
pub fn command(tool: &str, args: &[&str], asking: Option<&str>) -> Command {
    let mut c = Command::new(SUDO);
    match asking {
        None => {
            c.arg("-n");
        }
        // -k: always asked, and no remembered password left behind for
        // whatever runs next.
        Some(what) => {
            c.args(["-k", "-A"]).env("SUDO_ASKPASS", ASKPASS).env("FERRO_ADMIN_FOR", what);
        }
    }
    c.arg("--").arg(tool).args(args).stdin(Stdio::null());
    c
}

/// Runs it and waits: whether it worked (and, asking, the password was given).
pub fn run(tool: &str, args: &[&str], asking: Option<&str>) -> bool {
    command(tool, args, asking).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    #[test]
    fn asks_only_when_told_what_for() {
        let args = |c: &std::process::Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(args(&super::command("/x", &["on"], None)), ["-n", "--", "/x", "on"]);
        let c = super::command("/x", &["off"], Some("Switching Tor off"));
        assert_eq!(args(&c), ["-k", "-A", "--", "/x", "off"]);
        assert!(c.get_envs().any(|(k, v)| k == "FERRO_ADMIN_FOR" && v.is_some_and(|v| v == "Switching Tor off")));
    }
}
