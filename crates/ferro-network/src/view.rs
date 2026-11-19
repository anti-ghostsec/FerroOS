//! Pure geometry, layout, and rendering for FerroOS Heavy's Network & Internet Properties dialog.

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::{button, default_button, text_field};

pub const W: i32 = 500;
pub const H: i32 = 390;

/// The VPN's list of configs: rows shown.
const VPN_ROWS: usize = 4;
const VPN_ROW_H: i32 = 14;

/// Under the list: what happens when a server stops answering, and how
/// many configs don't fit.
fn vpn_note(configs: usize) -> String {
    match configs.saturating_sub(VPN_ROWS) {
        0 => "If the server stops answering, the next config is used.".into(),
        more => format!("And {more} more. If a server stops answering: the next."),
    }
}

/// A config's name, from its ID ("02-mullvad-se" is "mullvad-se").
pub fn vpn_name(id: &str) -> &str {
    id.get(3..).filter(|_| id.as_bytes().get(2) == Some(&b'-')).unwrap_or(id)
}

const TABS: [&str; 4] = ["Ethernet (LAN)", "Wi-Fi Networks", "VPN & Tor", "Hardware Switches"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetInfo {
    pub interface: String,
    pub status: String,
    pub connected: bool,
    pub ip_address: String,
    pub subnet_mask: String,
    pub gateway: String,
    pub dns: String,
    pub mac_address: String,
    pub speed: String,
}

impl Default for EthernetInfo {
    fn default() -> Self {
        Self {
            interface: "eth0".into(),
            status: "Connected (DHCP)".into(),
            connected: true,
            ip_address: "192.168.1.150".into(),
            subnet_mask: "255.255.255.0".into(),
            gateway: "192.168.1.1".into(),
            dns: "9.9.9.9 (Quad9)".into(),
            mac_address: "52:54:00:12:34:56".into(),
            speed: "1000 Mbps (Full Duplex)".into(),
        }
    }
}

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
    pub ethernet: EthernetInfo,
    pub eth_diagnostic: Option<String>,
    pub vpn_enabled: bool,
    pub vpn_status: String,
    /// The VPN's configs (their IDs, "NN-name", in order), and which is in use.
    pub vpn_configs: Vec<String>,
    pub vpn_in_use: Option<String>,
    pub selected_vpn: Option<usize>,
    pub tor_enabled: bool,
    pub tor_status: String,
    pub tor_bridges: bool,
    pub wifi_aps: Vec<WifiAp>,
    pub selected_ap: Option<usize>,
    pub wifi_status: String,
    pub wifi_dialog: Option<WifiDialog>,
    /// The link whose sign-in page is allowed (plain lookups for a while),
    /// if any: what `ferro-portal` left in /run/ferro/portal.
    pub portal: Option<String>,
    pub net_switch: bool,
    pub mic_switch: bool,
    pub cam_switch: bool,
    /// Which of `controls` has the keyboard, if any (Tab moves it).
    pub focus: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            ethernet: EthernetInfo::default(),
            eth_diagnostic: None,
            vpn_enabled: false,
            vpn_status: "WireGuard is disconnected.".into(),
            vpn_configs: Vec::new(),
            vpn_in_use: None,
            selected_vpn: None,
            tor_enabled: false,
            tor_status: "Tor is inactive. Traffic uses standard routing.".into(),
            tor_bridges: false,
            wifi_aps: Vec::new(),
            selected_ap: None,
            wifi_status: "Wi-Fi is ready.".into(),
            wifi_dialog: None,
            portal: None,
            net_switch: true,
            mic_switch: true,
            cam_switch: true,
            focus: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    // Tab 0: Ethernet
    RenewDhcp,
    DisconnectEth,
    DiagnoseEth,
    // Tab 1: Wi-Fi
    ScanWifi,
    SelectAp(usize),
    ConnectWifi,
    DisconnectWifi,
    ResetWifi,
    TogglePortal,
    WifiOk,
    WifiCancel,
    // Tab 2: VPN & Tor
    ToggleVpn,
    ToggleTor,
    ToggleTorBridges,
    ImportVpn,
    SelectVpn(usize),
    UseVpn,
    RemoveVpn,
    // Tab 3: Hardware Switches
    ToggleNetSwitch,
    ToggleMicSwitch,
    ToggleCamSwitch,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

pub struct Layout {
    pub tabs: [Rect; 4],
    pub panel: Rect,
    pub ok_btn: Rect,
    pub cancel_btn: Rect,
    pub apply_btn: Rect,
    // Tab 0: Ethernet
    pub eth_box: Rect,
    pub renew_btn: Rect,
    pub eth_disconnect_btn: Rect,
    pub diagnose_btn: Rect,
    // Tab 1: Wi-Fi
    pub wifi_list: Rect,
    pub scan_btn: Rect,
    pub connect_btn: Rect,
    pub disconnect_btn: Rect,
    pub reset_wifi_btn: Rect,
    pub portal_chk: Rect,
    // Tab 2: VPN & Tor
    pub vpn_box: Rect,
    pub vpn_chk: Rect,
    pub vpn_import: Rect,
    pub vpn_list: Rect,
    pub vpn_use: Rect,
    pub vpn_remove: Rect,
    pub tor_box: Rect,
    pub tor_chk: Rect,
    pub tor_bridges_chk: Rect,
    // Tab 3: Switches
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
    let tabs = [Rect::new(8, 6, 105, 22), Rect::new(115, 6, 115, 22), Rect::new(232, 6, 95, 22), Rect::new(329, 6, 145, 22)];
    let panel = Rect::new(4, 26, w - 8, h - 66);

    let ok_btn = Rect::new(w - 245, h - 32, 75, 24);
    let cancel_btn = Rect::new(w - 165, h - 32, 75, 24);
    let apply_btn = Rect::new(w - 85, h - 32, 75, 24);

    let p = panel.inset(8);

    // Tab 0: Ethernet
    let eth_box = Rect::new(p.x, p.y + 4, p.w, 205);
    let renew_btn = Rect::new(p.x + 8, eth_box.bottom() + 10, 130, 24);
    let eth_disconnect_btn = Rect::new(renew_btn.right() + 10, eth_box.bottom() + 10, 100, 24);
    let diagnose_btn = Rect::new(eth_disconnect_btn.right() + 10, eth_box.bottom() + 10, 110, 24);

    // Tab 1: Wi-Fi
    let wifi_list = Rect::new(p.x + 4, p.y + 24, p.w - 120, p.h - 104);
    let scan_btn = Rect::new(p.right() - 105, p.y + 24, 100, 24);
    let connect_btn = Rect::new(p.right() - 105, p.y + 54, 100, 24);
    let disconnect_btn = Rect::new(p.right() - 105, p.y + 84, 100, 24);
    let reset_wifi_btn = Rect::new(p.right() - 105, p.y + 114, 100, 24);
    let portal_chk = Rect::new(wifi_list.x + 4, wifi_list.bottom() + 6, 300, 18);

    // Tab 2: VPN & Tor
    let vpn_box = Rect::new(p.x, p.y + 4, p.w, 166);
    let vpn_chk = Rect::new(vpn_box.x + 12, vpn_box.y + 24, 240, 18);
    let vpn_import = Rect::new(vpn_box.right() - 120, vpn_box.y + 20, 110, 24);
    let vpn_list = Rect::new(vpn_box.x + 12, vpn_box.y + 78, vpn_box.w - 144, VPN_ROWS as i32 * VPN_ROW_H + 4);
    let vpn_use = Rect::new(vpn_box.right() - 120, vpn_list.y, 110, 24);
    let vpn_remove = Rect::new(vpn_box.right() - 120, vpn_list.y + 30, 110, 24);

    let tor_box = Rect::new(p.x, vpn_box.bottom() + 8, p.w, 116);
    let tor_chk = Rect::new(tor_box.x + 12, tor_box.y + 24, 260, 18);
    let tor_bridges_chk = Rect::new(tor_box.x + 12, tor_box.y + 50, 240, 18);

    // Tab 3: Switches
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
        eth_box,
        renew_btn,
        eth_disconnect_btn,
        diagnose_btn,
        wifi_list,
        scan_btn,
        connect_btn,
        disconnect_btn,
        reset_wifi_btn,
        portal_chk,
        vpn_box,
        vpn_chk,
        vpn_import,
        vpn_list,
        vpn_use,
        vpn_remove,
        tor_box,
        tor_chk,
        tor_bridges_chk,
        net_chk,
        mic_chk,
        cam_chk,
        dialog_rect,
        dialog_ok,
        dialog_cancel,
        dialog_input,
    }
}

