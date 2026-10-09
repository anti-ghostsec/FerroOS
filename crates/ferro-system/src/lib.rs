//! The wire protocol between the desktop and `ferro-system`, plus what both
//! sides agree on. One request per connection: a command line in, one reply
//! line out (`OK`, `ERR <message>`, or a value).
//!
//! ```text
//! STATUS                 -> NONE | NEW | LOCKED | UNLOCKED | AMNESIC
//! CREATE <password>      format the vault disk with this password
//! UNLOCK <password>      unlock and restore saved files
//! AMNESIC                this session saves nothing
//! SWITCHES <n> <m> <c> [<v> <t>]
//!                        1 = enabled: network, mic, camera, VPN, Tor. The
//!                        first one after logon starts the network session.
//! LIMIT <pid> <bytes|none>   RAM budget for a desktop-user process
//! SAVE                   save now (also done automatically)
//! DISKS                  -> name|bytes|model;...  disks Setup may use (live only)
//! INSTALL <disk>         install FerroOS on it, erasing it (live only)
//! ```

/// Present in a live session started from installation media.
pub const INSTALL_MEDIA: &str = "/run/ferro/install-media";
/// Setup's progress (see the `install` module).
pub const INSTALL_STATUS: &str = "/run/ferro/install-status";

/// Where ferro-system listens.
pub const SOCKET: &str = "/run/ferro/system.sock";
/// The desktop user. FerroOS has one interactive user; apps run as it.
pub const USER_UID: u32 = 1000;
pub const USER_GID: u32 = 1000;
/// What persists (relative to `/`): settings, remembered choices and app
/// data, and the user's home folder. Everything else is rebuilt each boot.
pub const PERSISTED: [&str; 2] = ["ProgramData", "home"];

/// Temp folders and caches are never saved.
pub fn skip_persist(rel: &str) -> bool {
    rel.ends_with("/tmp") || rel.contains("/tmp/") || rel.ends_with("/cache") || rel.ends_with(".tmp")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultState {
    /// No vault disk: everything lives in RAM only.
    None,
    /// A disk with no vault yet: the user can create one.
    New,
    Locked,
    Unlocked,
    /// The user chose not to save anything this session.
    Amnesic,
}

impl VaultState {
    pub fn as_str(self) -> &'static str {
        match self {
            VaultState::None => "NONE",
            VaultState::New => "NEW",
            VaultState::Locked => "LOCKED",
            VaultState::Unlocked => "UNLOCKED",
            VaultState::Amnesic => "AMNESIC",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "NONE" => VaultState::None,
            "NEW" => VaultState::New,
            "LOCKED" => VaultState::Locked,
            "UNLOCKED" => VaultState::Unlocked,
            "AMNESIC" => VaultState::Amnesic,
            _ => return None,
        })
    }
}

/// Passwords travel on one line, so they may not contain line breaks.
pub fn valid_password(p: &str) -> Result<(), &'static str> {
    if p.chars().count() < 8 {
        return Err("Use at least 8 characters.");
    }
    if p.contains(['\n', '\r']) {
        return Err("The password can't contain line breaks.");
    }
    Ok(())
}

/// The command-line entry point.
pub mod cli;
pub mod efiboot;
pub mod gpt;
#[cfg(target_os = "linux")]
mod install;
pub mod programs;
#[cfg(target_os = "linux")]
mod tor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip() {
        for s in [VaultState::None, VaultState::New, VaultState::Locked, VaultState::Unlocked, VaultState::Amnesic] {
            assert_eq!(VaultState::parse(s.as_str()), Some(s));
        }
        assert!(skip_persist("ProgramData/app/tmp") && !skip_persist("ProgramData/ferro/choices.conf"));
        assert!(valid_password("short").is_err() && valid_password("long enough").is_ok());
    }
}
