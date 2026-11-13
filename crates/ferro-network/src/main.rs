//! ferro-network: Network & Internet Properties dialog for FerroOS Heavy.
//!
//! Provides dedicated Windows 95 tabbed GUI controls for Wired Ethernet (LAN),
//! Wi-Fi network scanning & connection, WireGuard VPN, Arti Tor, and Hardware Kill Switches.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use view::{Hit, State, WifiAp, WifiDialog, H, W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

fn switches_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("switches.conf")
    } else {
        PathBuf::from("/etc/ferro/switches.conf")
    }
}

fn load_state() -> State {
    // Load live ethernet info
    let mut st = State { ethernet: load_ethernet_info(), ..State::default() };

    // Load hardware switches from Store first
    let store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    st.net_switch = store.switches.network;
    st.mic_switch = store.switches.microphone;
    st.cam_switch = store.switches.camera;
    st.vpn_enabled = store.switches.vpn;
    st.tor_enabled = store.switches.tor;
    if store.bridges.iter().any(|b| !b.contains("2B280B23DD74683028100AB73E41B3BB52F4F354")) {
        st.tor_bridges = true;
    }

    // Load switches from user configuration file (switches.conf)
    let sp = switches_path();
    if let Ok(content) = fs::read_to_string(&sp) {
        for line in content.lines() {
            let line = line.trim();
            if let Some((k, v)) = line.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                let b = v == "true" || v == "1" || v == "on";
                match k {
                    "network" => st.net_switch = b,
                    "microphone" => st.mic_switch = b,
                    "camera" => st.cam_switch = b,
                    "vpn" => st.vpn_enabled = b,
                    "tor" => st.tor_enabled = b,
                    "tor_bridges" => st.tor_bridges = b,
                    _ => {}
                }
            }
        }
    }

    // Load live net status if present
    if let Ok(net) = fs::read_to_string("/run/ferro/net-status") {
        for line in net.lines() {
            if let Some(v) = line.strip_prefix("vpn=") {
                st.vpn_enabled = v == "connected" || v == "on";
                st.vpn_status = format!("VPN is {v}");
            }
        }
    }

    // Load live vpn status if present
    let vpn = ferro_ui::admin::vpn_status();
    if !vpn.is_empty() {
        st.vpn_status = vpn_status_text(&vpn);
        if let Some(v) = vpn.lines().find_map(|l| l.strip_prefix("state=")) {
            st.vpn_enabled = vpn_wanted(v);
        }
    } else {
        let _ = Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-vpn", "status"]).status();
        if let Ok(vpn) = fs::read_to_string("/run/ferro/vpn-status") {
            st.vpn_status = vpn_status_text(&vpn);
            if let Some(v) = vpn.lines().find_map(|l| l.strip_prefix("state=")) {
                st.vpn_enabled = vpn_wanted(v);
            }
        } else {
            st.vpn_enabled = false;
            st.vpn_status = "WireGuard is disconnected.".into();
        }
    }

    // What ferro-tor reports: the switch's real state, and the address
    // websites see once Tor is connected.
    if let Ok(tor) = fs::read_to_string("/run/ferro/tor-status") {
        st.tor_status = tor_status_text(&tor);
        if let Some(v) = tor.lines().find_map(|l| l.strip_prefix("state=")) {
            st.tor_enabled = matches!(v, "ready" | "starting" | "error");
        }
    } else {
        // Nothing has switched Tor on since startup.
        st.tor_enabled = false;
        st.tor_status = "Tor is off.".to_string();
    }

    // What the card last saw: at once (a fresh look follows, once the window is up).
    scan_wifi_into(&mut st, false);
    st.portal = portal_link();

    st
}

/// Whether the VPN is switched on: connected, or on its way, or dropped
/// with the kill switch blocking the internet (switching it off ends that).
fn vpn_wanted(state: &str) -> bool {
    matches!(state, "connected" | "on" | "active" | "connecting" | "no_answer" | "blocked")
}

fn vpn_status_text(status: &str) -> String {
    let field = |k: &str| status.lines().find_map(|l| l.strip_prefix(k)).map(str::trim).unwrap_or("");
    match (field("state="), field("detail=")) {
        ("", _) | ("disconnected", _) => "WireGuard is disconnected.".to_string(),
        ("connected", "") => "WireGuard tunnel is active.".to_string(),
        (_, detail) if !detail.is_empty() => detail.to_string(),
        (state, "") => format!("VPN is {state}."),
        (_, _) => "WireGuard is disconnected.".to_string(),
    }
}

fn tor_status_text(status: &str) -> String {
    let field = |k: &str| status.lines().find_map(|l| l.strip_prefix(k)).map(str::trim).unwrap_or("");
    match (field("state="), field("detail=")) {
        ("", _) | ("off", _) => "Tor is off.".to_string(),
        (state, "") => format!("Tor is {state}."),
        (_, detail) => format!("Tor: {detail}"),
    }
}

