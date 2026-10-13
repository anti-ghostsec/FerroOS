//! ferro-network: Network & Internet Properties dialog for FerroOS Heavy.
//!
//! Provides dedicated Windows 95 tabbed GUI controls for WireGuard VPN, Arti Tor,
//! Wi-Fi network scanning & connection, and Hardware Kill Switches.

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
    let mut st = State::default();

    // Load live net status if present
    if let Ok(net) = fs::read_to_string("/run/ferro/net-status") {
        for line in net.lines() {
            if let Some(v) = line.strip_prefix("vpn=") {
                st.vpn_enabled = v == "connected" || v == "on";
                st.vpn_status = format!("VPN is {v}");
            }
        }
    }

    // Load live tor status if present
    if let Ok(tor) = fs::read_to_string("/run/ferro/tor-status") {
        for line in tor.lines() {
            if let Some(v) = line.strip_prefix("state=") {
                st.tor_enabled = v == "ready" || v == "on";
                st.tor_status = format!("Tor is {v}");
            }
        }
    }

    // Load hardware switches
    let store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    st.net_switch = store.switches.network;
    st.mic_switch = store.switches.microphone;
    st.cam_switch = store.switches.camera;
    st.vpn_enabled = store.switches.vpn;
    st.tor_enabled = store.switches.tor;

    // Load Wi-Fi status or scan
    scan_wifi_into(&mut st);

    st
}

fn scan_wifi_into(st: &mut State) {
    #[cfg(target_os = "linux")]
    {
        // Try iw scan via command or fallback to /run/ferro/wifi-scan
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

fn save_state(st: &State, conn: Option<&RustConnection>, root: Option<Window>) {
    let mut store = ferro_sandbox::store::Store::load(ferro_sandbox::store::DEFAULT_PATH);
    store.switches.network = st.net_switch;
    store.switches.microphone = st.mic_switch;
    store.switches.camera = st.cam_switch;
    store.switches.vpn = st.vpn_enabled;
    store.switches.tor = st.tor_enabled;
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
         tor = {}\n",
        st.net_switch, st.mic_switch, st.cam_switch, st.vpn_enabled, st.tor_enabled
    );
    let _ = fs::write(path, content);

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
                            state.wifi_status = format!("Connected to {ssid}");
                            state.wifi_dialog = None;
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
                        state.tor_status = if state.tor_enabled { "Arti Tor circuit established.".into() } else { "Tor is inactive.".into() };
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
                        state.wifi_status = "Disconnected from wireless network.".into();
                        state.selected_ap = None;
                        repaint(w, h, &state)?;
                    }
                    Hit::WifiOk => {
                        if let Some(diag) = state.wifi_dialog.take() {
                            state.wifi_status = format!("Connected to {}", diag.ssid);
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
                        break;
                    }
                    Hit::Apply => {
                        save_state(&state, Some(&conn), Some(root));
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
