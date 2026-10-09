//! Just enough DNS wire format for the resolver and the `nslookup` tool.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const TYPE_A: u16 = 1;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_OPT: u16 = 41;
/// HTTPS (SVCB) records: carry ALPN, address hints and ECH keys.
pub const TYPE_HTTPS: u16 = 65;
const OPTION_PADDING: u16 = 12;
/// RFC 8467 recommends padding queries to multiples of 128 bytes.
const PAD_BLOCK: usize = 128;

pub fn type_from_name(s: &str) -> Option<u16> {
    Some(match s.to_ascii_uppercase().as_str() {
        "A" => TYPE_A,
        "AAAA" => TYPE_AAAA,
        "CNAME" => TYPE_CNAME,
        "HTTPS" => TYPE_HTTPS,
        _ => return None,
    })
}

/// Builds a standard recursive query with an EDNS0 OPT record.
pub fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 1]); // RD; 1 question; 1 additional
    for label in name.trim_end_matches('.').split('.').filter(|l| !l.is_empty()) {
        q.push(label.len().min(63) as u8);
        q.extend_from_slice(&label.as_bytes()[..label.len().min(63)]);
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes()); // IN
                                              // OPT: root name, type 41, UDP size 1232, no flags, empty rdata.
    q.extend_from_slice(&[0, 0, 41, 0x04, 0xD0, 0, 0, 0, 0, 0, 0]);
    q
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// Skips a (possibly compressed) name, returning the offset after it.
fn skip_name(b: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let len = *b.get(i)? as usize;
        if len == 0 {
            return Some(i + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(i + 2);
        }
        i += 1 + len;
    }
}

/// Reads a name, following compression pointers.
pub fn read_name(b: &[u8], mut i: usize) -> Option<String> {
    let mut out = String::new();
    for _ in 0..64 {
        let len = *b.get(i)? as usize;
        if len == 0 {
            return Some(if out.is_empty() { ".".into() } else { out });
        }
        if len & 0xC0 == 0xC0 {
            i = (u16_at(b, i)? & 0x3FFF) as usize;
            continue;
        }
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&String::from_utf8_lossy(b.get(i + 1..i + 1 + len)?));
        i += 1 + len;
    }
    None // pointer loop
}

