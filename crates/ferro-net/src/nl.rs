//! Minimal netlink: just what the VPN and Tor routing need. Interfaces,
//! addresses, routes and per-user routing rules (rtnetlink), and WireGuard
//! device setup (generic netlink).

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const NETLINK_ROUTE: i32 = 0;
const NETLINK_GENERIC: i32 = 16;

const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_DUMP: u16 = 0x300;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLA_F_NESTED: u16 = 0x8000;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_NEWRULE: u16 = 32;
const RTM_DELRULE: u16 = 33;

const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_LINKINFO: u16 = 18;
const IFLA_INFO_KIND: u16 = 1;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_F_NODAD: u8 = 0x02;
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_TABLE: u16 = 15;
const FRA_PRIORITY: u16 = 6;
const FRA_TABLE: u16 = 15;
const FRA_UID_RANGE: u16 = 20;
const FR_ACT_TO_TBL: u8 = 1;

pub const RT_TABLE_MAIN: u32 = 254;

const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;

/// One netlink message under construction.
pub struct Msg {
    buf: Vec<u8>,
    nests: Vec<usize>,
}

impl Msg {
    pub fn new(kind: u16, flags: u16) -> Self {
        let mut buf = vec![0u8; 16];
        buf[4..6].copy_from_slice(&kind.to_ne_bytes());
        buf[6..8].copy_from_slice(&(flags | NLM_F_REQUEST).to_ne_bytes());
        Self { buf, nests: Vec::new() }
    }

    /// Fixed header that follows nlmsghdr (ifinfomsg, rtmsg, genlmsghdr...).
    pub fn header(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self.pad();
        self
    }

    fn pad(&mut self) {
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
    }

    pub fn attr(&mut self, kind: u16, data: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
        self.buf.extend_from_slice(&kind.to_ne_bytes());
        self.buf.extend_from_slice(data);
        self.pad();
        self
    }

    pub fn u8(&mut self, kind: u16, v: u8) -> &mut Self {
        self.attr(kind, &[v])
    }

    pub fn u16(&mut self, kind: u16, v: u16) -> &mut Self {
        self.attr(kind, &v.to_ne_bytes())
    }

    pub fn u32(&mut self, kind: u16, v: u32) -> &mut Self {
        self.attr(kind, &v.to_ne_bytes())
    }

    /// A NUL-terminated string attribute.
    pub fn str(&mut self, kind: u16, s: &str) -> &mut Self {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        self.attr(kind, &v)
    }

    pub fn begin(&mut self, kind: u16) -> &mut Self {
        self.nests.push(self.buf.len());
        self.attr(kind | NLA_F_NESTED, &[])
    }

    pub fn end(&mut self) -> &mut Self {
        if let Some(start) = self.nests.pop() {
            let len = (self.buf.len() - start) as u16;
            self.buf[start..start + 2].copy_from_slice(&len.to_ne_bytes());
        }
        self
    }

    fn finish(mut self, seq: u32) -> Vec<u8> {
        let len = self.buf.len() as u32;
        self.buf[0..4].copy_from_slice(&len.to_ne_bytes());
        self.buf[8..12].copy_from_slice(&seq.to_ne_bytes());
        self.buf
    }
}

/// Attributes in `b` as (type, payload), nested flag stripped.
pub fn attrs(mut b: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    while b.len() >= 4 {
        let len = u16::from_ne_bytes([b[0], b[1]]) as usize;
        let kind = u16::from_ne_bytes([b[2], b[3]]) & 0x3FFF;
        if len < 4 || len > b.len() {
            break;
        }
        out.push((kind, &b[4..len]));
        b = &b[len.next_multiple_of(4).min(b.len())..];
    }
    out
}

pub struct Socket {
    fd: OwnedFd,
    seq: u32,
}

impl Socket {
    pub fn route() -> io::Result<Self> {
        Self::open(NETLINK_ROUTE)
    }

    pub fn generic() -> io::Result<Self> {
        Self::open(NETLINK_GENERIC)
    }

