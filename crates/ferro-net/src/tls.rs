//! TLS for FerroOS: rustls with Encrypted Client Hello.
//!
//! ECH hides which site you're visiting from the network by encrypting the
//! server name inside the TLS handshake. It needs the server's ECH key,
//! published in its DNS HTTPS record; our resolver fetches those over
//! encrypted DNS. When a server publishes no key we still send a "GREASE"
//! ECH extension, so ECH users don't stand out from everyone else.

use hpke::aead::{Aead as HpkeAeadAlg, AeadCtxS, AesGcm128, ChaCha20Poly1305};
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem as _, OpModeS, Serializable};
use rustls::client::{EchConfig, EchGreaseConfig, EchMode, EchStatus};
use rustls::crypto::hpke::{EncapsulatedSecret, Hpke, HpkeOpener, HpkePrivateKey, HpkePublicKey, HpkeSealer, HpkeSuite};
use rustls::internal::msgs::enums::{HpkeAead, HpkeKdf, HpkeKem};
use rustls::internal::msgs::handshake::HpkeSymmetricCipherSuite;
use rustls::pki_types::{EchConfigListBytes, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::marker::PhantomData;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

// ---- HPKE (RFC 9180) for ECH, via the pure-Rust `hpke` crate -------------------------

type Kem = X25519HkdfSha256;

fn hpke_err(e: hpke::HpkeError) -> rustls::Error {
    rustls::Error::General(format!("HPKE: {e:?}"))
}

fn rng() -> rand_core::UnwrapErr<rand_core::OsRng> {
    rand_core::UnwrapErr(rand_core::OsRng)
}

/// X25519 + HKDF-SHA256 with AEAD `A`: the suites ECH deployments use.
struct Suite<A>(PhantomData<fn() -> A>, HpkeAead);

impl<A> fmt::Debug for Suite<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HPKE X25519/HKDF-SHA256/{:?}", self.1)
    }
}

struct Sealer<A: HpkeAeadAlg>(AeadCtxS<A, HkdfSha256, Kem>);

impl<A: HpkeAeadAlg> fmt::Debug for Sealer<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HpkeSealer")
    }
}

impl<A: HpkeAeadAlg + 'static> HpkeSealer for Sealer<A>
where
    AeadCtxS<A, HkdfSha256, Kem>: Send + Sync,
{
    fn seal(&mut self, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        self.0.seal(plaintext, aad).map_err(hpke_err)
    }
}

impl<A: HpkeAeadAlg + 'static> Hpke for Suite<A>
where
    AeadCtxS<A, HkdfSha256, Kem>: Send + Sync,
{
    fn seal(&self, info: &[u8], aad: &[u8], plaintext: &[u8], pub_key: &HpkePublicKey) -> Result<(EncapsulatedSecret, Vec<u8>), rustls::Error> {
        let (enc, mut sealer) = self.setup_sealer(info, pub_key)?;
        Ok((enc, sealer.seal(aad, plaintext)?))
    }

    fn setup_sealer(&self, info: &[u8], pub_key: &HpkePublicKey) -> Result<(EncapsulatedSecret, Box<dyn HpkeSealer>), rustls::Error> {
        let pk = <Kem as hpke::Kem>::PublicKey::from_bytes(&pub_key.0).map_err(hpke_err)?;
        let (enc, ctx) = hpke::setup_sender::<A, HkdfSha256, Kem, _>(&OpModeS::Base, &pk, info, &mut rng()).map_err(hpke_err)?;
        Ok((EncapsulatedSecret(enc.to_bytes().to_vec()), Box::new(Sealer(ctx))))
    }

    // A client only seals; opening is the server's job.
    fn open(&self, _: &EncapsulatedSecret, _: &[u8], _: &[u8], _: &[u8], _: &HpkePrivateKey) -> Result<Vec<u8>, rustls::Error> {
        Err(rustls::Error::General("HPKE open is server-side".into()))
    }

    fn setup_opener(&self, _: &EncapsulatedSecret, _: &[u8], _: &HpkePrivateKey) -> Result<Box<dyn HpkeOpener>, rustls::Error> {
        Err(rustls::Error::General("HPKE open is server-side".into()))
    }

    fn generate_key_pair(&self) -> Result<(HpkePublicKey, HpkePrivateKey), rustls::Error> {
        let (sk, pk) = Kem::gen_keypair(&mut rng());
        Ok((HpkePublicKey(pk.to_bytes().to_vec()), HpkePrivateKey::from(sk.to_bytes().to_vec())))
    }

    fn suite(&self) -> HpkeSuite {
        HpkeSuite { kem: HpkeKem::DHKEM_X25519_HKDF_SHA256, sym: HpkeSymmetricCipherSuite { kdf_id: HpkeKdf::HKDF_SHA256, aead_id: self.1 } }
    }
}