/// The question (lower-cased name, type, class), used as the cache key.
pub fn question(b: &[u8]) -> Option<(String, u16, u16)> {
    if u16_at(b, 4)? != 1 {
        return None;
    }
    let name = read_name(b, 12)?.to_ascii_lowercase();
    let end = skip_name(b, 12)?;
    Some((name, u16_at(b, end)?, u16_at(b, end + 2)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub rtype: u16,
    pub ttl: u32,
    pub data: Vec<u8>,
    /// Offset of `data` in the message (names inside may be compressed).
    pub data_at: usize,
}

/// Parses the answer section; also returns the response code.
pub fn answers(b: &[u8]) -> Option<(u8, Vec<Record>)> {
    let rcode = b.get(3)? & 0x0F;
    let (qd, an) = (u16_at(b, 4)?, u16_at(b, 6)?);
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(b, i)? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..an {
        let name = read_name(b, i)?;
        i = skip_name(b, i)?;
        let rtype = u16_at(b, i)?;
        let ttl = u32::from_be_bytes(b.get(i + 4..i + 8)?.try_into().ok()?);
        let len = u16_at(b, i + 8)? as usize;
        let data_at = i + 10;
        let data = b.get(data_at..data_at + len)?.to_vec();
        out.push(Record { name, rtype, ttl, data, data_at });
        i = data_at + len;
    }
    Some((rcode, out))
}

/// The `ech` value (ECHConfigList) from an HTTPS record's SvcParams.
pub fn https_ech(rdata: &[u8]) -> Option<Vec<u8>> {
    // SvcPriority(2) TargetName(uncompressed) then key/len/value triples.
    let mut i = skip_name(rdata, 2)?;
    while i + 4 <= rdata.len() {
        let (key, len) = (u16_at(rdata, i)?, u16_at(rdata, i + 2)? as usize);
        let value = rdata.get(i + 4..i + 4 + len)?;
        if key == 5 {
            return Some(value.to_vec());
        }
        i += 4 + len;
    }
    None
}

/// Pads a query to a multiple of 128 bytes with an EDNS0 Padding option,
/// so the encrypted query's size doesn't reveal the name's length.
pub fn pad_query(q: &[u8]) -> Vec<u8> {
    let mut out = q.to_vec();
    let has_opt = answers_end(q).and_then(|end| {
        // A trailing OPT record: root name (0), type 41.
        (q.len() >= end + 11 && q[end] == 0 && u16_at(q, end + 1) == Some(TYPE_OPT)).then_some(end)
    });
    let Some(opt_at) = has_opt else {
        if u16_at(q, 10) != Some(0) {
            return out; // other additional records; leave untouched
        }
        out[11] = 1;
        out.extend_from_slice(&[0, 0, 41, 0x04, 0xD0, 0, 0, 0, 0, 0, 0]);
        return pad_query(&out);
    };
    let rdlen_at = opt_at + 9;
    let pad_len = (PAD_BLOCK - (out.len() + 4) % PAD_BLOCK) % PAD_BLOCK;
    let new_rdlen = u16_at(&out, rdlen_at).unwrap_or(0) as usize + 4 + pad_len;
    out[rdlen_at..rdlen_at + 2].copy_from_slice(&(new_rdlen as u16).to_be_bytes());
    out.extend_from_slice(&OPTION_PADDING.to_be_bytes());
    out.extend_from_slice(&(pad_len as u16).to_be_bytes());
    out.resize(out.len() + pad_len, 0);
    out
}

/// Offset just past the question/answer/authority sections.
fn answers_end(b: &[u8]) -> Option<usize> {
    let counts = [u16_at(b, 4)?, u16_at(b, 6)?, u16_at(b, 8)?];
    let mut i = 12;
    for _ in 0..counts[0] {
        i = skip_name(b, i)? + 4;
    }
    for _ in 0..counts[1] + counts[2] {
        i = skip_name(b, i)?;
        i += 10 + u16_at(b, i + 8)? as usize;
    }
    Some(i)
}

/// A small in-RAM answer cache: faster repeat lookups and fewer queries
/// leaving the machine. Never written to disk.
pub struct Cache {
    entries: HashMap<(String, u16, u16), (Vec<u8>, Instant)>,
    cap: usize,
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self { entries: HashMap::new(), cap }
    }

    /// A cached answer with the query's ID patched in.
    pub fn get(&mut self, query: &[u8]) -> Option<Vec<u8>> {
        let key = question(query)?;
        let (resp, expires) = self.entries.get(&key)?;
        if Instant::now() >= *expires {
            self.entries.remove(&key);
            return None;
        }
        let mut r = resp.clone();
        r[..2].copy_from_slice(&query[..2]);
        Some(r)
    }

    pub fn put(&mut self, query: &[u8], resp: &[u8]) {
        let (Some(key), Some((rcode, recs))) = (question(query), answers(resp)) else { return };
        if rcode != 0 || recs.is_empty() {
            return; // don't cache failures or empty answers
        }
        let ttl = recs.iter().map(|r| r.ttl).min().unwrap_or(0).min(3600);
        if ttl == 0 {
            return;
        }
        if self.entries.len() >= self.cap {
            let now = Instant::now();
            self.entries.retain(|_, (_, exp)| *exp > now);
            if self.entries.len() >= self.cap {
                self.entries.clear();
            }
        }
        self.entries.insert(key, (resp.to_vec(), Instant::now() + Duration::from_secs(u64::from(ttl))));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(query: &[u8], ttl: u32) -> Vec<u8> {
        let mut r = query[..query.len() - 11].to_vec(); // drop OPT
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 1; // one answer
        r[11] = 0;
        r.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1]);
        r.extend_from_slice(&ttl.to_be_bytes());
        r.extend_from_slice(&[0, 4, 93, 184, 216, 34]);
        r
    }

    #[test]
    fn query_round_trip() {
        let q = build_query(0xBEEF, "Example.COM", TYPE_A);
        assert_eq!(question(&q), Some(("example.com".into(), TYPE_A, 1)));
        let r = response(&q, 300);
        let (rcode, recs) = answers(&r).unwrap();
        assert_eq!(rcode, 0);
        assert_eq!(recs[0].name, "Example.COM");
        assert_eq!(recs[0].data, [93, 184, 216, 34]);
    }

    #[test]
    fn padding_hides_name_length() {
        for name in ["a.io", "a-much-longer-name.example.org"] {
            let p = pad_query(&build_query(1, name, TYPE_A));
            assert_eq!(p.len() % 128, 0, "{name}");
            assert_eq!(question(&p).unwrap().0, name);
        }
        let short = pad_query(&build_query(1, "a.io", TYPE_A));
        let long = pad_query(&build_query(1, "abcdefghij.example.org", TYPE_A));
        assert_eq!(short.len(), long.len());
    }

    #[test]
    fn cache_patches_id_and_expires() {
        let mut c = Cache::new(4);
        let q1 = build_query(1, "example.com", TYPE_A);
        c.put(&q1, &response(&q1, 60));
        let q2 = build_query(2, "EXAMPLE.com", TYPE_A);
        let hit = c.get(&q2).expect("cached");
        assert_eq!(&hit[..2], &[0, 2]);
        let q3 = build_query(3, "zero.ttl", TYPE_A);
        c.put(&q3, &response(&q3, 0));
        assert!(c.get(&q3).is_none(), "TTL 0 is never cached");
    }

    #[test]
    fn ech_from_https_record() {
        // priority 1, target ".", alpn(1)=h2, ech(5)=[AA BB]
        let rdata = [0, 1, 0, 0, 1, 0, 3, 2, b'h', b'2', 0, 5, 0, 2, 0xAA, 0xBB];
        assert_eq!(https_ech(&rdata), Some(vec![0xAA, 0xBB]));
        assert_eq!(https_ech(&rdata[..10]), None);
    }
}