    fn open(proto: i32) -> io::Result<Self> {
        // SAFETY: plain socket/bind calls with a zeroed sockaddr_nl.
        unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, proto);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let mut addr: libc::sockaddr_nl = std::mem::zeroed();
            addr.nl_family = libc::AF_NETLINK as u16;
            let tv = libc::timeval { tv_sec: 5, tv_usec: 0 };
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&tv as *const libc::timeval).cast(),
                std::mem::size_of::<libc::timeval>() as u32,
            );
            if libc::bind(fd.as_raw_fd(), (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { fd, seq: 0 })
        }
    }

    /// Sends `msg` and returns the payloads of the replies (after nlmsghdr),
    /// or the kernel's error.
    pub fn request(&mut self, mut msg: Msg) -> io::Result<Vec<Vec<u8>>> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        // Always ask for an acknowledgement, so errors and the end are explicit.
        let flags = u16::from_ne_bytes([msg.buf[6], msg.buf[7]]) | NLM_F_ACK;
        msg.buf[6..8].copy_from_slice(&flags.to_ne_bytes());
        let data = msg.finish(seq);
        // SAFETY: sending our own buffer.
        if unsafe { libc::send(self.fd.as_raw_fd(), data.as_ptr().cast(), data.len(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            // SAFETY: receiving into our buffer.
            let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut b = &buf[..n as usize];
            while b.len() >= 16 {
                let len = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize;
                let kind = u16::from_ne_bytes([b[4], b[5]]);
                let rseq = u32::from_ne_bytes([b[8], b[9], b[10], b[11]]);
                if len < 16 || len > b.len() {
                    break;
                }
                let payload = &b[16..len];
                if rseq == seq {
                    match kind {
                        NLMSG_ERROR => {
                            let errno = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                            return if errno == 0 { Ok(out) } else { Err(io::Error::from_raw_os_error(-errno)) };
                        }
                        NLMSG_DONE => return Ok(out),
                        _ => out.push(payload.to_vec()),
                    }
                }
                b = &b[len.next_multiple_of(4).min(b.len())..];
            }
        }
    }
}

/// Treats "already there" / "already gone" as success.
fn tolerate(r: io::Result<Vec<Vec<u8>>>, ok: &[i32]) -> io::Result<()> {
    match r {
        Err(e) if e.raw_os_error().is_some_and(|c| ok.contains(&c)) => Ok(()),
        r => r.map(|_| ()),
    }
}

pub fn ifindex(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: valid C string.
    let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
    (i != 0).then_some(i)
}

fn family(ip: &IpAddr) -> u8 {
    if ip.is_ipv4() {
        libc::AF_INET as u8
    } else {
        libc::AF_INET6 as u8
    }
}

fn octets(ip: &IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(a) => a.octets().to_vec(),
        IpAddr::V6(a) => a.octets().to_vec(),
    }
}

// ---- links and addresses ---------------------------------------------------------------

/// Creates a virtual interface of `kind` (e.g. "wireguard").
pub fn link_add(s: &mut Socket, name: &str, kind: &str, mtu: u32) -> io::Result<()> {
    let mut m = Msg::new(RTM_NEWLINK, NLM_F_CREATE | NLM_F_EXCL);
    m.header(&[0u8; 16]).str(IFLA_IFNAME, name).u32(IFLA_MTU, mtu).begin(IFLA_LINKINFO).str(IFLA_INFO_KIND, kind).end();
    tolerate(s.request(m), &[libc::EEXIST])
}

pub fn link_del(s: &mut Socket, name: &str) -> io::Result<()> {
    let mut m = Msg::new(RTM_DELLINK, 0);
    m.header(&[0u8; 16]).str(IFLA_IFNAME, name);
    tolerate(s.request(m), &[libc::ENODEV])
}

pub fn addr_add(s: &mut Socket, ifindex: u32, ip: IpAddr, prefix: u8) -> io::Result<()> {
    let mut h = vec![family(&ip), prefix, if ip.is_ipv6() { IFA_F_NODAD } else { 0 }, 0];
    h.extend_from_slice(&ifindex.to_ne_bytes());
    let mut m = Msg::new(RTM_NEWADDR, NLM_F_CREATE | NLM_F_REPLACE);
    m.header(&h).attr(IFA_LOCAL, &octets(&ip)).attr(IFA_ADDRESS, &octets(&ip));
    tolerate(s.request(m), &[libc::EEXIST])
}

// ---- routes and rules ------------------------------------------------------------------

/// Where a route sends packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Gateway(IpAddr, u32),
    Device(u32),
}

fn route_msg(kind: u16, flags: u16, dst: IpAddr, prefix: u8, via: Option<Via>, table: u32) -> Msg {
    let add = kind == RTM_NEWROUTE;
    let scope = match via {
        Some(Via::Device(_)) if add => 253, // RT_SCOPE_LINK
        _ if add => 0,                      // RT_SCOPE_UNIVERSE
        _ => 255,                           // RT_SCOPE_NOWHERE: match any on delete
    };
    let table8 = if table < 256 { table as u8 } else { 0 };
    let h = [family(&dst), prefix, 0, 0, table8, if add { 4 } else { 0 }, scope, 1, 0, 0, 0, 0];
    let mut m = Msg::new(kind, flags);
    m.header(&h).u32(RTA_TABLE, table);
    if prefix > 0 {
        m.attr(RTA_DST, &octets(&dst));
    }
    match via {
        Some(Via::Gateway(gw, oif)) => {
            m.attr(RTA_GATEWAY, &octets(&gw)).u32(RTA_OIF, oif);
        }
        Some(Via::Device(oif)) => {
            m.u32(RTA_OIF, oif);
        }
        None => {}
    }
    m
}