fn load_ethernet_info() -> view::EthernetInfo {
    #[cfg(target_os = "linux")]
    {
        let mut iface = String::new();
        let mut gateway = String::new();

        if let Ok(out) = Command::new("ip").args(["route", "show", "default"]).output() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let parts: Vec<&str> = s.split_whitespace().collect();
                if let Some(via_idx) = parts.iter().position(|&p| p == "via") {
                    if let Some(gw) = parts.get(via_idx + 1) {
                        gateway = gw.to_string();
                    }
                }
                if let Some(dev_idx) = parts.iter().position(|&p| p == "dev") {
                    if let Some(dev) = parts.get(dev_idx + 1) {
                        iface = dev.to_string();
                    }
                }
            }
        }

        if iface.is_empty() {
            if let Ok(entries) = fs::read_dir("/sys/class/net") {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name != "lo" && !name.starts_with("wl") && !name.starts_with("wg") {
                        iface = name;
                        break;
                    }
                }
            }
        }

        if iface.is_empty() {
            iface = "eth0".to_string();
        }

        let mac = fs::read_to_string(format!("/sys/class/net/{iface}/address"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "00:00:00:00:00:00".to_string());

        let operstate =
            fs::read_to_string(format!("/sys/class/net/{iface}/operstate")).map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".to_string());
        let connected = operstate == "up";

        let mut ip_addr = String::new();
        let mut subnet = "255.255.255.0".to_string();
        if let Ok(out) = Command::new("ip").args(["-4", "addr", "show", &iface]).output() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                for line in s.lines() {
                    let line = line.trim();
                    if line.starts_with("inet ") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if let Some(cidr) = parts.get(1) {
                            if let Some((ip, mask_bits)) = cidr.split_once('/') {
                                ip_addr = ip.to_string();
                                if let Ok(bits) = mask_bits.parse::<u32>() {
                                    let mask_num = if bits == 0 { 0 } else { !0u32 << (32 - bits.min(32)) };
                                    subnet = format!(
                                        "{}.{}.{}.{}",
                                        (mask_num >> 24) & 0xff,
                                        (mask_num >> 16) & 0xff,
                                        (mask_num >> 8) & 0xff,
                                        mask_num & 0xff
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let speed = fs::read_to_string(format!("/sys/class/net/{iface}/speed"))
            .map(|s| format!("{} Mbps (Full Duplex)", s.trim()))
            .unwrap_or_else(|_| if connected { "1000 Mbps".to_string() } else { "Disconnected".to_string() });

        let dns = if let Ok(s) = fs::read_to_string("/etc/resolv.conf") {
            s.lines().filter_map(|l| l.strip_prefix("nameserver ")).next().unwrap_or("9.9.9.9 (Quad9)").trim().to_string()
        } else {
            "9.9.9.9 (Quad9)".to_string()
        };

        let status = if connected && !ip_addr.is_empty() {
            "Connected (DHCP)".to_string()
        } else if connected {
            "Carrier detected (Acquiring IP...)".to_string()
        } else {
            "Cable unplugged or link down".to_string()
        };

        view::EthernetInfo {
            interface: iface,
            status,
            connected,
            ip_address: if ip_addr.is_empty() { "Not assigned".to_string() } else { ip_addr },
            subnet_mask: subnet,
            gateway: if gateway.is_empty() { "Not assigned".to_string() } else { gateway },
            dns,
            mac_address: mac,
            speed,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        view::EthernetInfo::default()
    }
}

fn renew_ethernet(iface: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("nmcli").args(["dev", "connect", iface]).output();
        let _ = Command::new("nmcli").args(["dev", "reapply", iface]).output();
        let _ = Command::new("dhclient").args(["-v", iface]).output();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
    }
}

fn disconnect_ethernet(iface: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("nmcli").args(["dev", "disconnect", iface]).output();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
    }
}

/// What Diagnose found, step by step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Findings {
    /// The router answered at the network's own level (its entry in the
    /// neighbour table is filled in); `None`: no router to ask.
    gateway: Option<bool>,
    /// A name came back from the lookup programs use.
    names: bool,
    /// A connection to a server on the internet was taken, by address.
    internet: bool,
}

/// The line Diagnose shows. Pings aren't used: a normal user can't send them
/// here, so they always failed, even with the network working.
fn diagnosis(iface: &str, f: Findings) -> String {
    match f {
        Findings { gateway: None, internet: false, .. } => "Problem: no router, as this machine has no network address. Check the cable.".into(),
        Findings { gateway: Some(false), internet: false, .. } => "Problem: the router doesn't answer. Check the router and the cable.".into(),
        Findings { internet: false, .. } => "Problem: the router answers, but the internet doesn't. Check the router's own connection.".into(),
        Findings { names: false, .. } => "Problem: the internet works, but names can't be looked up (DNS).".into(),
        _ => format!("Diagnostic OK: the router, the internet and name lookups all work ({iface})."),
    }
}

fn diagnose_ethernet(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        // With Tor on, every connection is taken by Tor's own port on this
        // machine, so a test connection would always seem to work.
        if st.tor_enabled {
            st.eth_diagnostic = Some(format!("Tor is on, so everything goes through it: {}", st.tor_status));
            return;
        }
        let gateway = st.ethernet.gateway.parse::<std::net::IpAddr>().ok().map(gateway_answers);
        let (names, internet) = internet_answers();
        st.eth_diagnostic = Some(diagnosis(&st.ethernet.interface, Findings { gateway, names, internet }));
    }
    #[cfg(not(target_os = "linux"))]
    {
        st.eth_diagnostic = Some("Diagnostic OK: Gateway & Internet reachable (Simulated)".into());
    }
}

/// Whether the router answers: a datagram to it makes this machine ask for
/// it on the network, and the neighbour table then says whether it replied.
#[cfg(target_os = "linux")]
fn gateway_answers(gw: std::net::IpAddr) -> bool {
    let any: std::net::SocketAddr = if gw.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() };
    if let Ok(sock) = std::net::UdpSocket::bind(any) {
        let _ = sock.send_to(b"", (gw, 9));
    }
    for _ in 0..10 {
        let shown = Command::new("ip").args(["neigh", "show", &gw.to_string()]).output();
        match shown.map(|o| neighbour_state(&String::from_utf8_lossy(&o.stdout))) {
            Ok(Some(true)) => return true,
            Ok(Some(false)) => return false,
            _ => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
    false
}

/// From `ip neigh show <address>`: `Some(true)` it answered, `Some(false)` it
/// didn't, `None` still being asked (or not listed yet).
fn neighbour_state(shown: &str) -> Option<bool> {
    let words: Vec<&str> = shown.split_whitespace().collect();
    if words.iter().any(|w| matches!(*w, "REACHABLE" | "STALE" | "DELAY" | "PROBE" | "PERMANENT")) {
        Some(true)
    } else if words.iter().any(|w| matches!(*w, "FAILED")) {
        Some(false)
    } else {
        None
    }
}

/// Whether names are looked up, and whether a server on the internet takes
/// a connection: Quad9 by name, else by its address (so a broken lookup and
/// no internet are told apart). Each step gives up after a few seconds.
#[cfg(target_os = "linux")]
fn internet_answers() -> (bool, bool) {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::time::Duration;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send("dns.quad9.net:443".to_socket_addrs().map(|a| a.collect::<Vec<_>>()).unwrap_or_default());
    });
    let found = rx.recv_timeout(Duration::from_secs(4)).unwrap_or_default();
    let fixed: SocketAddr = ([9, 9, 9, 9], 443).into();
    let reaches = |a: &SocketAddr| TcpStream::connect_timeout(a, Duration::from_secs(3)).is_ok();
    let internet = found.iter().filter(|a| a.is_ipv4()).take(1).any(reaches) || reaches(&fixed);
    (!found.is_empty(), internet)
}

fn connect_wifi(ssid: &str, password: &str, st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        let out = Command::new("nmcli").args(["dev", "wifi", "connect", ssid, "password", password]).output();
        match out {
            Ok(o) if o.status.success() => {
                st.wifi_status = format!("Connected to {ssid} successfully.");
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                st.wifi_status = format!("Connection failed: {}", err.trim());
            }
            Err(e) => {
                st.wifi_status = format!("Error running nmcli: {e}");
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = password;
        st.wifi_status = format!("Connected to {ssid} (Simulated).");
    }
}

fn disconnect_wifi(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("nmcli").args(["radio", "wifi", "off"]).output();
        let _ = Command::new("nmcli").args(["radio", "wifi", "on"]).output();
    }
    st.wifi_status = "Disconnected from wireless network.".into();
    st.selected_ap = None;
}

/// A saved Wi-Fi network: what it takes to set it up again.
#[derive(Debug, PartialEq, Eq)]
struct SavedWifi {
    ssid: String,
    hidden: bool,
    key_mgmt: String,
    psk: String,
}

