//! Network interfaces: up/down, a random MAC per boot, and a minimal DHCP
//! client that sends nothing identifying (no hostname, no vendor ID).

use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::os::unix::io::AsRawFd;
use std::time::Duration;

const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
const SIOCSIFADDR: libc::c_ulong = 0x8916;
const SIOCSIFNETMASK: libc::c_ulong = 0x891C;
const SIOCSIFHWADDR: libc::c_ulong = 0x8924;

/// `struct ifreq`: 16-byte name + a 24-byte union.
struct IfReq([u8; 40]);

impl IfReq {
    fn new(name: &str) -> Self {
        let mut b = [0u8; 40];
        let n = name.len().min(15);
        b[..n].copy_from_slice(&name.as_bytes()[..n]);
        Self(b)
    }

    fn set_ipv4(&mut self, ip: Ipv4Addr) {
        self.0[16..18].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
        self.0[20..24].copy_from_slice(&ip.octets());
    }

    fn flags(&self) -> i16 {
        i16::from_ne_bytes([self.0[16], self.0[17]])
    }

    fn set_flags(&mut self, f: i16) {
        self.0[16..18].copy_from_slice(&f.to_ne_bytes());
    }
}

fn ioctl_sock(req: libc::c_ulong, ifr: &mut IfReq) -> io::Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")?; // any AF_INET socket carries these ioctls
                                              // SAFETY: `ifr` is a correctly sized struct ifreq.
    if unsafe { libc::ioctl(sock.as_raw_fd(), req as _, ifr.0.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Ethernet-like interfaces, excluding loopback.
pub fn interfaces() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "lo")
        .collect();
    v.sort();
    v
}

pub fn is_up(name: &str) -> bool {
    let mut ifr = IfReq::new(name);
    ioctl_sock(SIOCGIFFLAGS, &mut ifr).is_ok() && ifr.flags() & libc::IFF_UP as i16 != 0
}

pub fn set_up(name: &str, up: bool) -> io::Result<()> {
    let mut ifr = IfReq::new(name);
    ioctl_sock(SIOCGIFFLAGS, &mut ifr)?;
    let mut f = ifr.flags();
    if up {
        f |= (libc::IFF_UP | libc::IFF_RUNNING) as i16;
    } else {
        f &= !(libc::IFF_UP as i16);
    }
    ifr.set_flags(f);
    ioctl_sock(SIOCSIFFLAGS, &mut ifr)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: getrandom fills our buffer.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), N, 0) };
    b
}

/// A fresh, locally administered unicast MAC, so this machine can't be
/// recognised across networks or reboots by its hardware address.
pub fn randomize_mac(name: &str) -> io::Result<[u8; 6]> {
    let mut mac: [u8; 6] = random_bytes();
    mac[0] = (mac[0] & 0xFC) | 0x02;
    let _ = set_up(name, false); // the address can only change while down
    let mut ifr = IfReq::new(name);
    ifr.0[16..18].copy_from_slice(&1u16.to_ne_bytes()); // ARPHRD_ETHER
    ifr.0[18..24].copy_from_slice(&mac);
    ioctl_sock(SIOCSIFHWADDR, &mut ifr)?;
    Ok(mac)
}

#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub router: Option<Ipv4Addr>,
    pub seconds: u32,
}

/// Sets the leased address. The default route is `route`'s business: it
/// depends on the VPN and Tor settings.
pub fn configure(name: &str, lease: &Lease) -> io::Result<()> {
    let mut ifr = IfReq::new(name);
    ifr.set_ipv4(lease.ip);
    ioctl_sock(SIOCSIFADDR, &mut ifr)?;
    let mut ifr = IfReq::new(name);
    ifr.set_ipv4(lease.netmask);
    ioctl_sock(SIOCSIFNETMASK, &mut ifr)
}

// ---- DHCP (RFC 2131), the minimum ------------------------------------------------------

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