pub fn route_add(s: &mut Socket, dst: IpAddr, prefix: u8, via: Via, table: u32) -> io::Result<()> {
    tolerate(s.request(route_msg(RTM_NEWROUTE, NLM_F_CREATE | NLM_F_REPLACE, dst, prefix, Some(via), table)), &[libc::EEXIST])
}

pub fn route_del(s: &mut Socket, dst: IpAddr, prefix: u8, table: u32) -> io::Result<()> {
    tolerate(s.request(route_msg(RTM_DELROUTE, 0, dst, prefix, None, table)), &[libc::ESRCH, libc::ENOENT])
}

/// Removes every default route of `ipv6`'s family from `table`.
pub fn defaults_clear(s: &mut Socket, ipv6: bool, table: u32) {
    let any = if ipv6 { IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::UNSPECIFIED) };
    for _ in 0..16 {
        if s.request(route_msg(RTM_DELROUTE, 0, any, 0, None, table)).is_err() {
            break;
        }
    }
}

fn rule_msg(kind: u16, flags: u16, ipv6: bool, uid: u32, table: u32, priority: u32) -> Msg {
    let fam = if ipv6 { libc::AF_INET6 } else { libc::AF_INET } as u8;
    let mut range = uid.to_ne_bytes().to_vec();
    range.extend_from_slice(&uid.to_ne_bytes());
    let mut m = Msg::new(kind, flags);
    m.header(&[fam, 0, 0, 0, table as u8, 0, 0, FR_ACT_TO_TBL, 0, 0, 0, 0])
        .u32(FRA_PRIORITY, priority)
        .u32(FRA_TABLE, table)
        .attr(FRA_UID_RANGE, &range);
    m
}

/// Routes `uid`'s traffic with `table` (consulted before the main table).
pub fn uid_rule(s: &mut Socket, add: bool, ipv6: bool, uid: u32, table: u32, priority: u32) -> io::Result<()> {
    if add {
        tolerate(s.request(rule_msg(RTM_NEWRULE, NLM_F_CREATE | NLM_F_EXCL, ipv6, uid, table, priority)), &[libc::EEXIST])
    } else {
        tolerate(s.request(rule_msg(RTM_DELRULE, 0, ipv6, uid, table, priority)), &[libc::ENOENT, libc::ESRCH])
    }
}

// ---- generic netlink: WireGuard -------------------------------------------------------

pub fn genl_family(s: &mut Socket, name: &str) -> io::Result<u16> {
    let mut m = Msg::new(GENL_ID_CTRL, 0);
    m.header(&[CTRL_CMD_GETFAMILY, 1, 0, 0]).str(CTRL_ATTR_FAMILY_NAME, name);
    for reply in s.request(m)? {
        for (k, v) in attrs(reply.get(4..).unwrap_or(&[])) {
            if k == CTRL_ATTR_FAMILY_ID && v.len() >= 2 {
                return Ok(u16::from_ne_bytes([v[0], v[1]]));
            }
        }
    }
    Err(io::Error::other(format!("no {name} netlink family")))
}

const WG_CMD_GET_DEVICE: u8 = 0;
const WG_CMD_SET_DEVICE: u8 = 1;
const WGDEVICE_A_IFNAME: u16 = 2;
const WGDEVICE_A_PRIVATE_KEY: u16 = 3;
const WGDEVICE_A_FLAGS: u16 = 5;
const WGDEVICE_A_LISTEN_PORT: u16 = 6;
const WGDEVICE_A_PEERS: u16 = 8;
const WGDEVICE_F_REPLACE_PEERS: u32 = 1;
const WGPEER_A_PUBLIC_KEY: u16 = 1;
const WGPEER_A_PRESHARED_KEY: u16 = 2;
const WGPEER_A_FLAGS: u16 = 3;
const WGPEER_A_ENDPOINT: u16 = 4;
const WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL: u16 = 5;
const WGPEER_A_LAST_HANDSHAKE_TIME: u16 = 6;
const WGPEER_A_RX_BYTES: u16 = 7;
const WGPEER_A_TX_BYTES: u16 = 8;
const WGPEER_A_ALLOWEDIPS: u16 = 9;
const WGPEER_F_REPLACE_ALLOWEDIPS: u32 = 2;
const WGALLOWEDIP_A_FAMILY: u16 = 1;
const WGALLOWEDIP_A_IPADDR: u16 = 2;
const WGALLOWEDIP_A_CIDR_MASK: u16 = 3;

