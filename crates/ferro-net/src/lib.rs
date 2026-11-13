//! FerroOS networking.
//!
//! * [`dns`]: DNS wire format, query padding and an in-RAM cache.
//! * `tls`: rustls with Encrypted Client Hello, DNS-over-HTTPS, minimal HTTP.
//! * `netif`: interface up/down, random MAC per boot, a quiet DHCP client.
//! * [`wg`]: WireGuard profiles; `nl`: netlink; `route`: VPN and Tor routing.

pub mod clock;
pub mod dns;
pub mod h2;

pub mod wg;

#[cfg(target_os = "linux")]
pub mod netif;
#[cfg(target_os = "linux")]
pub mod nl;
#[cfg(target_os = "linux")]
pub mod route;
#[cfg(target_os = "linux")]
pub mod tls;

/// Present while the tray's network kill switch is off; ferro-netd takes
/// every interface down and refuses DNS until it disappears.
pub const KILL_FILE: &str = "/run/ferro/network-off";
/// Present once someone has logged on (or chosen not to save). Until then
/// nothing but DHCP leaves the machine: the VPN and Tor settings are inside
/// the encrypted vault, and traffic must not go out before they're known.
pub const SESSION_FILE: &str = "/run/ferro/session";
/// Present while the VPN is switched on.
pub const VPN_FILE: &str = "/run/ferro/vpn-on";
/// Present while Tor is switched on.
pub const TOR_FILE: &str = "/run/ferro/tor-on";
/// The VPN profile: a provider's WireGuard file, kept in the vault.
pub const VPN_CONFIG: &str = "/ProgramData/ferro/vpn.conf";

/// A settings file's text, if nobody else could have written it: a plain
/// file (not a link) nobody else may change, and, in a folder every account
/// may write in, root's or your own. Where the VPN goes is in it.
#[cfg(target_os = "linux")]
pub fn read_trusted(path: &str) -> std::io::Result<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let path = std::path::Path::new(path);
    let file = std::fs::symlink_metadata(path)?;
    let dir = std::fs::metadata(path.parent().unwrap_or(std::path::Path::new("/")))?;
    // SAFETY: geteuid has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    let open_to_all = |m: &std::fs::Metadata| m.permissions().mode() & 0o002 != 0;
    if !file.file_type().is_file() || open_to_all(&file) || !(file.uid() == 0 || file.uid() == me || !open_to_all(&dir)) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not trusted: someone else could have written it"));
    }
    std::fs::read_to_string(path)
}
/// Live VPN/Tor state for the desktop (`key=value` lines).
pub const NET_STATUS: &str = "/run/ferro/net-status";
/// The Tor service's user. Only it gets a route to the internet in Tor mode.
pub const TOR_UID: u32 = 900;
pub const TOR_SOCKS: &str = "127.0.0.1:9150";
pub const TOR_DNS: &str = "127.0.0.1:9053";

/// The command-line entry point.
pub mod cli;