pub fn dhcp_packet(kind: u8, xid: u32, mac: &[u8; 6], requested: Option<(Ipv4Addr, Ipv4Addr)>) -> Vec<u8> {
    let mut p = vec![0u8; 240];
    p[0] = 1; // BOOTREQUEST
    p[1] = 1; // Ethernet
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10] = 0x80; // ask the server to broadcast its reply (we have no IP yet)
    p[28..34].copy_from_slice(mac);
    p[236..240].copy_from_slice(&[99, 130, 83, 99]); // magic cookie
    p.extend_from_slice(&[53, 1, kind]);
    if let Some((ip, server)) = requested {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip.octets());
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&server.octets());
    }
    // Ask only for what we use: netmask, router, lease time. Deliberately no
    // hostname (12), vendor class (60) or DNS servers (6): we use DoH.
    p.extend_from_slice(&[55, 3, 1, 3, 51, 255]);
    p
}

/// (message type, offered IP, server id, lease) from a reply to `xid`.
pub fn parse_reply(p: &[u8], xid: u32) -> Option<(u8, Lease, Option<Ipv4Addr>)> {
    if p.len() < 240 || p[0] != 2 || p[4..8] != xid.to_be_bytes() || p[236..240] != [99, 130, 83, 99] {
        return None;
    }
    let ip = Ipv4Addr::new(p[16], p[17], p[18], p[19]);
    let mut lease = Lease { ip, netmask: Ipv4Addr::new(255, 255, 255, 0), router: None, seconds: 3600 };
    let (mut kind, mut server) = (0, None);
    let mut i = 240;
    while i + 1 < p.len() {
        let (code, len) = (p[i], p[i + 1] as usize);
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let v = p.get(i + 2..i + 2 + len)?;
        let ip4 = || (len >= 4).then(|| Ipv4Addr::new(v[0], v[1], v[2], v[3]));
        match code {
            53 if len == 1 => kind = v[0],
            1 => lease.netmask = ip4()?,
            3 => lease.router = ip4(),
            51 if len == 4 => lease.seconds = u32::from_be_bytes([v[0], v[1], v[2], v[3]]),
            54 => server = ip4(),
            _ => {}
        }
        i += 2 + len;
    }
    Some((kind, lease, server))
}

/// DISCOVER -> OFFER -> REQUEST -> ACK on `iface`.
pub fn dhcp(iface: &str, mac: &[u8; 6]) -> io::Result<Lease> {
    let sock = UdpSocket::bind("0.0.0.0:68")?;
    sock.set_broadcast(true)?;
    let dev = iface.as_bytes();
    // SAFETY: SO_BINDTODEVICE takes the interface name bytes.
    if unsafe { libc::setsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, dev.as_ptr().cast(), dev.len() as u32) } != 0 {
        return Err(io::Error::last_os_error());
    }
    sock.set_read_timeout(Some(Duration::from_secs(3)))?;
    let xid = u32::from_ne_bytes(random_bytes());
    let mut buf = [0u8; 1500];
    for _attempt in 0..4 {
        sock.send_to(&dhcp_packet(DISCOVER, xid, mac, None), "255.255.255.255:67")?;
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let Some((OFFER, offer, Some(server))) = parse_reply(&buf[..n], xid) else { continue };
        sock.send_to(&dhcp_packet(REQUEST, xid, mac, Some((offer.ip, server))), "255.255.255.255:67")?;
        while let Ok((n, _)) = sock.recv_from(&mut buf) {
            match parse_reply(&buf[..n], xid) {
                Some((ACK, lease, _)) => return Ok(lease),
                Some((NAK, ..)) => break,
                _ => {}
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no DHCP server answered"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dhcp_round_trip() {
        let mac = [2, 0, 0, 0, 0, 1];
        let req = dhcp_packet(DISCOVER, 7, &mac, None);
        assert!(!req.windows(2).any(|w| w == [12, 5]), "no hostname option");
        let mut reply = req.clone();
        reply[0] = 2;
        reply[16..20].copy_from_slice(&[10, 0, 2, 15]);
        reply.truncate(240);
        reply.extend_from_slice(&[53, 1, ACK, 1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 2, 2, 51, 4, 0, 0, 0x0E, 0x10, 255]);
        let (kind, lease, _) = parse_reply(&reply, 7).unwrap();
        assert_eq!(kind, ACK);
        assert_eq!(lease.ip, Ipv4Addr::new(10, 0, 2, 15));
        assert_eq!(lease.router, Some(Ipv4Addr::new(10, 0, 2, 2)));
        assert_eq!(lease.seconds, 3600);
        assert!(parse_reply(&reply, 8).is_none(), "wrong transaction id");
    }
}
