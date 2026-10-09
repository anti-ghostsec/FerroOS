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
/// Live VPN/Tor state for the desktop (`key=value` lines).
pub const NET_STATUS: &str = "/run/ferro/net-status";
/// The Tor service's user. Only it gets a route to the internet in Tor mode.
pub const TOR_UID: u32 = 900;
pub const TOR_SOCKS: &str = "127.0.0.1:9150";
pub const TOR_DNS: &str = "127.0.0.1:9053";

/// The command-line entry point.
pub mod cli;