/// A saved network from `nmcli -s -t connection show uuid U`'s
/// `setting.property:value` lines. The value is all after the first `:`, as
/// saved: unlike in its tables, nmcli puts no backslashes in.
fn parse_saved_wifi(text: &str) -> Option<SavedWifi> {
    let mut w = SavedWifi { ssid: String::new(), hidden: false, key_mgmt: String::new(), psk: String::new() };
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.to_string();
        match k {
            "802-11-wireless.ssid" => w.ssid = v,
            "802-11-wireless.hidden" => w.hidden = v == "yes",
            "802-11-wireless-security.key-mgmt" => w.key_mgmt = v,
            "802-11-wireless-security.psk" => w.psk = v,
            _ => {}
        }
    }
    (!w.ssid.is_empty()).then_some(w)
}

/// The saved Wi-Fi networks in `nmcli -t -f UUID,TYPE,TIMESTAMP,ACTIVE
/// connection show`, the one to reset first: the one in use, or else the
/// one used last.
fn wifi_profiles(text: &str) -> Vec<String> {
    let mut found: Vec<(bool, u64, String)> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() == 4 && f[1] == "802-11-wireless").then(|| (f[3] == "yes", f[2].parse().unwrap_or(0), f[0].to_string()))
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse((a.0, a.1)));
    found.into_iter().map(|(_, _, uuid)| uuid).collect()
}

/// nmcli's words to join the network again, or why it's left alone: only
/// open networks and ones with a password can be set up from what's saved.
fn reconnect_args(w: &SavedWifi) -> Result<Vec<String>, String> {
    let mut a: Vec<String> = ["dev", "wifi", "connect", &w.ssid].map(String::from).to_vec();
    match w.key_mgmt.as_str() {
        "" => {}
        "wpa-psk" | "sae" if !w.psk.is_empty() => a.extend(["password".into(), w.psk.clone()]),
        "wpa-psk" | "sae" => return Err(format!("Couldn't read {}'s password, so it's left as it was.", w.ssid)),
        _ => return Err(format!("{} signs in a way this can't set up again, so it's left as it was.", w.ssid)),
    }
    if w.hidden {
        a.extend(["hidden".into(), "yes".into()]);
    }
    Ok(a)
}

/// nmcli's words to save the network again without joining it, so a reset
/// that couldn't connect (out of reach, say) doesn't lose the password.
fn resave_args(w: &SavedWifi) -> Vec<String> {
    let mut a: Vec<String> = ["connection", "add", "type", "wifi", "con-name", &w.ssid, "ssid", &w.ssid].map(String::from).to_vec();
    if !w.key_mgmt.is_empty() {
        a.extend(["wifi-sec.key-mgmt".into(), w.key_mgmt.clone(), "wifi-sec.psk".into(), w.psk.clone()]);
    }
    if w.hidden {
        a.extend(["802-11-wireless.hidden".into(), "yes".into()]);
    }
    a
}

#[cfg(target_os = "linux")]
fn nmcli<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Result<String, String> {
    match Command::new("nmcli").args(args).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(format!("Error running nmcli: {e}")),
    }
}

/// Forgets the Wi-Fi network in use (or the one used last), every saved copy
/// of it, and joins it again from scratch with the saved password: what
/// fixes a saved network that's gone bad (connected, but nothing loads).
fn reset_wifi(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        st.wifi_status = reset_saved_wifi();
    }
    #[cfg(not(target_os = "linux"))]
    {
        st.wifi_status = "Wi-Fi reset and connected again (Simulated).".into();
    }
    st.selected_ap = None;
}

#[cfg(target_os = "linux")]
fn reset_saved_wifi() -> String {
    let Ok(list) = nmcli(&["-t", "-f", "UUID,TYPE,TIMESTAMP,ACTIVE", "connection", "show"]) else {
        return "Couldn't read the saved networks.".into();
    };
    let saved: Vec<(String, Option<SavedWifi>)> = wifi_profiles(&list)
        .into_iter()
        .map(|uuid| {
            let w = nmcli(&["-s", "-t", "connection", "show", "uuid", &uuid]).ok().as_deref().and_then(parse_saved_wifi);
            (uuid, w)
        })
        .collect();
    let Some((_, Some(w))) = saved.first() else { return "There's no saved Wi-Fi network to reset.".into() };
    let args = match reconnect_args(w) {
        Ok(a) => a,
        Err(why) => return why,
    };
    for (uuid, other) in &saved {
        if other.as_ref().is_some_and(|o| o.ssid == w.ssid) {
            if let Err(e) = nmcli(&["connection", "delete", "uuid", uuid]) {
                return format!("Couldn't forget {}: {e}", w.ssid);
            }
        }
    }
    match nmcli(&args) {
        Ok(_) => format!("Reset {}: connected again.", w.ssid),
        Err(e) => {
            let _ = nmcli(&resave_args(w));
            format!("Reset {}, but couldn't connect: {e}", w.ssid)
        }
    }
}

/// The wireless networks in reach and what to say of them. `rescan` asks the
/// card to look again (a few seconds); without, what it last saw (at once).
fn wifi_scan(rescan: bool) -> (Vec<WifiAp>, String) {
    #[cfg(target_os = "linux")]
    {
        // Try nmcli first, which is standard on NetworkManager systems:
        let again = if rescan { "yes" } else { "no" };
        if let Ok(out) = Command::new("nmcli").args(["-t", "-f", "SSID,SIGNAL,SECURITY", "dev", "wifi", "list", "--rescan", again]).output() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                let mut list = Vec::new();
                for line in text.lines() {
                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() >= 3 && !parts[0].trim().is_empty() {
                        let sig: i32 = parts[1].trim().parse().unwrap_or(50);
                        let dbm = -100 + (sig / 2);
                        let sec = if parts[2].trim().is_empty() { "Open".to_string() } else { parts[2].trim().to_string() };
                        list.push(WifiAp { ssid: parts[0].trim().to_string(), signal: dbm, security: sec });
                    }
                }
                if !list.is_empty() {
                    let said = format!("Found {} wireless network(s).", list.len());
                    return (list, said);
                }
            }
        }

        // Fallback to iw scan
        if rescan {
            if let Ok(out) = Command::new("iw").args(["dev", "wlan0", "scan"]).output() {
                if let Ok(text) = String::from_utf8(out.stdout) {
                    let parsed = ferro_system::wifi::parse_scan(&text);
                    if !parsed.is_empty() {
                        let list: Vec<WifiAp> =
                            parsed.into_iter().map(|(ssid, signal)| WifiAp { ssid, signal, security: "WPA2/WPA3".into() }).collect();
                        let said = format!("Found {} wireless network(s).", list.len());
                        return (list, said);
                    }
                }
            }
        }
    }
    let said = if rescan { "No wireless networks found." } else { "Looking for wireless networks..." };
    (Vec::new(), said.into())
}

