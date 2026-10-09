//! Who may reach the internet, and how. Leak-proof by construction: there is
//! only ever one default route, and it points where the user's choice says.
//!
//! * Plain: default route via the network's gateway.
//! * VPN: default route into the WireGuard tunnel; only the VPN server
//!   itself is reached directly. If the tunnel can't be set up, there is no
//!   default route at all, so nothing leaks.
//! * Tor: the main table has no default route. A per-user rule gives the Tor
//!   service (and only it) a table that has one, via the tunnel when the VPN
//!   is on. Everything else can reach the local network but not the internet,
//!   unless it goes through Tor.
//!
//! IPv6 on the physical interface is switched off while the VPN or Tor is on:
//! router advertisements would otherwise hand it a route around both.

use crate::nl::{self, Socket, Via, RT_TABLE_MAIN};
use crate::{wg, TOR_UID};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

pub const WG_IF: &str = "wg0";
const TOR_TABLE: u32 = 100;
const TOR_RULE_PRIORITY: u32 = 100;

/// What the user wants, and what the network offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Physical interface and its gateway, while online and logged on.
    pub uplink: Option<(String, Ipv4Addr)>,
    /// `None` = VPN off; `Some(Err)` = on but the profile is unusable.
    pub vpn: Option<Result<wg::Config, String>>,
    pub tor: bool,
}

#[derive(Default)]
pub struct Router {
    applied: Option<Plan>,
    tunnel: Option<wg::Config>,
    tunnel_error: Option<String>,
    endpoint_routes: Vec<Ipv4Addr>,
    uplink_v6_off: Option<String>,
    wg_family: Option<u16>,
    /// Bytes received at the last change, when, and bytes sent by then:
    /// traffic going out with nothing coming back means the server is gone.
    rx_seen: (u64, Option<std::time::Instant>, u64),
}

impl Router {
    /// Brings routing in line with `plan`. Returns true if anything changed.
    /// `force` re-applies after DHCP touched the interface.
    pub fn reconcile(&mut self, plan: &Plan, force: bool) -> bool {
        if !force && self.applied.as_ref() == Some(plan) {
            return false;
        }
        if let Err(e) = self.apply(plan) {
            eprintln!("ferro-net: routing: {e}");
        }
        self.applied = Some(plan.clone());
        true
    }

    fn apply(&mut self, plan: &Plan) -> io::Result<()> {
        let mut rt = Socket::route()?;
        let uplink = plan.uplink.as_ref().and_then(|(name, gw)| Some((name.clone(), *gw, nl::ifindex(name)?)));

        // 1. The tunnel.
        let wanted = match &plan.vpn {
            Some(Ok(c)) => Some(c),
            _ => None,
        };
        if self.tunnel.as_ref() != wanted || (wanted.is_some() && nl::ifindex(WG_IF).is_none()) {
            self.tunnel = None;
            self.tunnel_error = None;
            let _ = nl::link_del(&mut rt, WG_IF);
            if let Some(cfg) = wanted {
                match self.bring_up(&mut rt, cfg) {
                    Ok(()) => {
                        self.tunnel = Some(cfg.clone());
                        eprintln!("ferro-net: VPN tunnel to {} set up", cfg.peers[0].endpoint);
                    }
                    Err(e) => {
                        let _ = nl::link_del(&mut rt, WG_IF);
                        self.tunnel_error = Some(format!("couldn't set up the tunnel: {e}"));
                    }
                }
            }
        }
        if let Some(Err(e)) = &plan.vpn {
            self.tunnel_error = Some(e.clone());
        }
        let wg_index = self.tunnel.as_ref().and_then(|_| nl::ifindex(WG_IF));

        // 2. IPv6 on the physical interface.
        let v6_off = plan.vpn.is_some() || plan.tor;
        let want_off = if v6_off { plan.uplink.as_ref().map(|(n, _)| n.clone()) } else { None };
        if self.uplink_v6_off != want_off {
            if let Some(old) = self.uplink_v6_off.take() {
                set_ipv6(&old, true);
            }
            if let Some(new) = &want_off {
                set_ipv6(new, false);
            }
            self.uplink_v6_off = want_off;
        }

        // 3. Routes.
        nl::defaults_clear(&mut rt, false, RT_TABLE_MAIN);
        nl::defaults_clear(&mut rt, false, TOR_TABLE);
        if v6_off {
            nl::defaults_clear(&mut rt, true, RT_TABLE_MAIN);
        }
        for ep in self.endpoint_routes.drain(..) {
            let _ = nl::route_del(&mut rt, IpAddr::V4(ep), 32, RT_TABLE_MAIN);
        }
        if let (Some(cfg), Some((_, gw, idx))) = (&self.tunnel, &uplink) {
            // The VPN server is the one host reached directly.
            for p in &cfg.peers {
                if let IpAddr::V4(ep) = p.endpoint.ip() {
                    nl::route_add(&mut rt, IpAddr::V4(ep), 32, Via::Gateway(IpAddr::V4(*gw), *idx), RT_TABLE_MAIN)?;
                    self.endpoint_routes.push(ep);
                }
            }
        }
        let egress = match (&plan.vpn, &uplink) {
            (Some(_), Some(_)) => wg_index.map(Via::Device),
            (None, Some((_, gw, idx))) => Some(Via::Gateway(IpAddr::V4(*gw), *idx)),
            (_, None) => None,
        };
        let table = if plan.tor { TOR_TABLE } else { RT_TABLE_MAIN };
        if let Some(via) = egress {
            nl::route_add(&mut rt, IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0, via, table)?;
            // IPv6 through the tunnel, when the profile carries it.
            if let (Some(cfg), Some(idx), false) = (&self.tunnel, wg_index, plan.tor) {
                if cfg.addresses.iter().any(|(ip, _)| ip.is_ipv6()) {
                    nl::route_add(&mut rt, IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0, Via::Device(idx), RT_TABLE_MAIN)?;
                }
            }
        }
        nl::uid_rule(&mut rt, plan.tor, false, TOR_UID, TOR_TABLE, TOR_RULE_PRIORITY)?;
        // Strict reverse-path filtering checks replies against the main
        // table, which in Tor mode has no route out on purpose: it would drop
        // every answer from the Tor network. Strict otherwise.
        let rp = if plan.tor { "0" } else { "1" };
        let mut ifaces = vec!["all".to_owned()];
        ifaces.extend(plan.uplink.as_ref().map(|(n, _)| n.clone()));
        if wg_index.is_some() {
            ifaces.push(WG_IF.into());
        }
        for i in ifaces {
            let _ = std::fs::write(format!("/proc/sys/net/ipv4/conf/{i}/rp_filter"), rp);
        }
        Ok(())
    }