/// What can be clicked on the tab shown, in Tab order.
pub fn controls(w: i32, h: i32, st: &State) -> Vec<Rect> {
    let l = layout(w, h, st);
    let mut c = l.tabs.to_vec();
    match st.tab {
        0 => c.extend([l.renew_btn, l.eth_disconnect_btn, l.diagnose_btn]),
        1 => c.extend([l.scan_btn, l.connect_btn, l.disconnect_btn, l.reset_wifi_btn, l.portal_chk]),
        2 => c.extend([l.vpn_chk, l.vpn_import, l.vpn_use, l.vpn_remove, l.tor_chk, l.tor_bridges_chk]),
        3 => c.extend([l.net_chk, l.mic_chk, l.cam_chk]),
        _ => {}
    }
    c.extend([l.ok_btn, l.cancel_btn, l.apply_btn]);
    c
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
            if l.renew_btn.contains(x, y) {
                return Hit::RenewDhcp;
            }
            if l.eth_disconnect_btn.contains(x, y) {
                return Hit::DisconnectEth;
            }
            if l.diagnose_btn.contains(x, y) {
                return Hit::DiagnoseEth;
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
            if l.reset_wifi_btn.contains(x, y) {
                return Hit::ResetWifi;
            }
            if l.portal_chk.contains(x, y) {
                return Hit::TogglePortal;
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
            if l.vpn_chk.contains(x, y) {
                return Hit::ToggleVpn;
            }
            if l.vpn_import.contains(x, y) {
                return Hit::ImportVpn;
            }
            if l.vpn_use.contains(x, y) {
                return Hit::UseVpn;
            }
            if l.vpn_remove.contains(x, y) {
                return Hit::RemoveVpn;
            }
            if l.vpn_list.contains(x, y) {
                let i = ((y - l.vpn_list.y - 2) / VPN_ROW_H).max(0) as usize;
                if i < st.vpn_configs.len().min(VPN_ROWS) {
                    return Hit::SelectVpn(i);
                }
            }
            if l.tor_chk.contains(x, y) {
                return Hit::ToggleTor;
            }
            if l.tor_bridges_chk.contains(x, y) {
                return Hit::ToggleTorBridges;
            }
        }
        3 => {
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
        0 => draw_ethernet(s, st, &l),
        1 => draw_wifi(s, st, &l),
        2 => draw_vpn_tor(s, st, &l),
        3 => draw_switches(s, st, &l),
        _ => {}
    }

    // Bottom buttons
    default_button(s, l.ok_btn, "OK", true);
    button(s, l.cancel_btn, "Cancel", true);
    button(s, l.apply_btn, "Apply", true);

    if st.wifi_dialog.is_none() {
        // OK, the default button, has its dotted outline already.
        if let Some(r) = st.focus.and_then(|i| controls(w, h, st).get(i).copied()).filter(|&r| r != l.ok_btn) {
            ferro_ui::focus_rect(s, r.inset(2));
        }
    }

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
                default_button(s, rok, "Connect", true);
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

fn draw_ethernet(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, RED, SHADOW};
    s.group_box(l.eth_box, "Wired Ethernet Connection");

    let x_label = l.eth_box.x + 16;
    let x_val = l.eth_box.x + 145;
    let mut y = l.eth_box.y + 20;

    s.text(x_label, y, "Interface:", SHADOW);
    s.text_bold(x_val, y, &st.ethernet.interface, BLACK);
    y += 18;

    s.text(x_label, y, "Status:", SHADOW);
    let status_color = if st.ethernet.connected { 0x008000 } else { RED };
    s.text_bold(x_val, y, &st.ethernet.status, status_color);
    y += 18;

    s.text(x_label, y, "Link Speed:", SHADOW);
    s.text(x_val, y, &st.ethernet.speed, BLACK);
    y += 18;

    s.text(x_label, y, "Physical Address (MAC):", SHADOW);
    s.text(x_val, y, &st.ethernet.mac_address, BLACK);
    y += 18;

    s.text(x_label, y, "IPv4 Address:", SHADOW);
    s.text_bold(x_val, y, &st.ethernet.ip_address, BLACK);
    y += 18;

    s.text(x_label, y, "Subnet Mask:", SHADOW);
    s.text(x_val, y, &st.ethernet.subnet_mask, BLACK);
    y += 18;

    s.text(x_label, y, "Default Gateway:", SHADOW);
    s.text(x_val, y, &st.ethernet.gateway, BLACK);
    y += 18;

    s.text(x_label, y, "DNS Servers:", SHADOW);
    s.text(x_val, y, &st.ethernet.dns, BLACK);

    button(s, l.renew_btn, "Renew IP (DHCP)", true);
    button(s, l.eth_disconnect_btn, "Disconnect", st.ethernet.connected);
    button(s, l.diagnose_btn, "Diagnose...", true);

    if let Some(diag) = &st.eth_diagnostic {
        let col = if diag.starts_with("Diagnostic OK") {
            0x008000
        } else if diag.starts_with("Problem") {
            RED
        } else {
            BLACK
        };
        // Up to two lines, the last where a one-line message goes.
        let room = ((l.panel.w - 32) / ferro_gfx::GLYPH_W).max(1) as usize;
        let lines = status_lines(diag, room, 2);
        let top = l.panel.bottom() - 24 - (lines.len() as i32 - 1) * 12;
        for (i, line) in lines.iter().enumerate() {
            s.text(l.panel.x + 16, top + i as i32 * 12, line, col);
        }
    } else {
        s.text(l.panel.x + 16, l.panel.bottom() - 24, "Automatic DHCP configuration via NetworkManager.", SHADOW);
    }
}

/// `text` in at most `max` lines of `room` letters, by words; the last cut
/// short if it's still too long.
fn status_lines(text: &str, room: usize, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if lines.len() + 1 == max || rest.chars().count() <= room {
            lines.push(ellipsize(rest, room));
            break;
        }
        // The last space that still fits; a word longer than the line is cut.
        let cut = rest.match_indices(' ').map(|(i, _)| i).take_while(|&i| rest[..i].chars().count() <= room).last();
        match cut {
            Some(i) => {
                lines.push(rest[..i].to_owned());
                rest = rest[i + 1..].trim_start();
            }
            None => {
                lines.push(ellipsize(rest, room));
                break;
            }
        }
    }
    lines
}