fn scan_wifi_into(st: &mut State, rescan: bool) {
    let (aps, said) = wifi_scan(rescan);
    st.wifi_aps = aps;
    st.wifi_status = said;
}

/// Where a scan in the background leaves the networks it found.
type Found = std::sync::Arc<std::sync::Mutex<Option<(Vec<WifiAp>, String)>>>;

/// Looks for wireless networks again while the window is up, and wakes it
/// (a message to `win`) with what was found in `found`.
fn rescan_in_background(win: Window, found: Found) {
    std::thread::spawn(move || {
        let result = wifi_scan(true);
        if let Ok(mut f) = found.lock() {
            *f = Some(result);
        }
        if let Ok((conn, _)) = RustConnection::connect(None) {
            let wake = ClientMessageEvent::new(32, win, AtomEnum::STRING, [WIFI_FOUND, 0, 0, 0, 0]);
            let _ = conn.send_event(false, win, EventMask::NO_EVENT, wake);
            let _ = conn.flush();
        }
    });
}

const VPN: &str = "/usr/local/sbin/ferro-vpn";
const TOR: &str = "/usr/local/sbin/ferro-tor";
const SWITCHES: &str = "/usr/local/sbin/ferro-switch-hardware";

/// The message `rescan_in_background` wakes the window with.
const WIFI_FOUND: u32 = 0x5749_4649;

/// Switches WireGuard VPN on or off through `ferro-vpn` (heavy/ferro-vpn),
/// which runs as root: it configures the wg0 interface and routes traffic
/// through the tunnel with kill-switch protection.
fn apply_vpn(enabled: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        let ok = if enabled { ferro_ui::admin::run(VPN, &["on"], None) } else { ferro_ui::admin::run(VPN, &["off"], Some("Switching the VPN off")) };
        if !ok {
            eprintln!("ferro-network: couldn't run ferro-vpn");
            return false;
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = enabled;
        true
    }
}

/// Where Import's last resort opens a config to paste into: your own.
fn pasted_vpn_config() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config/ferro/vpn.conf")
}

/// Hands a WireGuard config to `ferro-vpn import`, which keeps it as the VPN's
/// (and, the VPN being on, starts using it). It goes on standard input, read
/// here with our own rights: ferro-vpn runs as root and takes no file names,
/// so it can't be pointed at a file we couldn't read ourselves. Not taken:
/// why, to show.
fn import_vpn(config: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    // A new config can send everything through someone else's server: asked.
    let Ok(mut child) = ferro_ui::admin::command(VPN, &["import"], Some("Using a new VPN config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Err("Not imported: sudo couldn't be run.".into());
    };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(config).is_ok());
    match child.wait_with_output() {
        Ok(o) if o.status.success() && wrote => Ok(()),
        Ok(o) => Err(import_refusal(&String::from_utf8_lossy(&o.stderr))),
        Err(_) => Err(import_refusal("")),
    }
}

/// Why `ferro-vpn import` didn't take a config: its own words, or (sudo
/// said no first) that the password wasn't given.
fn import_refusal(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .filter_map(|l| l.strip_prefix("ferro-vpn: "))
        .find(|l| !l.starts_with("left out"))
        .map_or_else(|| "Not imported: the password wasn't given.".into(), |why| format!("Not imported: {why}."))
}

/// Whether a config pasted into Import's editor is one: an [Interface] with
/// its PrivateKey filled in, not the empty form it starts as.
fn filled_in(config: &[u8]) -> bool {
    let text = String::from_utf8_lossy(config);
    text.contains("[Interface]")
        && text.lines().filter_map(|l| l.split_once('=')).any(|(k, v)| k.trim().eq_ignore_ascii_case("privatekey") && !v.trim().is_empty())
}

/// What to say once `name` is imported: on, the VPN uses it at once.
fn imported_text(name: &str) -> String {
    let now = ferro_ui::admin::vpn_status();
    if now.lines().any(|l| l.trim() == "state=ready") || now.is_empty() {
        format!("Imported {name}. Click Apply to connect.")
    } else {
        format!("Imported {name}. {}", vpn_status_text(&now))
    }
}

/// Switches Tor on or off through `ferro-tor` (heavy/ferro-tor), which runs as
/// root: it starts Tor and puts in the firewall rules that send every
/// program's traffic and DNS through it. Starting Tor alone changes nothing
/// for programs that aren't set up to use its proxy.
fn apply_tor(enabled: bool, bridges: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        let ok = match (enabled, bridges) {
            (true, true) => ferro_ui::admin::run(TOR, &["on", "bridges"], None),
            (true, false) => ferro_ui::admin::run(TOR, &["on"], None),
            (false, _) => ferro_ui::admin::run(TOR, &["off"], Some("Switching Tor off")),
        };
        if !ok && !fs::read_to_string("/run/ferro/tor-status").is_ok_and(|t| t.contains("state=error")) {
            // ferro-tor didn't run at all (no permission): say so rather
            // than look switched.
            eprintln!("ferro-network: couldn't run ferro-tor");
            return false;
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (enabled, bridges);
        true
    }
}

/// The root helper that relaxes lookups for a sign-in page (heavy/ferro-portal).
const PORTAL: &str = "/usr/local/sbin/ferro-portal";
/// What it leaves while a sign-in page is allowed: `link=` and `started=`.
const PORTAL_STATE: &str = "/run/ferro/portal";
/// A plain-HTTP page that a network with a sign-in page sends to its own.
const PORTAL_PROBE: &str = "http://detectportal.firefox.com/canonical.html";
/// The browsers the sign-in page may open in, the first one found.
const BROWSERS: [&str; 3] = ["wisp", "firefox-esr", "firefox"];

/// The `key` line's value in `ferro-portal`'s output or state.
fn portal_field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The link whose sign-in page is allowed now, if any.
fn portal_link() -> Option<String> {
    fs::read_to_string(PORTAL_STATE).ok().and_then(|t| portal_field(&t, "link="))
}

/// What to say when `ferro-portal on` refused: its own words, or that sudo
/// wouldn't run it.
fn portal_refusal(stderr: &str) -> String {
    match stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
        Some(l) if !l.starts_with("sudo:") => l.to_string(),
        _ => "Couldn't allow it: no permission (sudo).".to_string(),
    }
}

/// The first of `BROWSERS` on the PATH.
fn find_browser() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    BROWSERS.iter().flat_map(|b| std::env::split_paths(&path).map(move |d| d.join(b))).find(|p| p.is_file())
}

