//! The smallest HTTP/2 client that DNS-over-HTTPS needs (RFC 9113): one
//! request per stream, a static HPACK header block, and enough connection
//! housekeeping (SETTINGS/PING acks, window updates) to keep a connection
//! alive for many queries. Some DoH providers (e.g. Quad9) only speak HTTP/2.

use std::io::{self, Read, Write};

pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const DATA: u8 = 0x0;
const HEADERS: u8 = 0x1;
const RST_STREAM: u8 = 0x3;
const SETTINGS: u8 = 0x4;
const PING: u8 = 0x6;
const GOAWAY: u8 = 0x7;
const WINDOW_UPDATE: u8 = 0x8;
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const MAX_FRAME: usize = 1 << 16;

pub fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut f = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
    f.extend_from_slice(&(stream & 0x7FFF_FFFF).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// Client preface plus our SETTINGS (server push off).
pub fn connection_start() -> Vec<u8> {
    let mut v = PREFACE.to_vec();
    v.extend(frame(SETTINGS, 0, 0, &[0, 2, 0, 0, 0, 0])); // ENABLE_PUSH = 0
    v
}

fn hpack_string(out: &mut Vec<u8>, s: &str) {
    // Plain (non-Huffman) literal; our values are short (< 127 bytes).
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

/// HPACK block for `POST /dns-query` using static-table indices only, so no
/// dynamic table state is ever needed on our side.
pub fn doh_headers(authority: &str, body_len: usize) -> Vec<u8> {
    let mut h = vec![0x83, 0x87]; // :method POST, :scheme https
    h.push(0x04); // :path, literal without indexing
    hpack_string(&mut h, "/dns-query");
    h.push(0x01); // :authority
    hpack_string(&mut h, authority);
    h.extend_from_slice(&[0x0F, 0x10]); // content-type (static 31)
    hpack_string(&mut h, "application/dns-message");
    h.extend_from_slice(&[0x0F, 0x04]); // accept (static 19)
    hpack_string(&mut h, "application/dns-message");
    h.extend_from_slice(&[0x0F, 0x0D]); // content-length (static 28)
    hpack_string(&mut h, &body_len.to_string());
    h
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<(u8, u8, u32, Vec<u8>)> {
    let mut head = [0u8; 9];
    r.read_exact(&mut head)?;
    let len = (usize::from(head[0]) << 16) | (usize::from(head[1]) << 8) | usize::from(head[2]);
    if len > MAX_FRAME {
        return Err(io::Error::other("HTTP/2 frame too large"));
    }
    let stream = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) & 0x7FFF_FFFF;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((head[3], head[4], stream, payload))
}

/// Sends one DoH request on `stream` and returns the response body.
pub fn request<S: Read + Write>(s: &mut S, stream: u32, authority: &str, body: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = frame(HEADERS, END_HEADERS, stream, &doh_headers(authority, body.len()));
    out.extend(frame(DATA, END_STREAM, stream, body));
    s.write_all(&out)?;
    s.flush()?;
    let mut resp = Vec::new();
    loop {
        let (kind, flags, id, payload) = read_frame(s)?;
        match kind {
            SETTINGS if flags & ACK == 0 => s.write_all(&frame(SETTINGS, ACK, 0, &[]))?,
            PING if flags & ACK == 0 => s.write_all(&frame(PING, ACK, 0, &payload))?,
            GOAWAY => return Err(io::Error::other("server closed the HTTP/2 connection")),
            RST_STREAM if id == stream => return Err(io::Error::other("server reset the DoH request")),
            HEADERS if id == stream && flags & END_STREAM != 0 => return Ok(resp),
            DATA if id == stream => {
                let data = if flags & PADDED != 0 {
                    let pad = usize::from(*payload.first().unwrap_or(&0));
                    payload.get(1..payload.len().saturating_sub(pad)).unwrap_or(&[])
                } else {
                    &payload[..]
                };
                resp.extend_from_slice(data);
                // Give the connection window back so it never runs dry.
                if !payload.is_empty() {
                    s.write_all(&frame(WINDOW_UPDATE, 0, 0, &(payload.len() as u32).to_be_bytes()))?;
                }
                if flags & END_STREAM != 0 {
                    return Ok(resp);
                }
            }
            _ => {} // WINDOW_UPDATE, PRIORITY, other streams' frames, acks
        }
        s.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pipe {
        input: io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Pipe {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.input.read(b)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn request_and_response() {
        let mut server = frame(SETTINGS, 0, 0, &[]);
        server.extend(frame(HEADERS, END_HEADERS, 1, &[0x88])); // :status 200
        server.extend(frame(DATA, PADDED, 1, &[2, b'd', b'n', 0, 0]));
        server.extend(frame(DATA, END_STREAM, 1, b"s!"));
        let mut pipe = Pipe { input: io::Cursor::new(server), output: Vec::new() };
        let body = request(&mut pipe, 1, "dns.quad9.net", b"query").unwrap();
        assert_eq!(body, b"dns!");
        // We sent HEADERS + DATA, acked their SETTINGS, and returned window.
        let mut sent = io::Cursor::new(pipe.output);
        let kinds: Vec<u8> = std::iter::from_fn(|| read_frame(&mut sent).ok().map(|f| f.0)).collect();
        assert_eq!(kinds, [HEADERS, DATA, SETTINGS, WINDOW_UPDATE, WINDOW_UPDATE]);
    }

    #[test]
    fn header_block_is_static_only() {
        let h = doh_headers("dns.quad9.net", 128);
        assert_eq!(&h[..3], &[0x83, 0x87, 0x04]);
        assert!(h.windows(10).any(|w| w == b"/dns-query"));
        assert!(h.windows(3).any(|w| w == b"128"));
    }
}
