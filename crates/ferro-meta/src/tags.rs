//! Tags: the words people give their files ("anime", "holiday", "taxes"),
//! kept inside the file itself, where other programs look for them:
//!
//! - pictures (JPEG, PNG, WebP, GIF, TIFF): XMP `dc:subject`; a JPEG also
//!   has them as IPTC Keywords, which older programs read;
//! - PDF: XMP `dc:subject` (the document's Keywords too, written alongside);
//! - MP4, MOV, M4A: XMP `dc:subject` (and QuickTime's keywords alongside);
//! - MKV, WebM: the Matroska tag KEYWORDS, comma-separated.
//!
//! Other files have no standard place for them (a song's genre isn't one),
//! so they can't be tagged.
//!
//! [`read_tags`] reads them straight from the file, a few small reads each,
//! fast enough for a whole folder; nothing is started and nothing written.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Where a kind of file keeps its tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    /// XMP and IPTC.
    Jpeg,
    /// XMP: PNG, WebP, GIF, TIFF.
    Picture,
    /// XMP, and the document's Keywords.
    Pdf,
    /// XMP, and QuickTime's keywords: MP4, MOV, M4A.
    QuickTime,
    /// The KEYWORDS tag: MKV, WebM.
    Matroska,
}

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Whether, and how, a file like `path` (by its name) can keep tags.
pub fn kind(path: &Path) -> Option<TagKind> {
    Some(match ext(path).as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => TagKind::Jpeg,
        "png" | "webp" | "gif" | "tif" | "tiff" => TagKind::Picture,
        "pdf" => TagKind::Pdf,
        "mp4" | "m4v" | "mov" | "m4a" => TagKind::QuickTime,
        "mkv" | "webm" | "mka" => TagKind::Matroska,
        _ => return None,
    })
}

/// The most of a file read looking for its tags in one go.
const MAX_BLOCK: u64 = 4 << 20;
/// How much of each end of a file is looked through for its XMP (GIF, TIFF, PDF).
const SCAN: u64 = 1 << 20;

/// The tags `path` carries, in order, each once (none if it has none, can't
/// have any, or can't be read).
pub fn read_tags(path: &Path) -> Vec<String> {
    let Some(kind) = kind(path) else { return Vec::new() };
    let Ok(mut f) = File::open(path) else { return Vec::new() };
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return Vec::new();
    }
    let found = match (kind, ext(path).as_str()) {
        (TagKind::Jpeg, _) => jpeg(&mut f),
        (_, "png") => png(&mut f).map(|x| xmp_subjects(&x)),
        (_, "webp") => webp(&mut f).map(|x| xmp_subjects(&x)),
        (TagKind::QuickTime, _) => quicktime(&mut f).map(|x| xmp_subjects(&x)),
        (TagKind::Matroska, _) => matroska(&mut f),
        _ => scan(&mut f).map(|x| xmp_subjects(&x)),
    };
    tidy(found.unwrap_or_default())
}

/// Trimmed, without empty ones or the same one twice.
fn tidy(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    }
    out
}

/// What was typed as tags: commas separate them ("anime, comedy" is two).
/// Control characters are dropped.
pub fn split_typed(typed: &str) -> Vec<String> {
    tidy(typed.split(',').map(|t| t.chars().filter(|c| !c.is_control()).collect::<String>()))
}