fn sockaddr(a: &SocketAddr) -> Vec<u8> {
    let mut v = Vec::new();
    match a {
        SocketAddr::V4(a) => {
            v.extend_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
            v.extend_from_slice(&a.port().to_be_bytes());
            v.extend_from_slice(&a.ip().octets());
            v.extend_from_slice(&[0; 8]);
        }
        SocketAddr::V6(a) => {
            v.extend_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes());
            v.extend_from_slice(&a.port().to_be_bytes());
            v.extend_from_slice(&[0; 4]);
            v.extend_from_slice(&a.ip().octets());
            v.extend_from_slice(&[0; 4]);
        }
    }
    v
}

/// Configures WireGuard device `ifname` from `cfg`, replacing any peers.
pub fn wg_set(s: &mut Socket, fam: u16, ifname: &str, cfg: &crate::wg::Config) -> io::Result<()> {
    let mut m = Msg::new(fam, 0);
    m.header(&[WG_CMD_SET_DEVICE, 1, 0, 0])
        .str(WGDEVICE_A_IFNAME, ifname)
        .attr(WGDEVICE_A_PRIVATE_KEY, &cfg.private_key)
        .u32(WGDEVICE_A_FLAGS, WGDEVICE_F_REPLACE_PEERS);
    if let Some(p) = cfg.listen_port {
        m.u16(WGDEVICE_A_LISTEN_PORT, p);
    }
    m.begin(WGDEVICE_A_PEERS);
    for p in &cfg.peers {
        m.begin(0)
            .attr(WGPEER_A_PUBLIC_KEY, &p.public_key)
            .u32(WGPEER_A_FLAGS, WGPEER_F_REPLACE_ALLOWEDIPS)
            .attr(WGPEER_A_ENDPOINT, &sockaddr(&p.endpoint));
        if let Some(k) = &p.preshared_key {
            m.attr(WGPEER_A_PRESHARED_KEY, k);
        }
        if let Some(k) = p.keepalive {
            m.u16(WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL, k);
        }
        m.begin(WGPEER_A_ALLOWEDIPS);
        for (ip, cidr) in &p.allowed_ips {
            m.begin(0).u16(WGALLOWEDIP_A_FAMILY, family(ip) as u16).attr(WGALLOWEDIP_A_IPADDR, &octets(ip)).u8(WGALLOWEDIP_A_CIDR_MASK, *cidr).end();
        }
        m.end().end();
    }
    m.end();
    s.request(m).map(|_| ())
}

/// A peer's live state.
#[derive(Clone, Copy, Debug, Default)]
pub struct PeerStatus {
    /// Unix seconds of the last completed handshake (0 = never).
    pub handshake: u64,
    pub rx: u64,
    pub tx: u64,
}

pub fn wg_status(s: &mut Socket, fam: u16, ifname: &str) -> io::Result<Vec<PeerStatus>> {
    let mut m = Msg::new(fam, NLM_F_DUMP);
    m.header(&[WG_CMD_GET_DEVICE, 1, 0, 0]).str(WGDEVICE_A_IFNAME, ifname);
    let mut out = Vec::new();
    for reply in s.request(m)? {
        for (k, peers) in attrs(reply.get(4..).unwrap_or(&[])) {
            if k != WGDEVICE_A_PEERS {
                continue;
            }
            for (_, peer) in attrs(peers) {
                let mut st = PeerStatus::default();
                for (pk, v) in attrs(peer) {
                    let u64_at = |v: &[u8]| v.get(..8).map_or(0, |b| u64::from_ne_bytes(b.try_into().unwrap()));
                    match pk {
                        WGPEER_A_LAST_HANDSHAKE_TIME => st.handshake = u64_at(v),
                        WGPEER_A_RX_BYTES => st.rx = u64_at(v),
                        WGPEER_A_TX_BYTES => st.tx = u64_at(v),
                        _ => {}
                    }
                }
                out.push(st);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_attributes_round_trip() {
        let mut m = Msg::new(1, 0);
        m.header(&[9, 9, 9, 9]).str(2, "wg0").begin(8).begin(0).u16(5, 25).end().end();
        let buf = m.finish(7);
        assert_eq!(u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize, buf.len());
        let top = attrs(&buf[20..]);
        assert_eq!(top[0], (2, &b"wg0\0"[..]));
        assert_eq!(top[1].0, 8);
        let peer = attrs(top[1].1);
        assert_eq!(attrs(peer[0].1), vec![(5, &25u16.to_ne_bytes()[..])]);
    }
}