/// Ticked: plain lookups to this network's own server, through
/// `ferro-portal`, and its sign-in page in a throwaway browser. Never with
/// Tor on (`ferro-portal` refuses too): Tor can't reach a sign-in page.
fn allow_sign_in(st: &mut State) {
    if st.tor_enabled {
        st.wifi_status = "Tor is on: switch Tor off first to sign in.".into();
        return;
    }
    let Some(browser) = find_browser() else {
        st.wifi_status = "No web browser to open the sign-in page in.".into();
        return;
    };
    // Plain lookups take a protection away, so they ask for the password:
    // otherwise any program could keep them in plain text, again and again.
    let out = ferro_ui::admin::command(PORTAL, &["on"], Some("A Wi-Fi sign-in page")).output();
    let o = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            st.wifi_status = portal_refusal(&String::from_utf8_lossy(&o.stderr));
            return;
        }
        Err(_) => {
            st.wifi_status = portal_refusal("");
            return;
        }
    };
    let link = portal_field(&String::from_utf8_lossy(&o.stdout), "link=").unwrap_or_default();
    // A process of its own, so the page stays open (and lookups go back to
    // encrypted when it closes) after this window is closed.
    use std::os::unix::process::CommandExt;
    let opened = std::env::current_exe().is_ok_and(|me| {
        Command::new(me)
            .arg("--sign-in-page")
            .arg(&browser)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .is_ok()
    });
    if opened {
        st.wifi_status = format!("Sign-in page open on {link}; off in 10 minutes.");
        st.portal = Some(link);
    } else {
        stop_sign_in(st);
        st.wifi_status = "Couldn't open the sign-in page.".into();
    }
}