fn read_n(f: &mut File, n: u64) -> Option<Vec<u8>> {
    if n > MAX_BLOCK {
        return None;
    }
    let mut buf = vec![0u8; n as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn u16_be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

// ---- XMP ----

/// The `dc:subject` list of an XMP packet.
pub fn xmp_subjects(xmp: &str) -> Vec<String> {
    let Some(start) = xmp.find("<dc:subject") else { return Vec::new() };
    let rest = &xmp[start..];
    let block = &rest[..rest.find("</dc:subject>").unwrap_or(rest.len())];
    let mut out = Vec::new();
    let mut at = block;
    while let Some(i) = at.find("<rdf:li") {
        let after = &at[i..];
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            at = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let end = body.find("</rdf:li>").unwrap_or(body.len());
        out.push(unescape(&body[..end]));
        at = &body[end..];
    }
    out
}

/// XML's `&amp;` and the like, as the characters they stand for.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(semi) = after.find(';').filter(|&n| n <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let name = &after[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The last XMP packet in `buf` (the newest, in a PDF changed since it was made).
fn last_packet(buf: &[u8]) -> Option<String> {
    let start = rfind(buf, b"<x:xmpmeta")?;
    let len = find(&buf[start..], b"</x:xmpmeta>")?;
    Some(String::from_utf8_lossy(&buf[start..start + len]).into_owned())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// GIF, TIFF, PDF: the XMP packet near the end, or else near the start.
fn scan(f: &mut File) -> Option<String> {
    let len = f.metadata().ok()?.len();
    if len <= 2 * SCAN {
        f.seek(SeekFrom::Start(0)).ok()?;
        return last_packet(&read_n(f, len)?);
    }
    f.seek(SeekFrom::Start(len - SCAN)).ok()?;
    if let Some(x) = last_packet(&read_n(f, SCAN)?) {
        return Some(x);
    }
    f.seek(SeekFrom::Start(0)).ok()?;
    last_packet(&read_n(f, SCAN)?)
}

// ---- JPEG ----

const XMP_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const PHOTOSHOP_SIG: &[u8] = b"Photoshop 3.0\0";

/// A JPEG's XMP subjects, or if it has none, its IPTC Keywords.
fn jpeg(f: &mut File) -> Option<Vec<String>> {
    let mut b = [0u8; 2];
    f.read_exact(&mut b).ok()?;
    if b != [0xFF, 0xD8] {
        return None;
    }
    let (mut xmp, mut iptc) = (Vec::new(), Vec::new());
    loop {
        f.read_exact(&mut b).ok()?;
        while b == [0xFF, 0xFF] {
            f.read_exact(&mut b[1..]).ok()?;
        }
        if b[0] != 0xFF {
            break;
        }
        let marker = b[1];
        if marker == 0xDA || marker == 0xD9 {
            break; // the picture itself: the metadata comes before it
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        f.read_exact(&mut b).ok()?;
        let len = u64::from(u16_be(&b)).checked_sub(2)?;
        match marker {
            0xE1 | 0xED => {
                let body = read_n(f, len)?;
                if let Some(x) = body.strip_prefix(XMP_SIG) {
                    xmp.extend(xmp_subjects(&String::from_utf8_lossy(x)));
                } else if let Some(irb) = body.strip_prefix(PHOTOSHOP_SIG) {
                    iptc.extend(iptc_keywords(irb));
                }
            }
            _ => {
                f.seek(SeekFrom::Current(len as i64)).ok()?;
            }
        }
    }
    Some(if xmp.is_empty() { iptc } else { xmp })
}

/// The IPTC Keywords (2:25) in a Photoshop resource block.
fn iptc_keywords(irb: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 12 <= irb.len() && &irb[i..i + 4] == b"8BIM" {
        let id = u16_be(&irb[i + 4..]);
        // The name: a length byte and that many, padded to an even count.
        let name_len = irb[i + 6] as usize;
        let mut j = i + 6 + ((name_len + 2) & !1);
        let Some(size) = irb.get(j..j + 4).map(u32_be) else { break };
        j += 4;
        let Some(data) = irb.get(j..j + size as usize) else { break };
        if id == 0x0404 {
            let mut k = 0;
            while k + 5 <= data.len() && data[k] == 0x1C {
                let (record, dataset, n) = (data[k + 1], data[k + 2], u16_be(&data[k + 3..]));
                if n & 0x8000 != 0 {
                    break; // an extended length: not something keywords use
                }
                let Some(v) = data.get(k + 5..k + 5 + n as usize) else { break };
                if record == 2 && dataset == 25 {
                    out.push(String::from_utf8_lossy(v).into_owned());
                }
                k += 5 + n as usize;
            }
        }
        i = j + ((size as usize + 1) & !1);
    }
    out
}

// ---- PNG, WebP ----

/// A PNG's XMP (an uncompressed iTXt chunk named XML:com.adobe.xmp).
fn png(f: &mut File) -> Option<String> {
    let mut sig = [0u8; 8];
    f.read_exact(&mut sig).ok()?;
    if &sig != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut head = [0u8; 8];
    loop {
        f.read_exact(&mut head).ok()?;
        let (len, kind) = (u64::from(u32_be(&head)), &head[4..8]);
        if kind == b"IEND" {
            return None;
        }
        if kind == b"iTXt" && len <= MAX_BLOCK {
            let data = read_n(f, len)?;
            f.seek(SeekFrom::Current(4)).ok()?;
            let key = b"XML:com.adobe.xmp\0";
            if let Some(rest) = data.strip_prefix(&key[..]) {
                // Not compressed (flag 0); then the language and the
                // translated keyword, each ending in a 0.
                if rest.first() == Some(&0) {
                    let rest = rest.get(2..)?;
                    let lang_end = rest.iter().position(|&c| c == 0)?;
                    let rest = &rest[lang_end + 1..];
                    let tkw_end = rest.iter().position(|&c| c == 0)?;
                    return Some(String::from_utf8_lossy(&rest[tkw_end + 1..]).into_owned());
                }
            }
        } else {
            f.seek(SeekFrom::Current(len as i64 + 4)).ok()?;
        }
    }
}

/// A WebP's XMP (its `XMP ` chunk).
fn webp(f: &mut File) -> Option<String> {
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"RIFF" || &head[8..12] != b"WEBP" {
        return None;
    }
    let mut chunk = [0u8; 8];
    loop {
        f.read_exact(&mut chunk).ok()?;
        let len = u64::from(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        if &chunk[..4] == b"XMP " {
            return read_n(f, len).map(|d| String::from_utf8_lossy(&d).into_owned());
        }
        f.seek(SeekFrom::Current((len + (len & 1)) as i64)).ok()?;
    }
}

// ---- MP4, MOV ----

const XMP_UUID: [u8; 16] = [0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC];

/// The XMP of an MP4 or MOV: a `uuid` box at the top, or `XMP_` in
/// `moov/udta`.
fn quicktime(f: &mut File) -> Option<String> {
    let len = f.metadata().ok()?.len();
    boxes(f, 0, len, 0)
}

fn boxes(f: &mut File, start: u64, end: u64, depth: u8) -> Option<String> {
    let mut at = start;
    while at + 8 <= end {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head).ok()?;
        let mut size = u64::from(u32_be(&head));
        let mut header = 8;
        if size == 1 {
            let mut big = [0u8; 8];
            f.read_exact(&mut big).ok()?;
            size = u64::from_be_bytes(big);
            header = 16;
        } else if size == 0 {
            size = end - at;
        }
        if size < header || at + size > end {
            return None;
        }
        let body = size - header;
        match &head[4..8] {
            b"uuid" if body >= 16 => {
                let mut id = [0u8; 16];
                f.read_exact(&mut id).ok()?;
                if id == XMP_UUID {
                    return read_n(f, body - 16).map(|d| String::from_utf8_lossy(&d).into_owned());
                }
            }
            b"XMP_" => return read_n(f, body).map(|d| String::from_utf8_lossy(&d).into_owned()),
            b"moov" | b"udta" if depth < 2 => {
                if let Some(x) = boxes(f, at + header, at + size, depth + 1) {
                    return Some(x);
                }
            }
            _ => {}
        }
        at += size;
    }
    None
}

// ---- Matroska ----

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const CLUSTER: u32 = 0x1F43_B675;
const TAGS: u32 = 0x1254_C367;
const TAG: u32 = 0x7373;
const TARGETS: u32 = 0x63C0;
const SIMPLE_TAG: u32 = 0x67C8;
const TAG_NAME: u32 = 0x45A3;
const TAG_STRING: u32 = 0x4487;
/// In Targets: what a tag is for, if not the whole file (a track, a chapter...).
const TARGET_UIDS: [u32; 4] = [0x63C5, 0x63C4, 0x63C6, 0x63C9];

/// An element's ID (its length marker kept, as the IDs are written) and
/// how many bytes it took.
fn ebml_id(b: &[u8]) -> Option<(u32, usize)> {
    let n = b.first()?.leading_zeros() as usize + 1;
    if n > 4 || b.len() < n {
        return None;
    }
    Some((b[..n].iter().fold(0u32, |v, &x| v << 8 | u32::from(x)), n))
}

/// An element's size (`None` inside: not known, as a stream being
/// recorded has) and how many bytes it took.
fn ebml_size(b: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *b.first()?;
    let n = first.leading_zeros() as usize + 1;
    if n > 8 || b.len() < n {
        return None;
    }
    let mask = (0xFFu16 >> n) as u8;
    let mut v = u64::from(first & mask);
    let mut all_ones = first & mask == mask;
    for &x in &b[1..n] {
        v = v << 8 | u64::from(x);
        all_ones &= x == 0xFF;
    }
    Some((if all_ones { None } else { Some(v) }, n))
}

/// The elements directly inside `buf`: (ID, contents).
fn ebml_children(buf: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let Some((id, a)) = ebml_id(&buf[i..]) else { break };
        let Some((Some(size), b)) = ebml_size(&buf[i + a..]) else { break };
        let start = i + a + b;
        let Some(body) = usize::try_from(size).ok().and_then(|s| buf.get(start..start.checked_add(s)?)) else { break };
        out.push((id, body));
        i = start + body.len();
    }
    out
}

/// The KEYWORDS of an MKV or WebM (the ones for the whole file).
fn matroska(f: &mut File) -> Option<Vec<String>> {
    let len = f.metadata().ok()?.len();
    let mut head = [0u8; 12];
    let mut at = 0u64;
    let mut segment_end = None;
    let mut out = Vec::new();
    while at < segment_end.unwrap_or(len).min(len) {
        f.seek(SeekFrom::Start(at)).ok()?;
        let got = f.read(&mut head).ok()?;
        let (id, a) = ebml_id(&head[..got])?;
        let (size, b) = ebml_size(&head[a..got])?;
        let body_at = at + (a + b) as u64;
        if at == 0 && id != EBML {
            return None;
        }
        match (id, size) {
            // Into the segment: what the file holds is inside it.
            (SEGMENT, _) => {
                segment_end = size.map(|s| body_at + s);
                at = body_at;
                continue;
            }
            (TAGS, Some(s)) => {
                f.seek(SeekFrom::Start(body_at)).ok()?;
                out.extend(keywords(&read_n(f, s)?));
            }
            // A cluster recorded live, of no known size: past it nothing can be found.
            (CLUSTER, None) => break,
            (_, None) => return None,
            _ => {}
        }
        at = body_at + size?;
    }
    Some(out)
}

fn keywords(tags: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, tag) in ebml_children(tags).into_iter().filter(|(id, _)| *id == TAG) {
        let children = ebml_children(tag);
        let for_part = children.iter().filter(|(id, _)| *id == TARGETS).any(|(_, t)| ebml_children(t).iter().any(|(id, _)| TARGET_UIDS.contains(id)));
        if for_part {
            continue;
        }
        for (_, simple) in children.iter().filter(|(id, _)| *id == SIMPLE_TAG) {
            let parts = ebml_children(simple);
            let name = parts.iter().find(|(id, _)| *id == TAG_NAME).map(|(_, v)| String::from_utf8_lossy(v));
            if name.is_some_and(|n| n.trim_end_matches('\0').eq_ignore_ascii_case("KEYWORDS")) {
                if let Some((_, v)) = parts.iter().find(|(id, _)| *id == TAG_STRING) {
                    let s = String::from_utf8_lossy(v);
                    out.extend(s.trim_end_matches('\0').split(',').map(str::to_owned));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str, data: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ferro-meta-tags-{}-{name}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        p
    }

    fn xmp(tags: &[&str]) -> String {
        let lis: String = tags.iter().map(|t| format!("<rdf:li>{t}</rdf:li>")).collect();
        format!(
            "<?xpacket begin=''?><x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF><rdf:Description><dc:subject><rdf:Bag>{lis}</rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end='w'?>"
        )
    }

    fn check(name: &str, data: &[u8], want: &[&str]) {
        let p = temp(name, data);
        let got = read_tags(&p);
        let _ = std::fs::remove_file(&p);
        assert_eq!(got, want, "{name}");
    }

    #[test]
    fn subjects_of_an_xmp_packet() {
        let x = xmp(&["anime", "Tom &amp; Jerry", "&#252; sports", "a&lt;b"]);
        assert_eq!(xmp_subjects(&x), ["anime", "Tom & Jerry", "ü sports", "a<b"]);
        assert!(xmp_subjects("<x:xmpmeta><dc:title>x</dc:title></x:xmpmeta>").is_empty());
    }

    #[test]
    fn typed_tags() {
        assert_eq!(split_typed(" anime, comedy ,, anime,\tsports\n"), ["anime", "comedy", "sports"]);
        assert!(split_typed(" , ").is_empty());
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new("a/b.JPG")), Some(TagKind::Jpeg));
        assert_eq!(kind(Path::new("b.webm")), Some(TagKind::Matroska));
        assert_eq!(kind(Path::new("b.mp3")), None);
        assert_eq!(kind(Path::new("b.docx")), None);
    }

    fn jpeg_segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    fn iptc_block(words: &[&str]) -> Vec<u8> {
        let mut records = vec![0x1C, 2, 0, 0, 2, 0, 4];
        for w in words {
            records.extend_from_slice(&[0x1C, 2, 25]);
            records.extend_from_slice(&(w.len() as u16).to_be_bytes());
            records.extend_from_slice(w.as_bytes());
        }
        let mut irb = PHOTOSHOP_SIG.to_vec();
        irb.extend_from_slice(b"8BIM\x04\x04\0\0");
        irb.extend_from_slice(&(records.len() as u32).to_be_bytes());
        irb.extend_from_slice(&records);
        if records.len() % 2 == 1 {
            irb.push(0);
        }
        irb
    }

    #[test]
    fn jpeg_xmp_and_iptc() {
        let mut j = vec![0xFF, 0xD8];
        j.extend(jpeg_segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        let mut app1 = XMP_SIG.to_vec();
        app1.extend_from_slice(xmp(&["anime", "comedy"]).as_bytes());
        j.extend(jpeg_segment(0xE1, &app1));
        j.extend(jpeg_segment(0xED, &iptc_block(&["anime"])));
        j.extend_from_slice(&[0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
        check("x.jpg", &j, &["anime", "comedy"]);

        // IPTC alone (tagged by an older program).
        let mut only = vec![0xFF, 0xD8];
        only.extend(jpeg_segment(0xED, &iptc_block(&["holiday", "sea"])));
        only.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        check("i.jpeg", &only, &["holiday", "sea"]);
        check("none.jpg", &[0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9], &[]);
        check("junk.jpg", b"not a picture", &[]);
    }

    #[test]
    fn png_xmp() {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        p.extend(chunk(b"IHDR", &[0; 13]));
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(xmp(&["sports"]).as_bytes());
        p.extend(chunk(b"iTXt", &itxt));
        p.extend(chunk(b"IDAT", &[1, 2, 3]));
        p.extend(chunk(b"IEND", &[]));
        check("x.png", &p, &["sports"]);
    }

    #[test]
    fn webp_xmp() {
        let x = xmp(&["cat"]);
        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(b"VP8 \x03\0\0\0abc\0");
        body.extend_from_slice(b"XMP ");
        body.extend_from_slice(&(x.len() as u32).to_le_bytes());
        body.extend_from_slice(x.as_bytes());
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&(body.len() as u32).to_le_bytes());
        w.extend(body);
        check("x.webp", &w, &["cat"]);
    }

    #[test]
    fn pdf_newest_packet_counts() {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        pdf.extend_from_slice(xmp(&["old"]).as_bytes());
        pdf.extend_from_slice(b"\nendstream\n%%EOF\n");
        // Changed since: the tags taken off, in a packet added at the end.
        pdf.extend_from_slice(b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF/></x:xmpmeta>\n%%EOF\n");
        check("x.pdf", &pdf, &[]);
        let mut again = pdf.clone();
        again.extend_from_slice(xmp(&["new", "two"]).as_bytes());
        check("y.pdf", &again, &["new", "two"]);
    }

    #[test]
    fn mp4_xmp_box() {
        let x = xmp(&["film"]);
        let mut m = Vec::new();
        let bx = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b
        };
        m.extend(bx(b"ftyp", b"isom\0\0\x02\0"));
        m.extend(bx(b"moov", &bx(b"udta", &bx(b"name", b"x"))));
        let mut uuid = XMP_UUID.to_vec();
        uuid.extend_from_slice(x.as_bytes());
        m.extend(bx(b"mdat", &[0; 32]));
        m.extend(bx(b"uuid", &uuid));
        check("x.mp4", &m, &["film"]);
        // In moov/udta/XMP_, as a MOV may have it.
        let mut q = bx(b"ftyp", b"qt  \0\0\0\0");
        q.extend(bx(b"moov", &bx(b"udta", &bx(b"XMP_", x.as_bytes()))));
        check("x.mov", &q, &["film"]);
    }

    fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = id.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        v.push(0x01); // an 8-byte size
        v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn matroska_keywords() {
        let simple = |name: &str, value: &str| el(SIMPLE_TAG, &[el(TAG_NAME, name.as_bytes()), el(TAG_STRING, value.as_bytes())].concat());
        let global = el(TAG, &[el(TARGETS, &[]), simple("ENCODER", "x"), simple("KEYWORDS", "anime, ü sports")].concat());
        let track = el(TAG, &[el(TARGETS, &el(0x63C5, &[1])), simple("KEYWORDS", "not this")].concat());
        let tags = el(TAGS, &[global, track].concat());
        let segment = el(SEGMENT, &[el(0x1549_A966, &[0; 4]), el(CLUSTER, &[0; 64]), tags].concat());
        let file = [el(EBML, &[0; 8]), segment].concat();
        check("x.mkv", &file, &["anime", "ü sports"]);
        check("bad.webm", &[0x1A, 0x45, 0xDF, 0xA3, 0xFF], &[]);
    }

    /// Files tagged "anime" by ExifTool or ffmpeg, in the folder
    /// FERRO_TAG_SAMPLES names: `cargo test -p ferro-meta -- --ignored`.
    #[test]
    #[ignore]
    fn tagged_by_other_programs() {
        let dir = std::env::var_os("FERRO_TAG_SAMPLES").expect("FERRO_TAG_SAMPLES");
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if kind(&p).is_some() {
                let tags = read_tags(&p);
                println!("{}: {tags:?}", p.display());
                assert!(tags.iter().any(|t| t == "anime"), "{}", p.display());
            }
        }
    }

    #[test]
    fn unsupported_files_have_none() {
        check("x.mp3", b"ID3\x04 whatever", &[]);
    }
}