fn draw_vpn_tor(s: &mut Surface, st: &State, l: &Layout) {
    use color::{BLACK, SHADOW};
    s.group_box(l.vpn_box, "WireGuard VPN");
    draw_checkbox(s, l.vpn_chk, st.vpn_enabled, "Send all traffic through WireGuard");
    button(s, l.vpn_import, "Add config...", true);
    s.text(l.vpn_box.x + 14, l.vpn_box.y + 48, "Status:", SHADOW);
    // Up to two lines, so a reason (why it couldn't connect) fits.
    let room = ((l.vpn_box.right() - 14 - (l.vpn_box.x + 74)) / ferro_gfx::GLYPH_W).max(1) as usize;
    for (i, line) in status_lines(&st.vpn_status, room, 2).iter().enumerate() {
        s.text(l.vpn_box.x + 74, l.vpn_box.y + 48 + i as i32 * 12, line, BLACK);
    }
    // The configs, the one in use marked.
    s.fill(l.vpn_list, color::WHITE);
    s.bevel(l.vpn_list, Bevel::Sunken);
    if st.vpn_configs.is_empty() {
        s.text(l.vpn_list.x + 6, l.vpn_list.y + 4, "No configs yet: Add config...", SHADOW);
    }
    let shown = st.vpn_configs.len().min(VPN_ROWS);
    for (i, id) in st.vpn_configs.iter().take(VPN_ROWS).enumerate() {
        let row = Rect::new(l.vpn_list.x + 2, l.vpn_list.y + 2 + i as i32 * VPN_ROW_H, l.vpn_list.w - 4, VPN_ROW_H);
        let sel = st.selected_vpn == Some(i);
        if sel {
            s.fill(row, color::NAVY);
        }
        let fg = if sel { color::WHITE } else { BLACK };
        let in_use = st.vpn_in_use.as_deref() == Some(id.as_str());
        let label = format!("{}. {}{}", i + 1, vpn_name(id), if in_use { "  (in use)" } else { "" });
        let room = ((row.w - 12) / ferro_gfx::GLYPH_W).max(1) as usize;
        if in_use {
            s.text_bold(row.x + 4, row.y + (VPN_ROW_H - GLYPH_H) / 2, &ellipsize(&label, room), fg);
        } else {
            s.text(row.x + 4, row.y + (VPN_ROW_H - GLYPH_H) / 2, &ellipsize(&label, room), fg);
        }
    }
    let picked = st.selected_vpn.is_some_and(|i| i < shown);
    button(s, l.vpn_use, "Use", picked);
    button(s, l.vpn_remove, "Remove", picked);
    let room = ((l.vpn_box.w - 28) / ferro_gfx::GLYPH_W).max(1) as usize;
    s.text(l.vpn_box.x + 14, l.vpn_list.bottom() + 6, &ellipsize(&vpn_note(st.vpn_configs.len()), room), SHADOW);

    s.group_box(l.tor_box, "Tor (Onion Routing)");
    draw_checkbox(s, l.tor_chk, st.tor_enabled, "Route all applications through Tor");
    draw_checkbox(s, l.tor_bridges_chk, st.tor_bridges, "Use Pluggable Transports (obfs4 / Snowflake)");
    s.text(l.tor_box.x + 14, l.tor_box.y + 78, "Status:", SHADOW);
    s.text(l.tor_box.x + 74, l.tor_box.y + 78, &st.tor_status, BLACK);
    s.text(l.tor_box.x + 14, l.tor_box.y + 96, "SOCKS Proxy: 127.0.0.1:9150 | DNS: 127.0.0.1:9053", SHADOW);
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
    button(s, l.reset_wifi_btn, "Reset Wi-Fi", true);
    draw_checkbox(s, l.portal_chk, st.portal.is_some(), "Allow this network's sign-in page");
    let hint =
        if st.tor_enabled { "Sign-in pages can't load through Tor: switch it off." } else { "While ticked, lookups on this network aren't private." };
    s.text(l.portal_chk.x + 22, l.portal_chk.y + 20, hint, color::SHADOW);

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
    fn long_vpn_status_in_two_lines() {
        assert_eq!(status_lines("Connected (1.2.3.4:51820)", 60, 3), ["Connected (1.2.3.4:51820)"]);
        let text = "No answer from the VPN server (203.0.113.7:51820). Is this config also in use on another PC? Each needs its own.";
        let lines = status_lines(text, 46, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.chars().count() <= 46));
        // Nothing lost: it all fits in three.
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn layout_tabs_and_buttons() {
        let st = State::default();
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.tabs[0].x + 5, l.tabs[0].y + 5), Hit::Tab(0));
        assert_eq!(hit(W, H, &st, l.tabs[1].x + 5, l.tabs[1].y + 5), Hit::Tab(1));
        assert_eq!(hit(W, H, &st, l.tabs[2].x + 5, l.tabs[2].y + 5), Hit::Tab(2));
        assert_eq!(hit(W, H, &st, l.tabs[3].x + 5, l.tabs[3].y + 5), Hit::Tab(3));
        assert_eq!(hit(W, H, &st, l.ok_btn.x + 5, l.ok_btn.y + 5), Hit::Ok);
        assert_eq!(hit(W, H, &st, l.cancel_btn.x + 5, l.cancel_btn.y + 5), Hit::Cancel);
        assert_eq!(hit(W, H, &st, l.apply_btn.x + 5, l.apply_btn.y + 5), Hit::Apply);
    }

    #[test]
    fn ethernet_tab_hits() {
        let st = State { tab: 0, ..State::default() };
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.renew_btn.x + 5, l.renew_btn.y + 5), Hit::RenewDhcp);
        assert_eq!(hit(W, H, &st, l.eth_disconnect_btn.x + 5, l.eth_disconnect_btn.y + 5), Hit::DisconnectEth);
        assert_eq!(hit(W, H, &st, l.diagnose_btn.x + 5, l.diagnose_btn.y + 5), Hit::DiagnoseEth);
    }

    #[test]
    fn vpn_and_tor_toggle_hits() {
        let st = State { tab: 2, ..State::default() };
        let l = layout(W, H, &st);
        assert_eq!(hit(W, H, &st, l.vpn_chk.x + 5, l.vpn_chk.y + 5), Hit::ToggleVpn);
        assert_eq!(hit(W, H, &st, l.tor_chk.x + 5, l.tor_chk.y + 5), Hit::ToggleTor);
        assert_eq!(hit(W, H, &st, l.tor_bridges_chk.x + 5, l.tor_bridges_chk.y + 5), Hit::ToggleTorBridges);
        assert_eq!(hit(W, H, &st, l.vpn_use.x + 5, l.vpn_use.y + 5), Hit::UseVpn);
        assert_eq!(hit(W, H, &st, l.vpn_remove.x + 5, l.vpn_remove.y + 5), Hit::RemoveVpn);
        assert_eq!(hit(W, H, &st, l.vpn_list.x + 5, l.vpn_list.y + 5), Hit::Nothing, "no configs: no rows");
        let st2 = State { tab: 2, vpn_configs: vec!["01-home".into(), "02-mullvad".into()], ..State::default() };
        assert_eq!(hit(W, H, &st2, l.vpn_list.x + 5, l.vpn_list.y + 2 + VPN_ROW_H + 3), Hit::SelectVpn(1));
        assert!(l.tor_box.bottom() <= l.panel.bottom() - 8, "the Tor box fits");
        assert_eq!((vpn_name("02-mullvad-se"), vpn_name("odd")), ("mullvad-se", "odd"));
        let room = ((l.vpn_box.w - 28) / ferro_gfx::GLYPH_W) as usize;
        for n in [0, 9, 95] {
            assert!(vpn_note(n).chars().count() <= room, "{} doesn't fit in {room}", vpn_note(n));
        }
    }

    #[test]
    fn sign_in_page_box_sits_below_the_networks() {
        let st = State { tab: 1, ..State::default() };
        let l = layout(W, H, &st);
        assert!(l.portal_chk.y >= l.wifi_list.bottom());
        // The hint under it stays above the status line.
        assert!(l.portal_chk.y + 20 + GLYPH_H < l.panel.bottom() - 28);
        assert_eq!(hit(W, H, &st, l.portal_chk.x + 5, l.portal_chk.y + 5), Hit::TogglePortal);
        assert!(controls(W, H, &st).contains(&l.portal_chk));
    }

    #[test]
    fn reset_wifi_sits_under_disconnect() {
        let st = State { tab: 1, ..State::default() };
        let l = layout(W, H, &st);
        assert!(l.reset_wifi_btn.y >= l.disconnect_btn.bottom());
        assert!(l.reset_wifi_btn.bottom() <= l.wifi_list.bottom());
        assert_eq!(hit(W, H, &st, l.reset_wifi_btn.x + 5, l.reset_wifi_btn.y + 5), Hit::ResetWifi);
        assert!(controls(W, H, &st).contains(&l.reset_wifi_btn));
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
        for tab in 0..4 {
            let st = State { tab, ..State::default() };
            let mut s = Surface::new(W as usize, H as usize);
            draw(&mut s, W, H, &st);
        }
    }
}