static AES128: Suite<AesGcm128> = Suite(PhantomData, HpkeAead::AES_128_GCM);
static CHACHA: Suite<ChaCha20Poly1305> = Suite(PhantomData, HpkeAead::CHACHA20_POLY_1305);
pub static HPKE_SUITES: &[&dyn Hpke] = &[&AES128, &CHACHA];

// ---- client configuration ------------------------------------------------------------

/// TLS 1.3 client config: real ECH when `ech_configs` (from the server's DNS
/// HTTPS record) is given, GREASE ECH otherwise. `alpn` lists the HTTP
/// versions to offer, preferred first.
pub fn client_config(ech_configs: Option<&[u8]>, alpn: &[&[u8]]) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let mode = match ech_configs {
        Some(list) => EchMode::Enable(EchConfig::new(EchConfigListBytes::from(list.to_vec()), HPKE_SUITES)?),
        None => {
            let (placeholder, _) = AES128.generate_key_pair()?;
            EchMode::Grease(EchGreaseConfig::new(&AES128, placeholder))
        }
    };
    let mut config = ClientConfig::builder_with_provider(provider).with_ech(mode)?.with_root_certificates(roots).with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(config))
}

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

pub fn connect(addr: SocketAddr, host: &str, config: Arc<ClientConfig>) -> io::Result<TlsStream> {
    over(TcpStream::connect_timeout(&addr, Duration::from_secs(5))?, host, config)
}

/// TLS over an already connected stream (e.g. a SOCKS tunnel through Tor).
pub fn over(tcp: TcpStream, host: &str, config: Arc<ClientConfig>) -> io::Result<TlsStream> {
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
    tcp.set_nodelay(true)?;
    let name = ServerName::try_from(host.to_owned()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let conn = ClientConnection::new(config, name).map_err(io::Error::other)?;
    let mut s = StreamOwned::new(conn, tcp);
    // Drive the handshake now so errors surface here, not on first read.
    while s.conn.is_handshaking() {
        s.conn.complete_io(&mut s.sock)?;
    }
    Ok(s)
}

pub fn ech_status_text(s: &TlsStream) -> &'static str {
    match s.conn.ech_status() {
        EchStatus::Accepted => "accepted (server name encrypted)",
        EchStatus::Rejected => "rejected by server",
        EchStatus::Grease => "not offered by server (GREASE sent)",
        EchStatus::Offered => "offered",
        EchStatus::NotOffered => "not used",
    }
}

// ---- minimal HTTP/1.1 ---------------------------------------------------------------

/// Reads one HTTP response: (status code, body). Handles Content-Length and
/// chunked bodies; leaves the connection reusable.
pub fn read_response<R: Read>(r: &mut BufReader<R>) -> io::Result<(u16, Vec<u8>)> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::other("bad HTTP status line"))?;
    let (mut length, mut chunked) = (None, false);
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse::<usize>().ok();
        } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            line.clear();
            r.read_line(&mut line)?;
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
            if size == 0 {
                r.read_line(&mut line)?;
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            r.read_exact(&mut body[start..])?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf)?;
        }
    } else if let Some(n) = length {
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    } else {
        r.read_to_end(&mut body)?;
    }
    Ok((status, body))
}

/// Reads through the buffer, writes straight to the TLS stream.
struct Duplex<'a>(&'a mut BufReader<TlsStream>);

impl Read for Duplex<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Duplex<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.get_mut().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.get_mut().flush()
    }
}

// ---- DNS over HTTPS (RFC 8484) -------------------------------------------------------

/// A DoH provider, reached by IP address so looking it up leaks nothing.
#[derive(Clone, Copy, Debug)]
pub struct Provider {
    pub label: &'static str,
    pub host: &'static str,
    pub addrs: &'static [&'static str],
}

/// Tried in order. All three state they keep no logs that identify users.
pub const PROVIDERS: &[Provider] = &[
    Provider { label: "Quad9", host: "dns.quad9.net", addrs: &["9.9.9.9:443", "149.112.112.112:443"] },
    Provider { label: "Mullvad", host: "dns.mullvad.net", addrs: &["194.242.2.2:443"] },
    Provider { label: "Cloudflare", host: "cloudflare-dns.com", addrs: &["1.1.1.1:443", "1.0.0.1:443"] },
];

