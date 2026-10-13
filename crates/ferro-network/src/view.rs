//! Pure geometry, layout, and rendering for FerroOS Heavy's Network & Internet Properties dialog.

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::{button, text_field};

pub const W: i32 = 500;
pub const H: i32 = 380;

const TABS: [&str; 3] = ["VPN & Tor", "Wi-Fi Networks", "Hardware Switches"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiAp {
    pub ssid: String,
    pub signal: i32,
    pub security: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiDialog {
    pub ssid: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub tab: usize,
    pub vpn_enabled: bool,
    pub vpn_status: String,
    pub tor_enabled: bool,
    pub tor_status: String,
    pub tor_bridges: bool,
    pub wifi_aps: Vec<WifiAp>,
    pub selected_ap: Option<usize>,
    pub wifi_status: String,
    pub wifi_dialog: Option<WifiDialog>,
    pub net_switch: bool,
    pub mic_switch: bool,
    pub cam_switch: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            vpn_enabled: false,
            vpn_status: "WireGuard is disconnected.".into(),
            tor_enabled: false,
            tor_status: "Tor is inactive. Traffic uses standard routing.".into(),
            tor_bridges: false,
            wifi_aps: Vec::new(),
            selected_ap: None,
            wifi_status: "Wi-Fi is ready.".into(),
            wifi_dialog: None,
            net_switch: true,
            mic_switch: true,
            cam_switch: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    ToggleVpn,
    ToggleTor,
    ToggleTorBridges,
    ImportVpn,
    ScanWifi,
    SelectAp(usize),
    ConnectWifi,
    DisconnectWifi,
    WifiOk,
    WifiCancel,
    ToggleNetSwitch,
    ToggleMicSwitch,
    ToggleCamSwitch,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

pub struct Layout {
    pub tabs: [Rect; 3],
    pub panel: Rect,
    pub ok_btn: Rect,
    pub cancel_btn: Rect,
    pub apply_btn: Rect,
    // Tab 0
    pub vpn_box: Rect,
    pub vpn_chk: Rect,
    pub vpn_import: Rect,
    pub tor_box: Rect,
    pub tor_chk: Rect,
    pub tor_bridges_chk: Rect,
    // Tab 1
    pub wifi_list: Rect,
    pub scan_btn: Rect,
    pub connect_btn: Rect,
    pub disconnect_btn: Rect,
    // Tab 2
    pub net_chk: Rect,
    pub mic_chk: Rect,
    pub cam_chk: Rect,
    // Modal
    pub dialog_rect: Option<Rect>,
    pub dialog_ok: Option<Rect>,
    pub dialog_cancel: Option<Rect>,
    pub dialog_input: Option<Rect>,
}

pub fn layout(w: i32, h: i32, st: &State) -> Layout {
    let (w, h) = (w.max(W), h.max(H));
    let tabs = [Rect::new(8, 6, 90, 22), Rect::new(100, 6, 120, 22), Rect::new(222, 6, 140, 22)];
    let panel = Rect::new(4, 26, w - 8, h - 66);

    let ok_btn = Rect::new(w - 245, h - 32, 75, 24);
    let cancel_btn = Rect::new(w - 165, h - 32, 75, 24);
    let apply_btn = Rect::new(w - 85, h - 32, 75, 24);

    let p = panel.inset(8);

    // Tab 0
    let vpn_box = Rect::new(p.x, p.y + 4, p.w, 130);
    let vpn_chk = Rect::new(vpn_box.x + 12, vpn_box.y + 24, 240, 18);
    let vpn_import = Rect::new(vpn_box.right() - 120, vpn_box.y + 20, 110, 24);

    let tor_box = Rect::new(p.x, vpn_box.bottom() + 10, p.w, 130);
    let tor_chk = Rect::new(tor_box.x + 12, tor_box.y + 24, 260, 18);
    let tor_bridges_chk = Rect::new(tor_box.x + 12, tor_box.y + 50, 240, 18);

    // Tab 1
    let wifi_list = Rect::new(p.x + 4, p.y + 24, p.w - 120, p.h - 60);
    let scan_btn = Rect::new(p.right() - 105, p.y + 24, 100, 24);
    let connect_btn = Rect::new(p.right() - 105, p.y + 54, 100, 24);
    let disconnect_btn = Rect::new(p.right() - 105, p.y + 84, 100, 24);

    // Tab 2
    let net_chk = Rect::new(p.x + 16, p.y + 36, 300, 20);
    let mic_chk = Rect::new(p.x + 16, p.y + 76, 300, 20);
    let cam_chk = Rect::new(p.x + 16, p.y + 116, 300, 20);

    // Dialog
    let (dialog_rect, dialog_ok, dialog_cancel, dialog_input) = if st.wifi_dialog.is_some() {
        let (dw, dh) = (340, 140);
        let dr = Rect::new((w - dw) / 2, (h - dh) / 2, dw, dh);
        let din = Rect::new(dr.x + 16, dr.y + 56, dr.w - 32, 22);
        let dok = Rect::new(dr.right() - 165, dr.bottom() - 32, 72, 24);
        let dcan = Rect::new(dr.right() - 85, dr.bottom() - 32, 72, 24);
        (Some(dr), Some(dok), Some(dcan), Some(din))
    } else {
        (None, None, None, None)
    };

    Layout {
        tabs,
        panel,
        ok_btn,
        cancel_btn,
        apply_btn,
        vpn_box,
        vpn_chk,
        vpn_import,
        tor_box,
        tor_chk,
        tor_bridges_chk,
        wifi_list,
        scan_btn,
        connect_btn,
        disconnect_btn,
        net_chk,
        mic_chk,
        cam_chk,
        dialog_rect,
        dialog_ok,
        dialog_cancel,
        dialog_input,
    }
}

pub fn hit(w: i32, h: i32, st: &State, x: i32, y: i32) -> Hit {
    let l = layout(w, h, st);
    if st.wifi_dialog.is_some() {
        if let Some(ok) = l.dialog_ok {
            if ok.contains(x, y) {
                return Hit::WifiOk;
            }
        }
        if let Some(cancel) = l.dialog_cancel {
            if cancel.contains(x, y) {
                return Hit::WifiCancel;
            }
        }
        return Hit::Nothing;
    }

    if l.ok_btn.contains(x, y) {
        return Hit::Ok;
    }
    if l.cancel_btn.contains(x, y) {
        return Hit::Cancel;
    }
    if l.apply_btn.contains(x, y) {
        return Hit::Apply;
    }

    for (i, t) in l.tabs.iter().enumerate() {
        if t.contains(x, y) {
            return Hit::Tab(i);
        }
    }

    match st.tab {
        0 => {
            if l.vpn_chk.contains(x, y) {
                return Hit::ToggleVpn;
            }
            if l.vpn_import.contains(x, y) {
                return Hit::ImportVpn;
            }
            if l.tor_chk.contains(x, y) {
                return Hit::ToggleTor;
            }
            if l.tor_bridges_chk.contains(x, y) {
                return Hit::ToggleTorBridges;
            }
        }
        1 => {
            if l.scan_btn.contains(x, y) {
                return Hit::ScanWifi;
            }
            if l.connect_btn.contains(x, y) {
                return Hit::ConnectWifi;
            }
            if l.disconnect_btn.contains(x, y) {
                return Hit::DisconnectWifi;
            }
            if l.wifi_list.contains(x, y) {
                let row_h = 20;
                let clicked_idx = ((y - l.wifi_list.y - 2) / row_h) as usize;
                if clicked_idx < st.wifi_aps.len() {
                    return Hit::SelectAp(clicked_idx);
                }
            }
        }
        2 => {
            if l.net_chk.contains(x, y) {
                return Hit::ToggleNetSwitch;
            }
            if l.mic_chk.contains(x, y) {
                return Hit::ToggleMicSwitch;
            }
            if l.cam_chk.contains(x, y) {
                return Hit::ToggleCamSwitch;
            }
        }
        _ => {}
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, st: &State) {
    use color::{BLACK, FACE, NAVY, SHADOW, WHITE};
    s.fill(Rect::new(0, 0, w, h), FACE);
    let l = layout(w, h, st);

    s.bevel(l.panel, Bevel::Window);

    // Draw tabs
    for (k, (tab, label)) in l.tabs.iter().zip(TABS).enumerate() {
        let r = if st.tab == k { Rect::new(tab.x - 2, tab.y - 2, tab.w + 4, tab.h + 3) } else { *tab };
        s.fill(r, FACE);
        s.hline(r.x + 1, r.y, r.w - 2, WHITE);
        s.vline(r.x, r.y + 1, r.h - 1, WHITE);
        s.vline(r.right() - 1, r.y + 1, r.h - 1, BLACK);
        s.vline(r.right() - 2, r.y + 1, r.h - 1, SHADOW);
        s.text(r.x + (r.w - text_width(label)) / 2, r.y + 7, label, BLACK);
    }

    match st.tab {
        0 => draw_vpn_tor(s, st, &l),
        1 => draw_wifi(s, st, &l),
        2 => draw_switches(s, st, &l),
        _ => {}
    }

    // Bottom buttons
    button(s, l.ok_btn, "OK", true);
    button(s, l.cancel_btn, "Cancel", true);
    button(s, l.apply_btn, "Apply", true);

    // Modal password dialog if active
    if let Some(diag) = &st.wifi_dialog {
        if let Some(r) = l.dialog_rect {
            s.fill(r, FACE);
            s.bevel(r, Bevel::Window);
            let tb = Rect::new(r.x + 3, r.y + 3, r.w - 6, 18);
            s.fill(tb, NAVY);
            s.text_bold(tb.x + 6, tb.y + 3, "Connect to Wi-Fi", WHITE);
            s.text(r.x + 16, r.y + 34, &format!("Enter password for {}:", ellipsize(&diag.ssid, 20)), BLACK);
            if let Some(rin) = l.dialog_input {
                let stars = "*".repeat(diag.password.chars().count());
                text_field(s, rin, &stars, true);
            }
            if let Some(rok) = l.dialog_ok {
                button(s, rok, "Connect", true);
            }
            if let Some(rcan) = l.dialog_cancel {
                button(s, rcan, "Cancel", true);
            }
        }
    }
}

fn draw_checkbox(s: &mut Surface, r: Rect, checked: bool, label: &str) {
    let box_r = Rect::new(r.x, r.y + (r.h - 14) / 2, 14, 14);
    s.fill(box_r, color::WHITE);
    s.bevel(box_r, Bevel::Sunken);
    if checked {
        s.sprite_mono(box_r.x + 3, box_r.y + 3, icons::GLYPH_CHECK, color::BLACK);
    }
    s.text(box_r.right() + 8, r.y + (r.h - GLYPH_H) / 2, label, color::BLACK);
}

fn draw_vpn_tor(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, SHADOW};
    s.group_box(l.vpn_box, "WireGuard VPN");
    draw_checkbox(s, l.vpn_chk, st.vpn_enabled, "Send all traffic through WireGuard");
    button(s, l.vpn_import, "Import...", true);
    s.text(l.vpn_box.x + 14, l.vpn_box.y + 54, "Status:", SHADOW);
    s.text(l.vpn_box.x + 64, l.vpn_box.y + 54, &st.vpn_status, BLACK);
    s.text(l.vpn_box.x + 14, l.vpn_box.y + 78, "Interface: wg0 (Kernel WireGuard / ChaCha20-Poly1305)", SHADOW);
    s.text(l.vpn_box.x + 14, l.vpn_box.y + 96, "Kill Switch: Blocks all cleartext leaks outside tunnel", SHADOW);

    s.group_box(l.tor_box, "Tor (Arti Onion Routing)");
    draw_checkbox(s, l.tor_chk, st.tor_enabled, "Route all applications through Tor");
    draw_checkbox(s, l.tor_bridges_chk, st.tor_bridges, "Use Pluggable Transports (obfs4 / Snowflake)");
    s.text(l.tor_box.x + 14, l.tor_box.y + 78, "Status:", SHADOW);
    s.text(l.tor_box.x + 64, l.tor_box.y + 78, &st.tor_status, BLACK);
    s.text(l.tor_box.x + 14, l.tor_box.y + 100, "SOCKS Proxy: 127.0.0.1:9150 | DNS: 127.0.0.1:9053", SHADOW);
}

fn draw_wifi(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, NAVY, WHITE};
    s.group_box(Rect::new(l.panel.x + 8, l.panel.y + 8, l.panel.w - 16, l.panel.h - 16), "Available Wireless Networks");
    s.fill(l.wifi_list, WHITE);
    s.bevel(l.wifi_list, Bevel::Sunken);

    if st.wifi_aps.is_empty() {
        s.text(l.wifi_list.x + 10, l.wifi_list.y + 12, "No wireless networks found. Click Scan.", color::SHADOW);
    } else {
        let row_h = 20;
        for (i, ap) in st.wifi_aps.iter().enumerate() {
            let ry = l.wifi_list.y + 2 + i as i32 * row_h;
            if ry + row_h > l.wifi_list.bottom() {
                break;
            }
            let is_sel = st.selected_ap == Some(i);
            let row_rect = Rect::new(l.wifi_list.x + 2, ry, l.wifi_list.w - 4, row_h);
            if is_sel {
                s.fill(row_rect, NAVY);
            }
            let fg = if is_sel { WHITE } else { BLACK };
            let sig_bars = match ap.signal {
                -50..=0 => "[||||]",
                -65..=-51 => "[||| ]",
                -80..=-66 => "[||  ]",
                _ => "[|   ]",
            };
            s.text(row_rect.x + 6, ry + 4, sig_bars, fg);
            s.text(row_rect.x + 50, ry + 4, &ellipsize(&ap.ssid, 24), fg);
            s.text(row_rect.right() - 75, ry + 4, &ap.security, fg);
        }
    }

    button(s, l.scan_btn, "Scan", true);
    button(s, l.connect_btn, "Connect...", st.selected_ap.is_some());
    button(s, l.disconnect_btn, "Disconnect", true);

    s.text(l.panel.x + 16, l.panel.bottom() - 28, &format!("Status: {}", st.wifi_status), BLACK);
}

fn draw_switches(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, SHADOW};
    s.group_box(Rect::new(l.panel.x + 8, l.panel.y + 8, l.panel.w - 16, l.panel.h - 16), "Hardware Kill Switches");
    draw_checkbox(s, l.net_chk, st.net_switch, "Network Adapter (Wi-Fi & Ethernet) Enabled");
    s.text(l.net_chk.x + 22, l.net_chk.y + 20, "Toggling off disconnects all physical network hardware.", SHADOW);

    draw_checkbox(s, l.mic_chk, st.mic_switch, "Microphone Enabled");
    s.text(l.mic_chk.x + 22, l.mic_chk.y + 20, "Toggling off mutes and blocks all audio recording devices.", SHADOW);

    draw_checkbox(s, l.cam_chk, st.cam_switch, "Camera / Webcam Enabled");
    s.text(l.cam_chk.x + 22, l.cam_chk.y + 20, "Toggling off prevents kernel access to video capture nodes.", SHADOW);

    s.text(l.panel.x + 16, l.panel.bottom() - 32, "These switches enforce absolute zero-trust isolation at the kernel level.", BLACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_tabs_and_buttons() {
        let st = State::default();
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.tabs[0].x + 5, l.tabs[0].y + 5), Hit::Tab(0));
        assert_eq!(hit(W, H, &st, l.tabs[1].x + 5, l.tabs[1].y + 5), Hit::Tab(1));
        assert_eq!(hit(W, H, &st, l.tabs[2].x + 5, l.tabs[2].y + 5), Hit::Tab(2));
        assert_eq!(hit(W, H, &st, l.ok_btn.x + 5, l.ok_btn.y + 5), Hit::Ok);
        assert_eq!(hit(W, H, &st, l.cancel_btn.x + 5, l.cancel_btn.y + 5), Hit::Cancel);
        assert_eq!(hit(W, H, &st, l.apply_btn.x + 5, l.apply_btn.y + 5), Hit::Apply);
    }

    #[test]
    fn vpn_and_tor_toggle_hits() {
        let st = State { tab: 0, ..State::default() };
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.vpn_chk.x + 5, l.vpn_chk.y + 5), Hit::ToggleVpn);
        assert_eq!(hit(W, H, &st, l.tor_chk.x + 5, l.tor_chk.y + 5), Hit::ToggleTor);
        assert_eq!(hit(W, H, &st, l.tor_bridges_chk.x + 5, l.tor_bridges_chk.y + 5), Hit::ToggleTorBridges);
    }

    #[test]
    fn wifi_dialog_modal_hits() {
        let st = State { wifi_dialog: Some(WifiDialog { ssid: "TestAP".into(), password: "secret".into() }), ..State::default() };
        let l = layout(W, H, &st);
        let ok = l.dialog_ok.unwrap();
        let cancel = l.dialog_cancel.unwrap();
        assert_eq!(hit(W, H, &st, ok.x + 5, ok.y + 5), Hit::WifiOk);
        assert_eq!(hit(W, H, &st, cancel.x + 5, cancel.y + 5), Hit::WifiCancel);
        // Background hits blocked while dialog is open
        assert_eq!(hit(W, H, &st, l.ok_btn.x + 5, l.ok_btn.y + 5), Hit::Nothing);
    }

    #[test]
    fn draws_all_tabs() {
        for tab in 0..3 {
            let st = State { tab, ..State::default() };
            let mut s = Surface::new(W as usize, H as usize);
            draw(&mut s, W, H, &st);
        }
    }
}
