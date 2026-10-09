//! WireGuard configuration files, in the standard format VPN providers hand
//! out (`[Interface]` + `[Peer]`). Commands in them (`PostUp` and friends)
//! are never run, and `DNS` is ignored: lookups keep going over encrypted
//! DNS, which then travels inside the tunnel.

use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    pub endpoint: SocketAddr,
    pub allowed_ips: Vec<(IpAddr, u8)>,
    pub keepalive: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub private_key: [u8; 32],
    /// The tunnel's own addresses (`Address = 10.64.0.2/32, fc00::2/128`).
    pub addresses: Vec<(IpAddr, u8)>,
    pub mtu: u32,
    pub listen_port: Option<u16>,
    pub peers: Vec<Peer>,
}

/// Standard base64 of a 32-byte key.
pub fn key(s: &str) -> Result<[u8; 32], String> {
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::with_capacity(32);
    for c in s.trim().bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err("a key isn't valid base64".into()),
        };
        bits = (bits << 6) | u32::from(v);
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    out.try_into().map_err(|_| "a key isn't 32 bytes (44 base64 characters)".into())
}

fn cidr(s: &str) -> Result<(IpAddr, u8), String> {
    let s = s.trim();
    let (ip, len) = s.split_once('/').map_or((s, None), |(a, b)| (a, Some(b)));
    let ip: IpAddr = ip.parse().map_err(|_| format!("\"{s}\" isn't an IP address"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len = match len {
        Some(l) => l.parse::<u8>().ok().filter(|&l| l <= max).ok_or_else(|| format!("bad prefix length in \"{s}\""))?,
        None => max,
    };
    Ok((ip, len))
}

/// A `[Peer]` section while it's being read.
#[derive(Default)]
struct PeerDraft {
    public_key: Option<[u8; 32]>,
    preshared_key: Option<[u8; 32]>,
    endpoint: Option<SocketAddr>,
    allowed_ips: Vec<(IpAddr, u8)>,
    keepalive: Option<u16>,
}

impl PeerDraft {
    fn finish(self) -> Result<Peer, String> {
        Ok(Peer {
            public_key: self.public_key.ok_or("a [Peer] has no PublicKey")?,
            preshared_key: self.preshared_key,
            endpoint: self.endpoint.ok_or("a [Peer] has no Endpoint")?,
            allowed_ips: self.allowed_ips,
            keepalive: self.keepalive,
        })
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Interface,
        Peer,
    }
    let mut section = Section::None;
    let mut private_key = None;
    let mut addresses = Vec::new();
    let mut mtu = 1420;
    let mut listen_port = None;
    let mut peers: Vec<Peer> = Vec::new();
    let mut peer: Option<PeerDraft> = None;

    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if let Some(p) = peer.take() {
                peers.push(p.finish()?);
            }
            section = match line.to_ascii_lowercase().as_str() {
                "[interface]" => Section::Interface,
                "[peer]" => {
                    peer = Some(PeerDraft::default());
                    Section::Peer
                }
                _ => return Err(format!("line {}: unknown section {line}", n + 1)),
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { return Err(format!("line {}: expected Key = Value", n + 1)) };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        let list = || v.split(',').map(str::trim).filter(|s| !s.is_empty());
        match (&section, k.as_str()) {
            (Section::Interface, "privatekey") => private_key = Some(key(v)?),
            (Section::Interface, "address") => {
                for a in list() {
                    addresses.push(cidr(a)?);
                }
            }
            (Section::Interface, "mtu") => mtu = v.parse().ok().filter(|m| (1280..=9000).contains(m)).ok_or("MTU must be 1280-9000")?,
            (Section::Interface, "listenport") => listen_port = Some(v.parse().map_err(|_| "bad ListenPort")?),
            (Section::Peer, _) => {
                let p = peer.as_mut().expect("in [Peer]");
                match k.as_str() {
                    "publickey" => p.public_key = Some(key(v)?),
                    "presharedkey" => p.preshared_key = Some(key(v)?),
                    "endpoint" => {
                        p.endpoint = Some(v.parse().map_err(|_| {
                            format!(
                                "Endpoint \"{v}\" must be an IP address and port. A host name would have to be looked up outside the tunnel, which would reveal it."
                            )
                        })?)
                    }
                    "allowedips" => {
                        for a in list() {
                            p.allowed_ips.push(cidr(a)?);
                        }
                    }
                    "persistentkeepalive" => p.keepalive = if v == "off" { None } else { Some(v.parse().map_err(|_| "bad PersistentKeepalive")?) },
                    _ => {}
                }
            }
            // DNS, Table, PostUp... are deliberately ignored.
            _ => {}
        }
    }
    if let Some(p) = peer.take() {
        peers.push(p.finish()?);
    }
    let private_key = private_key.ok_or("the [Interface] has no PrivateKey")?;
    if addresses.is_empty() {
        return Err("the [Interface] has no Address".into());
    }
    if peers.is_empty() {
        return Err("there is no [Peer]".into());
    }
    if peers.iter().any(|p| p.endpoint.is_ipv6()) {
        return Err("IPv6 endpoints aren't supported yet; use the provider's IPv4 server address".into());
    }
    Ok(Config { private_key, addresses, mtu, listen_port, peers })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# An example profile. These keys are made up for this test and unused anywhere.
[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.64.0.2/32, fc00:bbbb::2/128
DNS = 10.64.0.1
PostUp = rm -rf /

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
PresharedKey = /UwcSPg38hW/D9Y3tcS1FOV0K1wuURMbS0sesJEP5ak=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = 185.65.135.1:51820
PersistentKeepalive = 25
";

    #[test]
    fn parses_a_provider_config() {
        let c = parse(SAMPLE).unwrap();
        assert_eq!(c.addresses, vec![("10.64.0.2".parse().unwrap(), 32), ("fc00:bbbb::2".parse().unwrap(), 128)]);
        assert_eq!(c.mtu, 1420);
        let p = &c.peers[0];
        assert_eq!(p.endpoint, "185.65.135.1:51820".parse().unwrap());
        assert_eq!(p.allowed_ips.len(), 2);
        assert_eq!(p.keepalive, Some(25));
        assert!(p.preshared_key.is_some());
        assert_eq!(c.private_key[0], 0xC8);
    }

    #[test]
    fn host_name_endpoints_are_refused() {
        let e = parse(&SAMPLE.replace("185.65.135.1:51820", "vpn.example.com:51820")).unwrap_err();
        assert!(e.contains("IP address"), "{e}");
    }

    #[test]
    fn bad_keys_are_refused() {
        assert!(key("short").is_err());
        assert!(key("yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fB!k=").is_err());
        assert!(parse("[Interface]\nAddress = 10.0.0.2\n[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nEndpoint = 1.2.3.4:5\n")
            .is_err());
    }
}