/// Keeps one TLS connection open to the current provider (fast, and fewer
/// handshakes means less to observe); fails over to the next on errors.
pub struct DohClient {
    current: usize,
    conn: Option<BufReader<TlsStream>>,
    config: Option<Arc<ClientConfig>>,
    /// The connection negotiated HTTP/2; next request goes on this stream.
    h2_stream: Option<u32>,
}

impl Default for DohClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DohClient {
    pub fn new() -> Self {
        Self { current: 0, conn: None, config: None, h2_stream: None }
    }

    pub fn provider(&self) -> Provider {
        PROVIDERS[self.current]
    }

    /// Drops the connection (e.g. when the network kill switch is thrown).
    pub fn disconnect(&mut self) {
        self.conn = None;
        self.h2_stream = None;
    }

    pub fn query(&mut self, msg: &[u8]) -> io::Result<Vec<u8>> {
        let mut last_err = io::Error::other("no DoH provider reachable");
        // Each provider gets two tries (the second on a fresh connection,
        // in case the kept-alive one went stale), then the next one.
        for _ in 0..PROVIDERS.len() {
            for _ in 0..2 {
                match self.try_query(msg) {
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        last_err = io::Error::new(e.kind(), format!("{}: {e}", self.provider().label));
                        self.disconnect();
                    }
                }
            }
            self.current = (self.current + 1) % PROVIDERS.len();
        }
        Err(last_err)
    }

    fn try_query(&mut self, msg: &[u8]) -> io::Result<Vec<u8>> {
        if self.conn.is_none() {
            let config = match &self.config {
                Some(c) => c.clone(),
                None => {
                    let c = client_config(None, &[b"h2", b"http/1.1"]).map_err(io::Error::other)?;
                    self.config = Some(c.clone());
                    c
                }
            };
            let p = self.provider();
            let mut err = io::Error::other("no address");
            for a in p.addrs {
                match connect(a.parse().expect("valid provider address"), p.host, config.clone()) {
                    Ok(mut s) => {
                        if s.conn.alpn_protocol() == Some(b"h2") {
                            s.write_all(&crate::h2::connection_start())?;
                            self.h2_stream = Some(1);
                        }
                        self.conn = Some(BufReader::new(s));
                        break;
                    }
                    Err(e) => err = e,
                }
            }
            if self.conn.is_none() {
                return Err(err);
            }
        }
        let host = self.provider().host;
        let r = self.conn.as_mut().expect("connected");
        let body = match self.h2_stream {
            Some(stream) => {
                self.h2_stream = Some(stream + 2);
                crate::h2::request(&mut Duplex(r), stream, host, msg)?
            }
            None => Self::http1(r, host, msg)?,
        };
        // A real DNS answer to *this* query echoes its 16-bit ID.
        if body.len() < 12 || body[..2] != msg[..2] {
            return Err(io::Error::other("DoH reply was not a DNS answer"));
        }
        Ok(body)
    }

    fn http1(r: &mut BufReader<TlsStream>, host: &str, msg: &[u8]) -> io::Result<Vec<u8>> {
        let head = format!(
            "POST /dns-query HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/dns-message\r\n\
             Accept: application/dns-message\r\nContent-Length: {}\r\n\r\n",
            msg.len()
        );
        let s = r.get_mut();
        s.write_all(head.as_bytes())?;
        s.write_all(msg)?;
        s.flush()?;
        let (status, body) = read_response(r)?;
        if status != 200 {
            return Err(io::Error::other(format!("DoH HTTP status {status}")));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_bodies() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nxy\r\n1\r\nz\r\n0\r\n\r\n";
        let mut r = BufReader::new(&raw[..]);
        assert_eq!(read_response(&mut r).unwrap(), (200, b"abc".to_vec()));
        assert_eq!(read_response(&mut r).unwrap(), (200, b"xyz".to_vec()));
    }

    #[test]
    fn hpke_suites_seal() {
        let (pk, _) = AES128.generate_key_pair().unwrap();
        let (enc, ct) = AES128.seal(b"info", b"aad", b"hello", &pk).unwrap();
        assert_eq!(enc.0.len(), 32);
        assert_eq!(ct.len(), 5 + 16);
        assert!(client_config(None, &[b"h2"]).is_ok(), "GREASE config builds");
    }
}
