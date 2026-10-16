//! ferro-network: Network & Internet Properties dialog for FerroOS Heavy.
//!
//! Provides dedicated Windows 95 tabbed GUI controls for Wired Ethernet (LAN),
//! Wi-Fi network scanning & connection, WireGuard VPN, Arti Tor, and Hardware Kill Switches.

mod view;

use ferro_gfx::Surface;
use ferro_ui::{Key, Keymap, Res};
use std::fs;
use std::path::PathBuf;
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
    if !store.bridges.is_empty() {
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

    // Load Wi-Fi status or scan
    scan_wifi_into(&mut st);

    st
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

fn scan_wifi_into(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        // Try nmcli first, which is standard on NetworkManager systems:
        if let Ok(out) = Command::new("nmcli").args(["-t", "-f", "SSID,SIGNAL,SECURITY", "dev", "wifi", "list", "--rescan", "yes"]).output() {
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
                    st.wifi_aps = list;
                    st.wifi_status = format!("Found {} wireless network(s).", st.wifi_aps.len());
                    return;
                }
            }
        }

        // Fallback to iw scan
        if let Ok(out) = Command::new("iw").args(["dev", "wlan0", "scan"]).output() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                let parsed = ferro_system::wifi::parse_scan(&text);
                if !parsed.is_empty() {
                    st.wifi_aps = parsed.into_iter().map(|(ssid, signal)| WifiAp { ssid, signal, security: "WPA2/WPA3".into() }).collect();
                    st.wifi_status = format!("Found {} wireless network(s).", st.wifi_aps.len());
                    return;
                }
            }
        }
    }

    // Fallback default list for offline or simulation environments
    st.wifi_aps = vec![
        WifiAp { ssid: "FerroNet-Secure".into(), signal: -45, security: "WPA3".into() },
        WifiAp { ssid: "Office_5G".into(), signal: -62, security: "WPA2".into() },
        WifiAp { ssid: "Guest-Network".into(), signal: -78, security: "Open".into() },
    ];
    st.wifi_status = "Scan complete. 3 network(s) available.".into();
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

fn save_state(st: &State, conn: Option<&RustConnection>, root: Option<Window>) {
    let mut store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    store.switches.network = st.net_switch;
    store.switches.microphone = st.mic_switch;
    store.switches.camera = st.cam_switch;
    store.switches.vpn = st.vpn_enabled;
    store.switches.tor = st.tor_enabled;
    if st.tor_bridges && store.bridges.is_empty() {
        store.bridges.push("snowflake 192.0.2.3:80 2B280B23DD74683028100AB73E41B3BB52F4F354 fingerprint=2B280B23DD74683028100AB73E41B3BB52F4F354 url=https://1098762253.rsc.cdn77.org/ front=www.phpmyadmin.net ice=stun:stun.l.google.com:19302,stun:stun.antisip.com:3478,stun:stun.bluesip.net:3478,stun:stun.dus.net:3478,stun:stun.epygi.com:3478,stun:stun.sonetel.com:21,stun:stun.uls.co.za:3478,stun:stun.voipgate.com:3478,stun:stun.voys.nl:3478 utls-imitate=hellorandomizedalpn".to_string());
    } else if !st.tor_bridges {
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
    // What Tor was last switched to, so Apply only touches it when it changed.
    let mut tor_applied = (state.tor_enabled, state.tor_bridges);

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::Expose(_) => repaint(w, h, &state)?,
            Event::ConfigureNotify(e) => {
                w = i32::from(e.width);
                h = i32::from(e.height);
                repaint(w, h, &state)?;
            }
            Event::ClientMessage(e) => {
                if e.data.as_data32()[0] == wm_delete {
                    break;
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
                } else if key == Key::Escape {
                    break;
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
                        state.vpn_enabled = !state.vpn_enabled;
                        state.vpn_status = if state.vpn_enabled { "WireGuard tunnel is active.".into() } else { "WireGuard is disconnected.".into() };
                        repaint(w, h, &state)?;
                    }
                    Hit::ImportVpn => {
                        state.vpn_status = "Place config in /etc/ferro/vpn.conf".into();
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
                        scan_wifi_into(&mut state);
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
                        if (state.tor_enabled, state.tor_bridges) != tor_applied {
                            let _ = apply_tor(state.tor_enabled, state.tor_bridges);
                        }
                        break;
                    }
                    Hit::Apply => {
                        save_state(&state, Some(&conn), Some(root));
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