/// Unticked: encrypted lookups only again.
fn stop_sign_in(st: &mut State) {
    let ok = Command::new("sudo").args(["-n", PORTAL, "off"]).stdin(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    st.portal = portal_link();
    st.wifi_status =
        if ok && st.portal.is_none() { "Lookups are encrypted again.".into() } else { "Couldn't switch it off: no permission (sudo).".into() };
}

/// The settings a throwaway profile starts with: plain HTTP allowed (the
/// sign-in page is reached through it), the system's lookups rather than the
/// browser's own, private browsing, and no first-run pages.
const SIGN_IN_PREFS: &str = "\
user_pref(\"dom.security.https_only_mode\", false);
user_pref(\"dom.security.https_only_mode_pbm\", false);
user_pref(\"network.trr.mode\", 5);
user_pref(\"network.proxy.type\", 0);
user_pref(\"browser.privatebrowsing.autostart\", true);
user_pref(\"browser.shell.checkDefaultBrowser\", false);
user_pref(\"browser.startup.homepage_override.mstone\", \"ignore\");
user_pref(\"browser.aboutwelcome.enabled\", false);
user_pref(\"datareporting.policy.firstRunURL\", \"\");
";

/// `ferro-network --sign-in-page <browser>`: the sign-in page in `browser`
/// with a profile of its own, in memory, deleted when its window closes.
/// Then lookups go back to encrypted, unless the sign-in was allowed again
/// since (another window's to end).
fn sign_in_page(browser: &str) {
    let started = fs::read_to_string(PORTAL_STATE).ok().and_then(|t| portal_field(&t, "started="));
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let profile = base.join(format!("ferro-sign-in-{}", std::process::id()));
    let _ = fs::remove_dir_all(&profile);
    use std::os::unix::fs::DirBuilderExt;
    if fs::DirBuilder::new().mode(0o700).create(&profile).is_ok() && fs::write(profile.join("user.js"), SIGN_IN_PREFS).is_ok() {
        let _ = Command::new(browser).arg("--no-remote").arg("--profile").arg(&profile).arg(PORTAL_PROBE).stdin(std::process::Stdio::null()).status();
    }
    let _ = fs::remove_dir_all(&profile);
    let now = fs::read_to_string(PORTAL_STATE).ok().and_then(|t| portal_field(&t, "started="));
    if started.is_some() && now == started {
        let _ = Command::new("sudo").args(["-n", PORTAL, "off"]).stdin(std::process::Stdio::null()).status();
    }
}

fn save_state(st: &mut State, conn: Option<&RustConnection>, root: Option<Window>) {
    let mut store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    // The hardware switches, first: off at once (again, in case), back on
    // only once the password's given; refused, it stays off.
    let was = store.switches;
    for (now, before, what, asking) in [
        (&mut st.mic_switch, was.microphone, "mic", "Switching the microphone on"),
        (&mut st.cam_switch, was.camera, "cam", "Switching the camera on"),
        (&mut st.net_switch, was.network, "net", "Switching the network on"),
    ] {
        if !*now {
            ferro_ui::admin::run(SWITCHES, &[what, "off"], None);
        } else if !before && !ferro_ui::admin::run(SWITCHES, &[what, "on"], Some(asking)) {
            *now = false;
        }
    }
    store.switches.network = st.net_switch;
    store.switches.microphone = st.mic_switch;
    store.switches.camera = st.cam_switch;
    store.switches.vpn = st.vpn_enabled;
    store.switches.tor = st.tor_enabled;
    // With Pluggable Transports on and no bridges of your own, ferro-tor
    // uses FerroOS's built-in set (kept current with each release), so
    // nothing is copied in here to go stale. A line saved by earlier
    // versions (the old snowflake bridge) is dropped.
    store.bridges.retain(|b| !b.contains("2B280B23DD74683028100AB73E41B3BB52F4F354"));
    if !st.tor_bridges {
        store.bridges.clear();
    }
    let _ = store.save();

    // Save switches file
    let path = switches_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = format!(
        "# FerroOS Hardware & Privacy Switches\n\
         network = {}\n\
         microphone = {}\n\
         camera = {}\n\
         vpn = {}\n\
         tor = {}\n\
         tor_bridges = {}\n",
        st.net_switch, st.mic_switch, st.cam_switch, st.vpn_enabled, st.tor_enabled, st.tor_bridges
    );
    let _ = fs::write(path, content);

    // Notify ferro-system daemon via unix domain socket
    #[cfg(unix)]
    {
        use std::io::Write;
        let b = |v: bool| if v { 1 } else { 0 };
        let line = format!("SWITCHES {} {} {} {} {}", b(st.net_switch), b(st.mic_switch), b(st.cam_switch), b(st.vpn_enabled), b(st.tor_enabled));
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect("/run/ferro/system.sock") {
            let _ = writeln!(stream, "{line}");
        }
    }

    // Broadcast change to window manager via X11 root property
    if let (Some(c), Some(r)) = (conn, root) {
        if let Ok(atom) = c.intern_atom(false, b"_FERRO_SWITCHES") {
            if let Ok(rep) = atom.reply() {
                let data = [
                    u32::from(st.net_switch),
                    u32::from(st.mic_switch),
                    u32::from(st.cam_switch),
                    u32::from(st.vpn_enabled),
                    u32::from(st.tor_enabled),
                ];
                let _ = c.change_property32(PropMode::REPLACE, r, rep.atom, AtomEnum::CARDINAL, &data);
                let _ = c.flush();
            }
        }
    }
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, browser] = args.as_slice() {
        if flag == "--sign-in-page" {
            sign_in_page(browser);
            return Ok(());
        }
    }
    let mut state = load_state();

    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (root, visual, depth) = (screen.root, screen.root_visual, screen.root_depth);

    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
    let (wx, wy) = ((sw - W) / 2, (sh - H) / 2);

    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;
    conn.create_window(
        depth,
        win,
        root,
        wx as i16,
        wy as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new().background_pixel(ferro_gfx::color::FACE).event_mask(events),
    )?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;

    // Window title and WM_CLASS
    let wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Network & Internet Properties")?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, b"Network & Internet Properties")?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-network\0ferro-network\0")?;

    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;

    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut w = W;
    let mut h = H;

    let repaint = |w: i32, h: i32, st: &State| -> Res<()> {
        let mut s = Surface::new(w as usize, h as usize);
        view::draw(&mut s, w, h, st);
        let px = s.pixels();
        let mut row = 0;
        while row < h {
            let n = (h - row).min(64);
            let mut data = Vec::with_capacity(w as usize * n as usize * 4);
            for y in row..row + n {
                let start = y as usize * w as usize;
                for p in &px[start..start + w as usize] {
                    data.extend_from_slice(&p.to_le_bytes());
                }
            }
            conn.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, n as u16, 0, row as i16, 0, depth, &data)?;
            row += n;
        }
        conn.flush()?;
        Ok(())
    };

    repaint(w, h, &state)?;
    let found: Found = std::sync::Arc::new(std::sync::Mutex::new(None));
    rescan_in_background(win, found.clone());
    // What Tor and WireGuard were last switched to, so Apply only touches them when changed.
    let mut tor_applied = (state.tor_enabled, state.tor_bridges);
    let mut vpn_applied = state.vpn_enabled;

    // A key press handed on as a click on the focused control.
    let mut pending: Option<Event> = None;
    loop {
        let event = match pending.take() {
            Some(e) => e,
            None => conn.wait_for_event()?,
        };
        match event {
            Event::Expose(_) => {
                // The sign-in page may have switched itself off meanwhile.
                state.portal = portal_link();
                repaint(w, h, &state)?
            }
            Event::ConfigureNotify(e) => {
                w = i32::from(e.width);
                h = i32::from(e.height);
                repaint(w, h, &state)?;
            }
            Event::ClientMessage(e) => {
                if e.data.as_data32()[0] == wm_delete {
                    break;
                }
                if e.data.as_data32()[0] == WIFI_FOUND {
                    if let Some((aps, said)) = found.lock().ok().and_then(|mut f| f.take()) {
                        // The one picked stays picked, if it's still there.
                        let picked = state.selected_ap.and_then(|i| state.wifi_aps.get(i)).map(|a| a.ssid.clone());
                        state.selected_ap = picked.and_then(|p| aps.iter().position(|a| a.ssid == p));
                        state.wifi_aps = aps;
                        state.wifi_status = said;
                        repaint(w, h, &state)?;
                    }
                }
            }
            Event::KeyPress(e) => {
                let key = keymap.typed(&e);
                if let Some(diag) = &mut state.wifi_dialog {
                    match key {
                        _ if ferro_ui::dialog_answer(key) == Some(false) => {
                            state.wifi_dialog = None;
                            repaint(w, h, &state)?;
                        }
                        _ if ferro_ui::dialog_answer(key) == Some(true) => {
                            let ssid = diag.ssid.clone();
                            let password = diag.password.clone();
                            state.wifi_dialog = None;
                            state.wifi_status = format!("Connecting to {ssid}...");
                            repaint(w, h, &state)?;
                            connect_wifi(&ssid, &password, &mut state);
                            repaint(w, h, &state)?;
                        }
                        Key::Backspace => {
                            diag.password.pop();
                            repaint(w, h, &state)?;
                        }
                        Key::Char(c) if !c.is_control() && diag.password.len() < 64 => {
                            diag.password.push(c);
                            repaint(w, h, &state)?;
                        }
                        _ => {}
                    }
                } else {
                    // Tab moves between the controls, Space or Enter
                    // presses one (Enter alone is OK); on the Wi-Fi tab Up
                    // and Down choose a network.
                    match (key, ferro_ui::focus_key(&e, key, state.focus, &view::controls(w, h, &state))) {
                        _ if ferro_ui::dialog_answer(key) == Some(false) => break,
                        (_, ferro_ui::FocusKey::Moved(f)) => {
                            state.focus = f;
                            repaint(w, h, &state)?;
                        }
                        (_, ferro_ui::FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                        _ if ferro_ui::dialog_answer(key) == Some(true) => {
                            let r = view::layout(w, h, &state).ok_btn;
                            pending = Some(ferro_ui::click_event(win, r.x + r.w / 2, r.y + r.h / 2));
                        }
                        (Key::Up | Key::Down, _) if state.tab == 1 && !state.wifi_aps.is_empty() => {
                            let n = state.wifi_aps.len();
                            state.selected_ap = Some(match (state.selected_ap, key) {
                                (None, _) => 0,
                                (Some(i), Key::Up) => i.saturating_sub(1),
                                (Some(i), _) => (i + 1).min(n - 1),
                            });
                            repaint(w, h, &state)?;
                        }
                        _ => {}
                    }
                }
            }
            Event::ButtonPress(e) if e.detail == 1 => {
                let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                match view::hit(w, h, &state, x, y) {
                    Hit::Tab(i) => {
                        state.tab = i;
                        if i == 0 {
                            state.ethernet = load_ethernet_info();
                        }
                        state.portal = portal_link();
                        repaint(w, h, &state)?;
                    }
                    Hit::RenewDhcp => {
                        state.eth_diagnostic = Some("Requesting DHCP lease renewal...".into());
                        repaint(w, h, &state)?;
                        renew_ethernet(&state.ethernet.interface);
                        state.ethernet = load_ethernet_info();
                        state.eth_diagnostic = Some("DHCP lease renewal completed.".into());
                        repaint(w, h, &state)?;
                    }
                    Hit::DisconnectEth => {
                        disconnect_ethernet(&state.ethernet.interface);
                        state.ethernet = load_ethernet_info();
                        state.eth_diagnostic = Some("Ethernet disconnected.".into());
                        repaint(w, h, &state)?;
                    }
                    Hit::DiagnoseEth => {
                        diagnose_ethernet(&mut state);
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleVpn => {
                        let conf_exists = Path::new("/etc/ferro/vpn.conf").exists()
                            || Path::new("/etc/ferro/vpn.comf").exists()
                            || Path::new("/etc/wireguard/wg0.conf").exists()
                            || pasted_vpn_config().exists();
                        if !state.vpn_enabled && !conf_exists {
                            state.vpn_status = "No config found! Click Import... to select or paste one.".into();
                        } else {
                            state.vpn_enabled = !state.vpn_enabled;
                            state.vpn_status = if state.vpn_enabled { "Click Apply to connect.".into() } else { "Click Apply to disconnect.".into() };
                        }
                        repaint(w, h, &state)?;
                    }
                    Hit::ImportVpn => {
                        // What was tried, and how it went; None: nothing yet.
                        let mut tried: Option<Result<String, String>> = None;
                        let import = |config: &[u8], name: &str| Some(import_vpn(config).map(|()| name.to_string()));

                        // 1. A config left as /etc/ferro/vpn.comf.
                        if let Ok(config) = fs::read("/etc/ferro/vpn.comf") {
                            tried = import(&config, "/etc/ferro/vpn.comf");
                        }
                        // 2. The one picked. Cancelled: nothing more.
                        let mut cancelled = false;
                        if tried.is_none() {
                            let picked = Command::new("zenity")
                                .args([
                                    "--file-selection",
                                    "--title=Select WireGuard Configuration (.conf / .comf)",
                                    "--file-filter=WireGuard Config (*.conf *.comf)|*.conf *.comf",
                                    "--file-filter=All Files|*",
                                ])
                                .output();
                            match picked {
                                Ok(out) if out.status.success() => {
                                    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                    let name = Path::new(&chosen).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                    tried = Some(match fs::read(&chosen) {
                                        Ok(config) => import_vpn(&config).map(|()| name),
                                        Err(e) => Err(format!("Not imported: couldn't read {name} ({e}).")),
                                    });
                                }
                                Ok(_) => cancelled = true,
                                Err(_) => {}
                            }
                        }
                        // 3. No picker: a config copied to the clipboard.
                        if tried.is_none() && !cancelled {
                            if let Ok(out) = Command::new("xclip").args(["-o", "-selection", "clipboard"]).output() {
                                if out.stdout.windows(11).any(|w| w == b"[Interface]") {
                                    tried = import(&out.stdout, "the config from the clipboard");
                                }
                            }
                        }
                        match tried {
                            Some(Ok(name)) => {
                                state.vpn_status = imported_text(&name);
                                state.vpn_enabled = true;
                            }
                            Some(Err(why)) => state.vpn_status = why,
                            None if cancelled => {}
                            // 4. Or one to paste into: Apply hands it to ferro-vpn.
                            None => {
                                let conf_path = pasted_vpn_config();
                                if !conf_path.exists() {
                                    let tmpl = "# WireGuard VPN Configuration\n[Interface]\nPrivateKey = \nAddress = \nDNS = \n\n[Peer]\nPublicKey = \nEndpoint = \nAllowedIPs = 0.0.0.0/0\n";
                                    if let Some(dir) = conf_path.parent() {
                                        let _ = fs::create_dir_all(dir);
                                    }
                                    let _ = fs::write(&conf_path, tmpl);
                                }
                                ferro_associations::open_in_program(&conf_path);
                                state.vpn_status = "Opened editor. Paste config, save (Ctrl+S), then click Apply.".into();
                            }
                        }
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleTor => {
                        state.tor_enabled = !state.tor_enabled;
                        state.tor_status = if state.tor_enabled { "Click Apply to connect.".into() } else { "Click Apply to switch Tor off.".into() };
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleTorBridges => {
                        state.tor_bridges = !state.tor_bridges;
                        repaint(w, h, &state)?;
                    }
                    Hit::ScanWifi => {
                        state.wifi_status = "Looking for wireless networks...".into();
                        rescan_in_background(win, found.clone());
                        repaint(w, h, &state)?;
                    }
                    Hit::SelectAp(idx) => {
                        state.selected_ap = Some(idx);
                        repaint(w, h, &state)?;
                    }
                    Hit::ConnectWifi => {
                        if let Some(idx) = state.selected_ap {
                            if let Some(ap) = state.wifi_aps.get(idx) {
                                state.wifi_dialog = Some(WifiDialog { ssid: ap.ssid.clone(), password: String::new() });
                                repaint(w, h, &state)?;
                            }
                        }
                    }
                    Hit::DisconnectWifi => {
                        disconnect_wifi(&mut state);
                        repaint(w, h, &state)?;
                    }
                    Hit::ResetWifi => {
                        state.wifi_status = "Resetting Wi-Fi...".into();
                        repaint(w, h, &state)?;
                        reset_wifi(&mut state);
                        // The list as it is now, under what the reset said.
                        state.wifi_aps = wifi_scan(false).0;
                        repaint(w, h, &state)?;
                    }
                    Hit::TogglePortal => {
                        if state.portal.take().is_some() {
                            stop_sign_in(&mut state);
                        } else {
                            allow_sign_in(&mut state);
                        }
                        repaint(w, h, &state)?;
                    }
                    Hit::WifiOk => {
                        if let Some(diag) = state.wifi_dialog.take() {
                            state.wifi_status = format!("Connecting to {}...", diag.ssid);
                            repaint(w, h, &state)?;
                            connect_wifi(&diag.ssid, &diag.password, &mut state);
                            repaint(w, h, &state)?;
                        }
                    }
                    Hit::WifiCancel => {
                        state.wifi_dialog = None;
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleNetSwitch => {
                        state.net_switch = !state.net_switch;
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleMicSwitch => {
                        state.mic_switch = !state.mic_switch;
                        repaint(w, h, &state)?;
                    }
                    Hit::ToggleCamSwitch => {
                        state.cam_switch = !state.cam_switch;
                        repaint(w, h, &state)?;
                    }
                    Hit::Ok => {
                        save_state(&mut state, Some(&conn), Some(root));
                        if state.vpn_enabled != vpn_applied {
                            let _ = apply_vpn(state.vpn_enabled);
                        }
                        if (state.tor_enabled, state.tor_bridges) != tor_applied {
                            let _ = apply_tor(state.tor_enabled, state.tor_bridges);
                        }
                        break;
                    }
                    Hit::Apply => {
                        save_state(&mut state, Some(&conn), Some(root));
                        // A config pasted into Import's editor: the VPN's now,
                        // a new one or in place of the one there was.
                        if state.vpn_enabled {
                            if let Ok(config) = fs::read(pasted_vpn_config()) {
                                if filled_in(&config) {
                                    match import_vpn(&config) {
                                        Ok(()) => {
                                            let _ = fs::remove_file(pasted_vpn_config());
                                            state.vpn_status = imported_text("the pasted config");
                                        }
                                        Err(why) => state.vpn_status = why,
                                    }
                                }
                            }
                        }
                        if state.vpn_enabled != vpn_applied {
                            let conf_exists = Path::new("/etc/ferro/vpn.conf").exists()
                                || Path::new("/etc/ferro/vpn.comf").exists()
                                || Path::new("/etc/wireguard/wg0.conf").exists()
                                || pasted_vpn_config().exists();
                            if state.vpn_enabled && !conf_exists {
                                state.vpn_status = "No config found in /etc/ferro/vpn.conf. Click Import...".into();
                                state.vpn_enabled = false;
                            } else if apply_vpn(state.vpn_enabled) {
                                vpn_applied = state.vpn_enabled;
                                if let Ok(vpn) = fs::read_to_string("/run/ferro/vpn-status") {
                                    state.vpn_status = vpn_status_text(&vpn);
                                }
                            } else {
                                // ferro-vpn says why it failed (what wg-quick said).
                                state.vpn_status = fs::read_to_string("/run/ferro/vpn-status")
                                    .ok()
                                    .filter(|v| v.contains("state=error"))
                                    .map(|v| vpn_status_text(&v))
                                    .unwrap_or_else(|| "Failed to switch WireGuard (check /etc/ferro/vpn.conf).".into());
                            }
                        }
                        if (state.tor_enabled, state.tor_bridges) != tor_applied {
                            if apply_tor(state.tor_enabled, state.tor_bridges) {
                                tor_applied = (state.tor_enabled, state.tor_bridges);
                                if let Ok(tor) = fs::read_to_string("/run/ferro/tor-status") {
                                    state.tor_status = tor_status_text(&tor);
                                }
                            } else {
                                state.tor_status = "Couldn't switch Tor: no permission (sudo).".into();
                            }
                        }
                        repaint(w, h, &state)?;
                    }
                    Hit::Cancel => {
                        break;
                    }
                    Hit::Nothing => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnose_says_where_it_stops() {
        let ok = Findings { gateway: Some(true), names: true, internet: true };
        assert!(diagnosis("eth0", ok).starts_with("Diagnostic OK"));
        assert!(diagnosis("eth0", ok).contains("eth0"));
        assert!(diagnosis("eth0", Findings { names: false, ..ok }).contains("DNS"));
        assert!(diagnosis("eth0", Findings { internet: false, names: false, ..ok }).contains("router answers"));
        assert!(diagnosis("eth0", Findings { gateway: Some(false), internet: false, names: false }).contains("doesn't answer"));
        assert!(diagnosis("eth0", Findings { gateway: None, internet: false, names: false }).contains("no router"));
        // A router that won't say so, with the internet working, is fine.
        assert!(diagnosis("eth0", Findings { gateway: Some(false), ..ok }).starts_with("Diagnostic OK"));
        for f in [Findings { names: false, ..ok }, Findings { gateway: None, internet: false, names: false }] {
            assert!(diagnosis("eth0", f).starts_with("Problem"), "shown in red");
        }
    }

    #[test]
    fn reads_the_neighbour_table() {
        assert_eq!(neighbour_state("192.168.1.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff REACHABLE\n"), Some(true));
        assert_eq!(neighbour_state("192.168.1.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff STALE\n"), Some(true));
        assert_eq!(neighbour_state("192.168.1.1 dev eth0 FAILED\n"), Some(false));
        assert_eq!(neighbour_state("192.168.1.1 dev eth0 INCOMPLETE\n"), None);
        assert_eq!(neighbour_state(""), None);
    }

    #[test]
    fn says_why_a_config_wasnt_taken() {
        let said = "ferro-vpn: left out 2 line(s) a VPN config doesn't need (commands to run, unknown settings)\n\
                    ferro-vpn: what was given isn't a WireGuard config (no [Interface] section)\n";
        assert_eq!(import_refusal(said), "Not imported: what was given isn't a WireGuard config (no [Interface] section).");
        assert_eq!(import_refusal("sudo: no password was provided\n"), "Not imported: the password wasn't given.");
        assert_eq!(import_refusal(""), "Not imported: the password wasn't given.");
    }

    #[test]
    fn a_pasted_config_only_once_filled_in() {
        let form = "# WireGuard VPN Configuration\n[Interface]\nPrivateKey = \nAddress = \n\n[Peer]\nPublicKey = \n";
        assert!(!filled_in(form.as_bytes()));
        assert!(filled_in(form.replace("PrivateKey = ", "PrivateKey = aBcD=").as_bytes()));
        assert!(!filled_in(b"PrivateKey = aBcD="));
    }

    fn wifi(ssid: &str, key_mgmt: &str, psk: &str, hidden: bool) -> SavedWifi {
        SavedWifi { ssid: ssid.into(), hidden, key_mgmt: key_mgmt.into(), psk: psk.into() }
    }

    #[test]
    fn reads_a_saved_network_and_its_password() {
        // As nmcli 1.52 prints it: colons and backslashes left as they are.
        let shown = "connection.id:Home: Upstairs\n802-11-wireless.ssid:Home: Upstairs\n802-11-wireless.hidden:yes\n\
                     802-11-wireless-security.key-mgmt:wpa-psk\n802-11-wireless-security.psk:pa:ss\\word1\n";
        assert_eq!(parse_saved_wifi(shown), Some(wifi("Home: Upstairs", "wpa-psk", "pa:ss\\word1", true)));
        assert_eq!(parse_saved_wifi("connection.id:Wired\n802-3-ethernet.mtu:auto\n"), None);
    }

    #[test]
    fn resets_the_network_in_use_first_then_the_last_used() {
        let list = "aaaa:802-3-ethernet:900:yes\nbbbb:802-11-wireless:100:no\ncccc:802-11-wireless:50:yes\ndddd:802-11-wireless:300:no\n";
        assert_eq!(wifi_profiles(list), ["cccc", "dddd", "bbbb"]);
        assert!(wifi_profiles("aaaa:802-3-ethernet:900:yes\n").is_empty());
    }

    #[test]
    fn joins_again_only_what_it_can_set_up() {
        assert_eq!(reconnect_args(&wifi("Home", "wpa-psk", "secret", false)).unwrap(), ["dev", "wifi", "connect", "Home", "password", "secret"]);
        assert_eq!(reconnect_args(&wifi("Cafe", "", "", true)).unwrap(), ["dev", "wifi", "connect", "Cafe", "hidden", "yes"]);
        // No password read, or a work/school sign-in: nothing is forgotten.
        assert!(reconnect_args(&wifi("Home", "sae", "", false)).is_err());
        assert!(reconnect_args(&wifi("Work", "wpa-eap", "", false)).is_err());
        assert_eq!(
            resave_args(&wifi("Home", "sae", "secret", false)),
            ["connection", "add", "type", "wifi", "con-name", "Home", "ssid", "Home", "wifi-sec.key-mgmt", "sae", "wifi-sec.psk", "secret"]
        );
    }

    #[test]
    fn reads_what_ferro_portal_says() {
        assert_eq!(portal_field("link=wlan0\nstarted=1700000000\n", "link=").as_deref(), Some("wlan0"));
        assert_eq!(portal_field("state=off\n", "link="), None);
        assert_eq!(portal_field("link=\n", "link="), None);
    }

    #[test]
    fn refusals_in_its_own_words_or_no_permission() {
        assert_eq!(portal_refusal("Tor is on: switch Tor off first to sign in.\n"), "Tor is on: switch Tor off first to sign in.");
        assert_eq!(portal_refusal("sudo: a password is required\n"), "Couldn't allow it: no permission (sudo).");
        assert_eq!(portal_refusal(""), "Couldn't allow it: no permission (sudo).");
    }
}
