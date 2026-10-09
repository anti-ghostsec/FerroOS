//! The "VPN and Tor" settings window. Both are off by default; the switches
//! are kept with the other remembered choices and enforced by ferro-system
//! and ferro-net, which report back through small status files.

use super::*;

const NP_VPN: usize = 0;
const NP_TOR: usize = 1;
const NP_CLOSE: usize = 2;

/// Live VPN and Tor state, from `/run/ferro/net-status` and `tor-status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetStatus {
    pub vpn: String,
    pub vpn_detail: String,
    pub endpoint: String,
    pub handshake_secs: Option<u64>,
    pub silent_secs: Option<u64>,
    pub tor: String,
    pub tor_detail: String,
}

impl NetStatus {
    pub fn parse(net: &str, tor: &str) -> Self {
        let get = |text: &str, k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap_or("").to_owned();
        Self {
            vpn: get(net, "vpn"),
            vpn_detail: get(net, "detail"),
            endpoint: get(net, "endpoint"),
            handshake_secs: get(net, "handshake").parse().ok(),
            silent_secs: get(net, "silent").parse().ok(),
            tor: get(tor, "state"),
            tor_detail: get(tor, "detail"),
        }
    }

    pub(crate) fn vpn_text(&self, on: bool) -> String {
        if !on {
            return "Off. To set it up, get your VPN provider's WireGuard file and run VPN IMPORT <file> in the Command Prompt.".into();
        }
        match self.vpn.as_str() {
            "connected" => match self.handshake_secs {
                Some(s) => format!("Connected to {} (last handshake {s} s ago).", self.endpoint),
                None => format!("Connected to {}.", self.endpoint),
            },
            "connecting" => format!("Connecting to {}... Nothing goes out until it answers.", self.endpoint),
            "stalled" => {
                format!("The VPN server {} hasn't answered for {} s. Nothing goes out meanwhile.", self.endpoint, self.silent_secs.unwrap_or(0))
            }
            "blocked" => format!("Internet blocked until this is fixed: {}", self.vpn_detail),
            _ => "Starting...".into(),
        }
    }

    pub(crate) fn tor_text(&self, on: bool, vpn: bool) -> String {
        if !on {
            return "Off.".into();
        }
        let path = if vpn { " through the VPN" } else { "" };
        match self.tor.as_str() {
            "ready" => format!("Connected to the Tor network{path}. Apps reach the internet only through Tor (SOCKS 127.0.0.1:9150)."),
            "error" => format!("Problem: {}", self.tor_detail),
            "starting" if !self.tor_detail.is_empty() => format!("Starting: {}{path}...", self.tor_detail),
            _ => "Starting...".into(),
        }
    }
}

impl Shell {
    pub fn open_network_privacy(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::NetworkPrivacy)) {
            self.open("VPN and Tor".into(), icons::NETWORK, (480, 330), Content::NetworkPrivacy);
        }
    }

    /// Tray marker while the VPN or Tor is on.
    pub(crate) fn tunnel_label(&self) -> Option<&'static str> {
        match (self.store.switches.vpn, self.store.switches.tor) {
            (true, true) => Some("VPN+Tor"),
            (true, false) => Some("VPN"),
            (false, true) => Some("Tor"),
            (false, false) => None,
        }
    }

    pub(crate) fn netpriv_items(&self, win: &Window) -> Vec<Rect> {
        let c = client_rect(win.rect);
        vec![
            Rect::new(c.x + 20, c.y + 52, c.w - 40, 16),
            Rect::new(c.x + 20, c.y + 162, c.w - 40, 16),
            Rect::new(c.right() - 83, c.bottom() - 32, 75, 23),
        ]
    }

    pub(crate) fn draw_network_privacy(&self, s: &mut Surface, win: &Window, items: &[Rect]) {
        let c = client_rect(win.rect);
        let sw = self.store.switches;
        let cols = ((c.w - 48) / 8) as usize;
        s.text(c.x + 10, c.y + 12, "Hide your traffic from the network you're on.", BLACK);

        s.group_box(Rect::new(c.x + 8, c.y + 32, c.w - 16, 104), "VPN (WireGuard)");
        crate::privacy::draw_checkbox(s, items[NP_VPN], sw.vpn, "Send all traffic through the VPN");
        for (k, line) in wrap(&self.info.net.vpn_text(sw.vpn), cols).iter().take(4).enumerate() {
            s.text(c.x + 24, c.y + 76 + k as i32 * 14, line, BLACK);
        }

        s.group_box(Rect::new(c.x + 8, c.y + 142, c.w - 16, 116), "Tor");
        crate::privacy::draw_checkbox(s, items[NP_TOR], sw.tor, "Only allow internet access through Tor");
        for (k, line) in wrap(&self.info.net.tor_text(sw.tor, sw.vpn), cols).iter().take(5).enumerate() {
            s.text(c.x + 24, c.y + 186 + k as i32 * 14, line, BLACK);
        }
        self.draw_push_button(s, items[NP_CLOSE], Hit::Item(win.id, NP_CLOSE), "Close");
    }

    pub(crate) fn netpriv_click(&mut self, id: u32, item: usize) -> Option<Action> {
        match item {
            NP_CLOSE => {
                self.close(id);
                None
            }
            NP_VPN | NP_TOR => {
                let s = &mut self.store.switches;
                if item == NP_VPN {
                    s.vpn = !s.vpn;
                } else {
                    s.tor = !s.tor;
                }
                // Until ferro-net reports back, don't show the old state.
                if item == NP_VPN {
                    self.info.net.vpn.clear();
                } else {
                    self.info.net.tor.clear();
                }
                self.save_store();
                self.push_switches();
                Some(Action::SwitchesChanged)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_both_files() {
        let st = NetStatus::parse("vpn=connected\nendpoint=1.2.3.4:51820\nhandshake=12\ntor=on\n", "state=ready\ndetail=connected\n");
        assert_eq!(st.vpn_text(true), "Connected to 1.2.3.4:51820 (last handshake 12 s ago).");
        assert!(st.tor_text(true, true).contains("through the VPN"));
        assert!(NetStatus::parse("vpn=blocked\ndetail=no VPN profile yet\n", "").vpn_text(true).contains("blocked"));
        assert_eq!(NetStatus::default().tor_text(false, false), "Off.");
        assert!(NetStatus::parse("vpn=stalled\nendpoint=1.2.3.4:5\nsilent=40\n", "").vpn_text(true).contains("40 s"));
    }
}
