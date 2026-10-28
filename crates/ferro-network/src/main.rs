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
    if let Ok(vpn) = fs::read_to_string("/run/ferro/vpn-status") {
        st.vpn_status = vpn_status_text(&vpn);
        if let Some(v) = vpn.lines().find_map(|l| l.strip_prefix("state=")) {
            st.vpn_enabled = matches!(v, "connected" | "on" | "active");
        }
    } else {
        let _ = Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-vpn", "status"]).status();
        if let Ok(vpn) = fs::read_to_string("/run/ferro/vpn-status") {
            st.vpn_status = vpn_status_text(&vpn);
            if let Some(v) = vpn.lines().find_map(|l| l.strip_prefix("state=")) {
                st.vpn_enabled = matches!(v, "connected" | "on" | "active");
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

fn diagnose_ethernet(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        let gw_target = if st.ethernet.gateway.contains('.') { &st.ethernet.gateway } else { "9.9.9.9" };
        let ping = Command::new("ping").args(["-c", "1", "-W", "2", gw_target]).output();
        let is_ok = ping.is_ok_and(|o| o.status.success());
        st.eth_diagnostic = Some(if is_ok {
            format!("Diagnostic OK: Gateway & Internet reachable via {}", st.ethernet.interface)
        } else {
            "Diagnostic Warning: Gateway unreachable. Check router / cable.".to_string()
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        st.eth_diagnostic = Some("Diagnostic OK: Gateway & Internet reachable (Simulated)".into());
    }
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

/// The message `rescan_in_background` wakes the window with.
const WIFI_FOUND: u32 = 0x5749_4649;

/// Switches WireGuard VPN on or off through `ferro-vpn` (heavy/ferro-vpn),
/// which runs as root: it configures the wg0 interface and routes traffic
/// through the tunnel with kill-switch protection.
fn apply_vpn(enabled: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        let arg = if enabled { "on" } else { "off" };
        let ok = Command::new("sudo")
            .args(["-n", "/usr/local/sbin/ferro-vpn", arg])
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
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

/// Hands a WireGuard config to `ferro-vpn import`, which keeps it as the VPN's.
/// It goes on standard input, read here with our own rights: ferro-vpn runs
/// as root and takes no file names, so it can't be pointed at a file we
/// couldn't read ourselves.
fn import_vpn(config: &[u8]) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    let Ok(mut child) = Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-vpn", "import"]).stdin(Stdio::piped()).stdout(Stdio::null()).spawn()
    else {
        return false;
    };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(config).is_ok());
    child.wait().is_ok_and(|s| s.success()) && wrote
}

/// Switches Tor on or off through `ferro-tor` (heavy/ferro-tor), which runs as
/// root: it starts Tor and puts in the firewall rules that send every
/// program's traffic and DNS through it. Starting Tor alone changes nothing
/// for programs that aren't set up to use its proxy.
fn apply_tor(enabled: bool, bridges: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut args = vec!["-n", "/usr/local/sbin/ferro-tor"];
        if enabled {
            args.push("on");
            if bridges {
                args.push("bridges");
            }
        } else {
            args.push("off");
        }
        let ok = Command::new("sudo").args(&args).stdin(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
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
const BROWSERS: [&str; 3] = ["librewolf", "firefox-esr", "firefox"];

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
    let out = Command::new("sudo").args(["-n", PORTAL, "on"]).stdin(std::process::Stdio::null()).output();
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

fn save_state(st: &State, conn: Option<&RustConnection>, root: Option<Window>) {
    let mut store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
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

    // Enforce hardware kill switches immediately via elevated helper
    let mic_arg = if st.mic_switch { "on" } else { "off" };
    let cam_arg = if st.cam_switch { "on" } else { "off" };
    let net_arg = if st.net_switch { "on" } else { "off" };
    let _ = std::process::Command::new("sudo").args(["-n", "/usr/local/sbin/ferro-switch-hardware", "apply-all", mic_arg, cam_arg, net_arg]).status();

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
                        Key::Escape => {
                            state.wifi_dialog = None;
                            repaint(w, h, &state)?;
                        }
                        Key::Enter => {
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
                        (Key::Escape, _) => break,
                        (_, ferro_ui::FocusKey::Moved(f)) => {
                            state.focus = f;
                            repaint(w, h, &state)?;
                        }
                        (_, ferro_ui::FocusKey::Click(x, y)) => pending = Some(ferro_ui::click_event(win, x, y)),
                        (Key::Enter, _) => {
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
                            || Path::new("/etc/wireguard/wg0.conf").exists();
                        if !state.vpn_enabled && !conf_exists {
                            state.vpn_status = "No config found! Click Import... to select or paste one.".into();
                        } else {
                            state.vpn_enabled = !state.vpn_enabled;
                            state.vpn_status = if state.vpn_enabled { "Click Apply to connect.".into() } else { "Click Apply to disconnect.".into() };
                        }
                        repaint(w, h, &state)?;
                    }
                    Hit::ImportVpn => {
                        let mut imported = false;

                        // 1. If vpn.comf exists in /etc/ferro, import it
                        if let Ok(config) = fs::read("/etc/ferro/vpn.comf") {
                            if import_vpn(&config) {
                                imported = true;
                                state.vpn_status = "Imported /etc/ferro/vpn.comf! Click Apply to connect.".into();
                                state.vpn_enabled = true;
                            }
                        }

                        // 2. Open interactive file picker via zenity
                        if !imported {
                            let zenity_res = Command::new("zenity")
                                .args([
                                    "--file-selection",
                                    "--title=Select WireGuard Configuration (.conf / .comf)",
                                    "--file-filter=WireGuard Config (*.conf *.comf)|*.conf *.comf",
                                    "--file-filter=All Files|*",
                                ])
                                .output();
                            if let Ok(out) = zenity_res {
                                if out.status.success() {
                                    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                    if let Some(config) = (!chosen.is_empty()).then(|| fs::read(&chosen).ok()).flatten() {
                                        if import_vpn(&config) {
                                            imported = true;
                                            let fname = Path::new(&chosen).file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                                            state.vpn_status = format!("Imported {}! Click Apply to connect.", fname);
                                            state.vpn_enabled = true;
                                        }
                                    }
                                }
                            }
                        }

                        // 3. Scan ~/Downloads, ~/Desktop, ~/.config/ferro for any .conf or .comf
                        if !imported {
                            if let Ok(home) = std::env::var("HOME") {
                                let h = PathBuf::from(home);
                                let dirs = [h.join("Downloads"), h.join("Desktop"), h.join(".config/ferro"), PathBuf::from("/etc/ferro")];
                                for dir in &dirs {
                                    if let Ok(entries) = fs::read_dir(dir) {
                                        for entry in entries.flatten() {
                                            let path = entry.path();
                                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                                if (ext == "conf" || ext == "comf") && !path.ends_with("vpn.conf") {
                                                    if let Ok(content) = fs::read_to_string(&path) {
                                                        if (content.contains("[Interface]") || content.contains("PrivateKey"))
                                                            && import_vpn(content.as_bytes())
                                                        {
                                                            imported = true;
                                                            state.vpn_status = format!(
                                                                "Imported {}! Click Apply to connect.",
                                                                path.file_name().unwrap().to_string_lossy()
                                                            );
                                                            state.vpn_enabled = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if imported {
                                        break;
                                    }
                                }
                            }
                        }

                        // 4. Check clipboard for [Interface]
                        if !imported {
                            if let Ok(out) = Command::new("xclip").args(["-o", "-selection", "clipboard"]).output() {
                                if let Ok(text) = String::from_utf8(out.stdout) {
                                    if text.contains("[Interface]") && import_vpn(text.as_bytes()) {
                                        imported = true;
                                        state.vpn_status = "Imported config from clipboard! Click Apply to connect.".into();
                                        state.vpn_enabled = true;
                                    }
                                }
                            }
                        }

                        // 5. If still not imported, open editor for user to paste configuration
                        if !imported {
                            let conf_path = "/etc/ferro/vpn.conf";
                            if !Path::new(conf_path).exists() {
                                let tmpl = "# WireGuard VPN Configuration\n[Interface]\nPrivateKey = \nAddress = \nDNS = \n\n[Peer]\nPublicKey = \nEndpoint = \nAllowedIPs = 0.0.0.0/0\n";
                                let _ = fs::write(conf_path, tmpl);
                            }
                            let _ = Command::new("geany").arg(conf_path).spawn();
                            state.vpn_status = "Opened editor. Paste config, save (Ctrl+S), then click Apply.".into();
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
                        save_state(&state, Some(&conn), Some(root));
                        if state.vpn_enabled != vpn_applied {
                            let _ = apply_vpn(state.vpn_enabled);
                        }
                        if (state.tor_enabled, state.tor_bridges) != tor_applied {
                            let _ = apply_tor(state.tor_enabled, state.tor_bridges);
                        }
                        break;
                    }
                    Hit::Apply => {
                        save_state(&state, Some(&conn), Some(root));
                        if state.vpn_enabled != vpn_applied {
                            let conf_exists = Path::new("/etc/ferro/vpn.conf").exists()
                                || Path::new("/etc/ferro/vpn.comf").exists()
                                || Path::new("/etc/wireguard/wg0.conf").exists();
                            if state.vpn_enabled && !conf_exists {
                                state.vpn_status = "No config found in /etc/ferro/vpn.conf. Click Import...".into();
                                state.vpn_enabled = false;
                            } else if apply_vpn(state.vpn_enabled) {
                                vpn_applied = state.vpn_enabled;
                                if let Ok(vpn) = fs::read_to_string("/run/ferro/vpn-status") {
                                    state.vpn_status = vpn_status_text(&vpn);
                                }
                            } else {
                                state.vpn_status = "Failed to switch WireGuard (check /etc/ferro/vpn.conf).".into();
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
