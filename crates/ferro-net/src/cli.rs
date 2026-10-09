//! `ferro-net`: one binary, three tools (the TLS stack is stored once).
//!
//! ```text
//! ferro-net daemon            network service (started by init)
//! nslookup NAME [TYPE]        query the local encrypted resolver
//! fetch https://URL           HTTPS GET with Encrypted Client Hello
//! ```

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("ferro-net runs on FerroOS (Linux)");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    let argv0 = args.first().map(|a| a.rsplit('/').next().unwrap_or(a).to_owned()).unwrap_or_default();
    let (cmd, rest) = match argv0.as_str() {
        "nslookup" | "fetch" => (argv0.as_str(), &args[1..]),
        _ => (args.get(1).map_or("", String::as_str), args.get(2..).unwrap_or(&[])),
    };
    let code = match cmd {
        "daemon" => linux::daemon(),
        "nslookup" => linux::nslookup(rest),
        "fetch" => linux::fetch(rest),
        _ => {
            eprintln!("usage: ferro-net daemon | nslookup NAME [A|AAAA|HTTPS] | fetch https://URL");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(target_os = "linux")]
mod linux {
    use crate::dns::{self, Cache};
    use crate::route::{Plan, Router};
    use crate::tls::{self, DohClient};
    use crate::{netif, wg, KILL_FILE, NET_STATUS, SESSION_FILE, TOR_DNS, TOR_FILE, TOR_SOCKS, VPN_CONFIG, VPN_FILE};
    use std::io::{BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const RESOLVER: &str = "127.0.0.1:53";

    fn killed() -> bool {
        Path::new(KILL_FILE).exists()
    }

    fn tor_on() -> bool {
        Path::new(TOR_FILE).exists()
    }

    /// Bumped whenever routing changes: connections and cached answers made
    /// on the old path are dropped rather than reused on the new one.
    static ROUTE_GENERATION: AtomicU32 = AtomicU32::new(0);

    // ---- daemon ----------------------------------------------------------------------

    pub fn daemon() -> i32 {
        // Everything resolves through us; nothing here is ever written to disk.
        let _ = std::fs::write("/etc/resolv.conf", "nameserver 127.0.0.1\noptions edns0\n");
        let _ = netif::set_up("lo", true);
        let floor = crate::clock::build_floor();
        if unix_now() < floor {
            set_clock(floor);
            eprintln!("ferro-net: clock was before this build's date (dead RTC battery?); moved forward");
        }
        let online = Arc::new(AtomicBool::new(false));
        let link_online = online.clone();
        std::thread::spawn(move || manage_links(&link_online));
        let clock_online = online.clone();
        std::thread::spawn(move || sync_clock(&clock_online));
        serve_dns(&online)
    }

    fn unix_now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    fn set_clock(secs: u64) {
        let tv = libc::timeval { tv_sec: secs as _, tv_usec: 0 };
        // SAFETY: settimeofday with a valid timeval and no timezone.
        unsafe { libc::settimeofday(&tv, std::ptr::null()) };
    }

    /// Corrects the clock from an authenticated HTTPS `Date` header, at
    /// start-up and every 6 hours while online.
    fn sync_clock(online: &AtomicBool) {
        loop {
            // Not over Tor: the Tor service itself needs the clock, and it
            // already rejects wildly wrong time.
            while !online.load(Ordering::SeqCst) || killed() || tor_on() {
                std::thread::sleep(Duration::from_secs(2));
            }
            match https_date() {
                Ok(server) => {
                    let skew = server as i64 - unix_now() as i64;
                    if skew.abs() > 30 {
                        set_clock(server);
                        eprintln!("ferro-net: clock corrected by {skew} s (verified HTTPS time)");
                    }
                    std::thread::sleep(Duration::from_secs(6 * 3600));
                }
                Err(e) => {
                    eprintln!("ferro-net: time check failed: {e}");
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
        }
    }

    fn https_date() -> std::io::Result<u64> {
        let host = "cloudflare-dns.com";
        let config = tls::client_config(None, &[b"http/1.1"]).map_err(std::io::Error::other)?;
        let mut s = tls::connect("1.1.1.1:443".parse().unwrap(), host, config)?;
        write!(s, "HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")?;
        s.flush()?;
        let mut head = String::new();
        let mut r = BufReader::new(s);
        loop {
            let mut line = String::new();
            if std::io::BufRead::read_line(&mut r, &mut line)? == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        head.lines()
            .find_map(|l| l.strip_prefix("date:").or_else(|| l.strip_prefix("Date:")))
            .and_then(|d| crate::clock::parse_http_date(d.trim()))
            .ok_or_else(|| std::io::Error::other("no Date header"))
    }

    /// Keeps interfaces matching the kill switch: down while it's off; up
    /// with a fresh DHCP lease (renewed at half-life) while it's on. Routing
    /// follows the VPN and Tor switches (see `route`).
    fn manage_links(online: &AtomicBool) {
        let mut macs = std::collections::HashMap::new();
        let mut lease_until: Option<Instant> = None;
        let mut uplink: Option<(String, Ipv4Addr)> = None;
        let mut router = Router::default();
        let mut last_status = Instant::now() - Duration::from_secs(60);
        loop {
            let ifaces: Vec<String> = netif::interfaces().into_iter().filter(|i| i != crate::route::WG_IF).collect();
            let mut force = false;
            if killed() {
                for i in &ifaces {
                    let _ = netif::set_up(i, false);
                }
                lease_until = None;
                uplink = None;
            } else if let Some(iface) = ifaces.first() {
                let need_lease = !netif::is_up(iface) || lease_until.is_none_or(|t| Instant::now() >= t);
                if need_lease {
                    online.store(false, Ordering::SeqCst);
                    let mac = *macs.entry(iface.clone()).or_insert_with(|| match netif::randomize_mac(iface) {
                        Ok(m) => {
                            eprintln!("ferro-net: {iface}: random MAC {}", mac_text(&m));
                            m
                        }
                        Err(e) => {
                            eprintln!("ferro-net: {iface}: can't randomize MAC ({e})");
                            [0; 6]
                        }
                    });
                    let _ = netif::set_up(iface, true);
                    match netif::dhcp(iface, &mac).and_then(|l| netif::configure(iface, &l).map(|()| l)) {
                        Ok(l) => {
                            eprintln!("ferro-net: {iface}: {} via DHCP", l.ip);
                            // Strict reverse-path filtering (drops spoofed
                            // packets) can only start once we have a route:
                            // before that it would drop the DHCP reply itself.
                            let _ = std::fs::write("/proc/sys/net/ipv4/conf/all/rp_filter", "1");
                            let _ = std::fs::write(format!("/proc/sys/net/ipv4/conf/{iface}/rp_filter"), "1");
                            lease_until = Some(Instant::now() + Duration::from_secs(u64::from(l.seconds.max(120) / 2)));
                            uplink = l.router.map(|gw| (iface.clone(), gw));
                            force = true;
                        }
                        Err(e) => {
                            eprintln!("ferro-net: {iface}: {e}");
                            uplink = None;
                            std::thread::sleep(Duration::from_secs(5));
                        }
                    }
                }
            }
            // Nothing but DHCP goes out before logon: the VPN and Tor choices
            // are in the vault and aren't known yet.
            let session = Path::new(SESSION_FILE).exists();
            let plan = Plan {
                uplink: uplink.clone().filter(|_| session && !killed()),
                vpn: Path::new(VPN_FILE).exists().then(|| match std::fs::read_to_string(VPN_CONFIG) {
                    Ok(text) => wg::parse(&text).map_err(|e| format!("C:\\ProgramData\\ferro\\vpn.conf: {e}")),
                    Err(_) => Err("no VPN profile yet: save your provider's WireGuard file as C:\\ProgramData\\ferro\\vpn.conf".into()),
                }),
                tor: tor_on(),
            };
            if router.reconcile(&plan, force) {
                ROUTE_GENERATION.fetch_add(1, Ordering::SeqCst);
            }
            online.store(plan.uplink.is_some(), Ordering::SeqCst);
            if last_status.elapsed() >= Duration::from_secs(2) {
                write_status(&router.status(&plan));
                last_status = Instant::now();
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// World-readable, so the desktop can show it; holds no secrets.
    fn write_status(text: &str) {
        let tmp = format!("{NET_STATUS}.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, NET_STATUS);
        }
    }

    fn mac_text(m: &[u8; 6]) -> String {
        m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
    }

    /// Local DNS service: answers from the RAM cache or forwards over HTTPS.
    /// HTTPS/SVCB records pass through untouched, which is what lets
    /// ECH-capable apps find servers' ECH keys.
    fn serve_dns(online: &AtomicBool) -> i32 {
        let sock = match UdpSocket::bind(RESOLVER) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ferro-net: can't serve DNS on {RESOLVER}: {e}");
                return 1;
            }
        };
        let mut doh = DohClient::new();
        let mut cache = Cache::new(512);
        let mut was_killed = false;
        let mut generation = ROUTE_GENERATION.load(Ordering::SeqCst);
        let mut buf = [0u8; 1500];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
            let query = &buf[..n];
            if n < 12 {
                continue;
            }
            let k = killed() || !online.load(Ordering::SeqCst);
            let g = ROUTE_GENERATION.load(Ordering::SeqCst);
            if (k && !was_killed) || g != generation {
                cache.clear(); // nothing from the old session or path lingers
                doh.disconnect();
                generation = g;
            }
            was_killed = k;
            let reply = if k {
                Some(error_reply(query, 5)) // REFUSED: network is off
            } else if let Some(hit) = cache.get(query) {
                Some(hit)
            } else if tor_on() {
                // Through Tor: the exit relay resolves, so no DNS server sees us.
                match tor_dns(query) {
                    Ok(resp) if resp.len() >= 12 => {
                        cache.put(query, &resp);
                        Some(resp)
                    }
                    _ => Some(error_reply(query, 2)),
                }
            } else {
                match doh.query(&dns::pad_query(query)) {
                    Ok(resp) if resp.len() >= 12 => {
                        let mut resp = resp;
                        resp[..2].copy_from_slice(&query[..2]);
                        cache.put(query, &resp);
                        Some(resp)
                    }
                    Ok(_) => Some(error_reply(query, 2)),
                    Err(e) => {
                        eprintln!("ferro-net: DoH via {}: {e}", doh.provider().label);
                        Some(error_reply(query, 2)) // SERVFAIL
                    }
                }
            };
            if let Some(r) = reply {
                let _ = sock.send_to(&r, from);
            }
        }
    }

    fn tor_dns(query: &[u8]) -> std::io::Result<Vec<u8>> {
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        sock.set_read_timeout(Some(Duration::from_secs(10)))?;
        sock.send_to(query, TOR_DNS)?;
        let mut buf = [0u8; 4096];
        let n = sock.recv(&mut buf)?;
        Ok(buf[..n].to_vec())
    }

    fn error_reply(query: &[u8], rcode: u8) -> Vec<u8> {
        let mut r = query.to_vec();
        r[2] |= 0x80; // response
        r[3] = (r[3] & 0xF0) | rcode | 0x80; // RA
        r
    }

    // ---- nslookup --------------------------------------------------------------------

    fn lookup(name: &str, qtype: u16) -> std::io::Result<(u8, Vec<dns::Record>)> {
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        sock.set_read_timeout(Some(Duration::from_secs(8)))?;
        let id = std::process::id() as u16 ^ qtype;
        sock.send_to(&dns::build_query(id, name, qtype), RESOLVER)?;
        let mut buf = [0u8; 4096];
        let n = sock.recv(&mut buf)?;
        dns::answers(&buf[..n]).ok_or_else(|| std::io::Error::other("malformed DNS reply"))
    }

    pub fn nslookup(args: &[String]) -> i32 {
        let Some(name) = args.first() else {
            eprintln!("usage: nslookup NAME [A|AAAA|HTTPS]");
            return 2;
        };
        let types: Vec<u16> = match args.get(1).and_then(|t| dns::type_from_name(t)) {
            Some(t) => vec![t],
            None => vec![dns::TYPE_A, dns::TYPE_AAAA],
        };
        let via = if tor_on() { "through Tor" } else { "encrypted DNS over HTTPS" };
        println!("Server:  127.0.0.1 ({via})\nName:    {name}");
        let mut found = false;
        for t in types {
            match lookup(name, t) {
                Ok((0, recs)) => {
                    for r in recs {
                        found = true;
                        match r.rtype {
                            dns::TYPE_A if r.data.len() == 4 => println!("Address: {}", Ipv4Addr::new(r.data[0], r.data[1], r.data[2], r.data[3])),
                            dns::TYPE_AAAA if r.data.len() == 16 => {
                                let a: [u8; 16] = r.data.clone().try_into().unwrap();
                                println!("Address: {}", std::net::Ipv6Addr::from(a));
                            }
                            dns::TYPE_HTTPS => println!(
                                "HTTPS:   ECH key {}",
                                if dns::https_ech(&r.data).is_some() { "published (site supports Encrypted Client Hello)" } else { "not published" }
                            ),
                            dns::TYPE_CNAME => println!("Alias:   {}", r.name),
                            _ => {}
                        }
                    }
                }
                Ok((5, _)) => {
                    println!("Refused: the network kill switch is off");
                    return 1;
                }
                Ok((rcode, _)) => println!("No answer (DNS error {rcode})"),
                Err(e) => {
                    println!("Lookup failed: {e}");
                    return 1;
                }
            }
        }
        if found {
            0
        } else {
            1
        }
    }

    // ---- fetch -----------------------------------------------------------------------

    pub fn fetch(args: &[String]) -> i32 {
        // --direct skips Tor on purpose: a leak test. In Tor mode it must fail.
        let direct = args.first().is_some_and(|a| a == "--direct");
        let args = if direct { &args[1..] } else { args };
        let Some(url) = args.first() else {
            eprintln!("usage: fetch [--direct] https://HOST[:PORT]/PATH");
            return 2;
        };
        let Some(rest) = url.strip_prefix("https://") else {
            eprintln!("fetch: only https:// URLs (FerroOS doesn't speak plain HTTP)");
            return 2;
        };
        let (hostport, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
        let (host, port) = hostport.split_once(':').map_or((hostport, 443u16), |(h, p)| (h, p.parse().unwrap_or(443)));
        if tor_on() && !direct {
            return fetch_via_tor(host, port, &path);
        }

        let ip = match lookup(host, dns::TYPE_A) {
            Ok((0, recs)) => {
                recs.iter().find(|r| r.rtype == dns::TYPE_A && r.data.len() == 4).map(|r| Ipv4Addr::new(r.data[0], r.data[1], r.data[2], r.data[3]))
            }
            _ => None,
        };
        let Some(ip) = ip else {
            eprintln!("fetch: can't resolve {host}");
            return 1;
        };
        // The site's ECH key, if it publishes one, comes from its HTTPS record.
        let ech = lookup(host, dns::TYPE_HTTPS).ok().and_then(|(_, recs)| recs.iter().find_map(|r| dns::https_ech(&r.data)));
        let config = match tls::client_config(ech.as_deref(), &[b"http/1.1"]) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fetch: {e}");
                return 1;
            }
        };
        let mut stream = match tls::connect(SocketAddr::new(ip.into(), port), host, config) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fetch: {host}: {e}");
                return 1;
            }
        };
        eprintln!("fetch: TLS {:?}, ECH {}", stream.conn.protocol_version().map_or("?".into(), |v| format!("{v:?}")), tls::ech_status_text(&stream));
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: FerroOS\r\nAccept: */*\r\nConnection: close\r\n\r\n");
        if let Err(e) = stream.write_all(req.as_bytes()).and_then(|()| stream.flush()) {
            eprintln!("fetch: {e}");
            return 1;
        }
        finish_fetch(stream)
    }

    /// Through Tor's SOCKS port, with the host name resolved by the exit
    /// relay. No ECH here: Tor already hides the site from the network.
    fn fetch_via_tor(host: &str, port: u16, path: &str) -> i32 {
        let tcp = match socks_connect(host, port) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fetch: via Tor: {e}");
                return 1;
            }
        };
        let config = match tls::client_config(None, &[b"http/1.1"]) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fetch: {e}");
                return 1;
            }
        };
        let mut stream = match tls::over(tcp, host, config) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fetch: {host}: {e}");
                return 1;
            }
        };
        eprintln!("fetch: via Tor, TLS {:?}", stream.conn.protocol_version().map_or("?".into(), |v| format!("{v:?}")));
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: FerroOS\r\nAccept: */*\r\nConnection: close\r\n\r\n");
        if let Err(e) = stream.write_all(req.as_bytes()).and_then(|()| stream.flush()) {
            eprintln!("fetch: {e}");
            return 1;
        }
        finish_fetch(stream)
    }

    /// SOCKS5 CONNECT by name (RFC 1928), no authentication.
    fn socks_connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
        use std::io::{Error, ErrorKind};
        let mut s = TcpStream::connect(TOR_SOCKS).map_err(|e| Error::new(e.kind(), format!("Tor isn't running ({e})")))?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
        s.write_all(&[5, 1, 0])?;
        let mut r = [0u8; 2];
        s.read_exact(&mut r)?;
        if r != [5, 0] {
            return Err(Error::other("Tor's SOCKS port refused the handshake"));
        }
        let name = host.as_bytes();
        if name.len() > 255 {
            return Err(Error::new(ErrorKind::InvalidInput, "host name too long"));
        }
        let mut req = vec![5, 1, 0, 3, name.len() as u8];
        req.extend_from_slice(name);
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req)?;
        let mut head = [0u8; 4];
        s.read_exact(&mut head)?;
        if head[1] != 0 {
            return Err(Error::other(format!("Tor couldn't reach {host} (SOCKS error {})", head[1])));
        }
        let skip = match head[3] {
            1 => 4,
            4 => 16,
            3 => {
                let mut l = [0u8; 1];
                s.read_exact(&mut l)?;
                usize::from(l[0])
            }
            _ => return Err(Error::other("bad SOCKS reply")),
        };
        let mut rest = vec![0u8; skip + 2];
        s.read_exact(&mut rest)?;
        s.set_read_timeout(None)?;
        Ok(s)
    }

    fn finish_fetch(stream: tls::TlsStream) -> i32 {
        let mut reader = BufReader::new(stream);
        match tls::read_response(&mut reader) {
            Ok((status, body)) => {
                let _ = std::io::stdout().write_all(&body);
                if status == 200 {
                    0
                } else {
                    eprintln!("fetch: HTTP {status}");
                    1
                }
            }
            Err(e) => {
                // Some servers close without close_notify; show what arrived.
                let mut rest = Vec::new();
                let _ = reader.read_to_end(&mut rest);
                eprintln!("fetch: {e}");
                1
            }
        }
    }
}