    fn bring_up(&mut self, rt: &mut Socket, cfg: &wg::Config) -> io::Result<()> {
        load_module("wireguard")?;
        nl::link_add(rt, WG_IF, "wireguard", cfg.mtu)?;
        let mut g = Socket::generic()?;
        let fam = match self.wg_family {
            Some(f) => f,
            None => *self.wg_family.insert(nl::genl_family(&mut g, "wireguard")?),
        };
        nl::wg_set(&mut g, fam, WG_IF, cfg)?;
        let idx = nl::ifindex(WG_IF).ok_or_else(|| io::Error::other("wg0 vanished"))?;
        for (ip, len) in &cfg.addresses {
            nl::addr_add(rt, idx, *ip, *len)?;
        }
        crate::netif::set_up(WG_IF, true)
    }

    /// One `key=value` per line, for the desktop and `VPN` command.
    pub fn status(&mut self, plan: &Plan) -> String {
        let mut out = String::new();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        match (&plan.vpn, &self.tunnel) {
            (None, _) => out.push_str("vpn=off\n"),
            (Some(_), Some(cfg)) => {
                let peer = Socket::generic()
                    .ok()
                    .zip(self.wg_family)
                    .and_then(|(mut g, f)| nl::wg_status(&mut g, f, WG_IF).ok())
                    .and_then(|v| v.first().copied());
                let p = peer.unwrap_or_default();
                if p.rx != self.rx_seen.0 || self.rx_seen.1.is_none() {
                    self.rx_seen = (p.rx, Some(std::time::Instant::now()), p.tx);
                }
                let silent = self.rx_seen.1.map_or(0, |t| t.elapsed().as_secs());
                // Keepalives (32 bytes, one-way) don't count; real traffic does.
                let stalled = p.tx.saturating_sub(self.rx_seen.2) > 512 && silent >= 20;
                let state = match (p.handshake > 0 && now.saturating_sub(p.handshake) < 180, stalled) {
                    (_, true) => "stalled",
                    (true, false) => "connected",
                    (false, false) => "connecting",
                };
                out += &format!("vpn={state}\nendpoint={}\n", cfg.peers[0].endpoint);
                if stalled {
                    out += &format!("silent={silent}\n");
                }
                if p.handshake > 0 {
                    out += &format!("handshake={}\n", now.saturating_sub(p.handshake));
                }
                out += &format!("rx={}\ntx={}\n", p.rx, p.tx);
            }
            (Some(_), None) => {
                let why = self.tunnel_error.clone().unwrap_or_else(|| "waiting for the network".into());
                out += &format!("vpn=blocked\ndetail={why}\n");
            }
        }
        out += &format!("tor={}\n", if plan.tor { "on" } else { "off" });
        out
    }
}

fn set_ipv6(iface: &str, enabled: bool) {
    let _ = std::fs::write(format!("/proc/sys/net/ipv6/conf/{iface}/disable_ipv6"), if enabled { "0" } else { "1" });
}

/// Loads a kernel module (and what it needs) on demand, so drivers for
/// features that are off cost no RAM.
pub fn load_module(name: &str) -> io::Result<()> {
    if Path::new("/sys/module").join(name).exists() {
        return Ok(());
    }
    let dir = std::fs::read_dir("/lib/modules")?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("modules.dep").exists())
        .ok_or_else(|| io::Error::other("no kernel modules"))?;
    let deps = std::fs::read_to_string(dir.join("modules.dep"))?;
    let lookup = |rel: &str| -> Vec<String> {
        deps.lines()
            .find_map(|l| l.strip_prefix(rel)?.strip_prefix(':'))
            .map(|d| d.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default()
    };
    let rel = deps
        .lines()
        .filter_map(|l| l.split_once(':').map(|(m, _)| m))
        .find(|m| m.rsplit('/').next().and_then(|f| f.split(".ko").next()) == Some(name))
        .ok_or_else(|| io::Error::other(format!("the {name} driver isn't installed")))?
        .to_owned();
    // modules.dep lists every dependency, deepest last: load from the end.
    for m in lookup(&rel).iter().rev().chain(std::iter::once(&rel)) {
        let path = dir.join(m);
        let file = std::fs::File::open(&path)?;
        // SAFETY: finit_module with an open fd, empty params and the
        // "kernel decompresses" flag (MODULE_INIT_COMPRESSED_FILE).
        let r = unsafe {
            use std::os::fd::AsRawFd;
            let compressed = [".zst", ".xz", ".gz"].iter().any(|x| m.ends_with(x));
            libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), c"".as_ptr(), if compressed { 4 } else { 0 })
        };
        if r != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EEXIST) {
                return Err(io::Error::other(format!("loading {m}: {e}")));
            }
        }
    }
    Ok(())
}
