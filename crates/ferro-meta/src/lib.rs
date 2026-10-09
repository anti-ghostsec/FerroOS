//! Removes personal metadata from images without re-encoding them.
//!
//! Photos carry far more than pixels: camera serial numbers, GPS
//! coordinates, timestamps, editing software, author names and embedded
//! thumbnails (which can show a pre-crop image). This crate drops those
//! blocks and copies the image data byte for byte, so quality is untouched.
//!
//! Kept on purpose: colour information (ICC profiles, sRGB/gamma, the Adobe
//! JPEG colour-transform segment), because removing it changes how the image
//! looks. Lost on purpose: the EXIF orientation flag, so a photo shot sideways
//! may display rotated, as with most "remove properties" tools.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaError {
    /// Not a JPEG, PNG or WebP file.
    Unsupported,
    /// The file ends early or a block length is impossible.
    Corrupt(&'static str),
}

impl fmt::Display for MetaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetaError::Unsupported => f.write_str("only JPEG, PNG and WebP images are supported"),
            MetaError::Corrupt(why) => write!(f, "the file looks damaged ({why})"),
        }
    }
}

/// One kind of metadata that was removed, with how many bytes it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub kind: &'static str,
    pub bytes: usize,
}

#[derive(Debug)]
pub struct Stripped {
    pub format: Format,
    pub data: Vec<u8>,
    pub removed: Vec<Removed>,
}

impl Stripped {
    pub fn removed_bytes(&self) -> usize {
        self.removed.iter().map(|r| r.bytes).sum()
    }
}

pub fn detect(data: &[u8]) -> Option<Format> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else {
        None
    }
}

/// True if the file name suggests a format this crate handles.
pub fn is_supported_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("jpg" | "jpeg" | "jpe" | "jfif" | "png" | "webp"))
}

pub fn strip(data: &[u8]) -> Result<Stripped, MetaError> {
    let format = detect(data).ok_or(MetaError::Unsupported)?;
    let mut removed = Vec::new();
    let data = match format {
        Format::Jpeg => strip_jpeg(data, &mut removed)?,
        Format::Png => strip_png(data, &mut removed)?,
        Format::WebP => strip_webp(data, &mut removed)?,
    };
    // Merge repeated kinds (e.g. several text chunks) for a readable report.
    let mut merged: Vec<Removed> = Vec::new();
    for r in removed {
        match merged.iter_mut().find(|m| m.kind == r.kind) {
            Some(m) => m.bytes += r.bytes,
            None => merged.push(r),
        }
    }
    Ok(Stripped { format, data, removed: merged })
}

fn note(removed: &mut Vec<Removed>, kind: &'static str, bytes: usize) {
    removed.push(Removed { kind, bytes });
}

fn strip_jpeg(data: &[u8], removed: &mut Vec<Removed>) -> Result<Vec<u8>, MetaError> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..2]); // SOI
    let mut i = 2;
    loop {
        // Markers may be preceded by fill bytes (0xFF).
        while data.get(i) == Some(&0xFF) && data.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if data.get(i) != Some(&0xFF) {
            return Err(MetaError::Corrupt("expected a JPEG marker"));
        }
        let marker = *data.get(i + 1).ok_or(MetaError::Corrupt("truncated marker"))?;
        // Standalone markers carry no length.
        if marker == 0xD9 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            out.extend_from_slice(&data[i..i + 2]);
            i += 2;
            if marker == 0xD9 {
                out.extend_from_slice(&data[i..]); // anything after EOI is kept as-is
                return Ok(out);
            }
            continue;
        }
        let len = usize::from(u16::from_be_bytes([
            *data.get(i + 2).ok_or(MetaError::Corrupt("truncated length"))?,
            *data.get(i + 3).ok_or(MetaError::Corrupt("truncated length"))?,
        ]));
        let end = i + 2 + len;
        if len < 2 || end > data.len() {
            return Err(MetaError::Corrupt("segment runs past the end"));
        }
        let payload = &data[i + 4..end];
        let drop_as = match marker {
            0xE1 if payload.starts_with(b"Exif\0") => Some("EXIF (camera, GPS, timestamps, thumbnail)"),
            0xE1 if payload.starts_with(b"http://ns.adobe.com/") => Some("XMP (author, edit history, location)"),
            0xE1 => Some("Other APP1 metadata"),
            0xED => Some("IPTC / Photoshop (author, caption, keywords)"),
            0xFE => Some("Comment"),
            // APP3..APP12 and APP15: vendor metadata (maker notes, Ducky, etc.).
            0xE3..=0xEC | 0xEF => Some("Vendor metadata"),
            _ => None, // APP0 JFIF, APP2 ICC colour, APP14 Adobe colour, tables, frames
        };
        match drop_as {
            Some(kind) => note(removed, kind, end - i),
            None => out.extend_from_slice(&data[i..end]),
        }
        i = end;
        if marker == 0xDA {
            // Start of scan: the rest is compressed image data; copy verbatim.
            out.extend_from_slice(&data[i..]);
            return Ok(out);
        }
    }
}

fn strip_png(data: &[u8], removed: &mut Vec<Removed>) -> Result<Vec<u8>, MetaError> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..8]);
    let mut i = 8;
    while i < data.len() {
        let header = data.get(i..i + 8).ok_or(MetaError::Corrupt("truncated chunk header"))?;
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let end = i.checked_add(12 + len).filter(|&e| e <= data.len()).ok_or(MetaError::Corrupt("chunk runs past the end"))?;
        let kind = &header[4..8];
        let drop_as = match kind {
            b"tEXt" | b"zTXt" | b"iTXt" => Some("Text (author, software, comments)"),
            b"eXIf" => Some("EXIF (camera, GPS, timestamps)"),
            b"tIME" => Some("Last-modified time"),
            _ => None,
        };
        match drop_as {
            Some(k) => note(removed, k, end - i),
            None => out.extend_from_slice(&data[i..end]),
        }
        i = end;
        if kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

fn strip_webp(data: &[u8], removed: &mut Vec<Removed>) -> Result<Vec<u8>, MetaError> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..12]);
    let mut i = 12;
    let mut vp8x_flags_at = None;
    while i + 8 <= data.len() {
        let size = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        let end = i + 8 + size + (size & 1); // chunks are padded to even length
        if end > data.len() {
            return Err(MetaError::Corrupt("chunk runs past the end"));
        }
        match &data[i..i + 4] {
            b"EXIF" => note(removed, "EXIF (camera, GPS, timestamps)", end - i),
            b"XMP " => note(removed, "XMP (author, edit history, location)", end - i),
            fourcc => {
                if fourcc == b"VP8X" {
                    vp8x_flags_at = Some(out.len() + 8);
                }
                out.extend_from_slice(&data[i..end]);
            }
        }
        i = end;
    }
    // The extended header advertises EXIF (bit 3) and XMP (bit 2); clear them.
    if let Some(at) = vp8x_flags_at {
        if let Some(flags) = out.get_mut(at) {
            *flags &= !(0x08 | 0x04);
        }
    }
    let riff_size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn jpeg() -> Vec<u8> {
        [
            vec![0xFF, 0xD8],
            seg(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"),
            seg(0xE1, b"Exif\0\0MM\0*GPS-LAT-51.5"),
            seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:author>Me</x:author>"),
            seg(0xE2, b"ICC_PROFILE\0\x01\x01data"),
            seg(0xFE, b"shot at home"),
            seg(0xDB, &[0; 65]),
            seg(0xDA, &[1, 1, 0, 0, 63, 0]),
            vec![0x12, 0x34, 0xFF, 0x00, 0x56], // entropy-coded data with a stuffed byte
            vec![0xFF, 0xD9],
        ]
        .concat()
    }

    #[test]
    fn jpeg_drops_exif_xmp_comment_keeps_colour_and_pixels() {
        let input = jpeg();
        let s = strip(&input).unwrap();
        assert_eq!(s.format, Format::Jpeg);
        let kinds: Vec<_> = s.removed.iter().map(|r| r.kind).collect();
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        assert!(!s.data.windows(7).any(|w| w == b"GPS-LAT"));
        assert!(!s.data.windows(2).any(|w| w == b"Me"));
        assert!(s.data.windows(11).any(|w| w == b"ICC_PROFILE"), "colour profile kept");
        assert!(s.data.ends_with(&[0x12, 0x34, 0xFF, 0x00, 0x56, 0xFF, 0xD9]), "image data untouched");
        assert_eq!(s.data.len() + s.removed_bytes(), input.len());
        // Stripping again finds nothing.
        assert!(strip(&s.data).unwrap().removed.is_empty());
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat()
    }

    #[test]
    fn png_drops_text_exif_time() {
        let input = [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            chunk(b"IHDR", &[0; 13]),
            chunk(b"tEXt", b"Author\0Jane"),
            chunk(b"iCCP", b"icc"),
            chunk(b"eXIf", b"MM\0*gps"),
            chunk(b"tIME", &[7; 7]),
            chunk(b"IDAT", b"pixels"),
            chunk(b"IEND", b""),
        ]
        .concat();
        let s = strip(&input).unwrap();
        assert_eq!(s.removed.len(), 3);
        assert!(!s.data.windows(4).any(|w| w == b"Jane"));
        assert!(s.data.windows(4).any(|w| w == b"iCCP") && s.data.windows(6).any(|w| w == b"pixels"));
    }

    #[test]
    fn webp_drops_chunks_and_fixes_header() {
        let riff = |chunks: &[u8]| [b"RIFF".to_vec(), ((chunks.len() + 4) as u32).to_le_bytes().to_vec(), b"WEBP".to_vec(), chunks.to_vec()].concat();
        let ck = |f: &[u8; 4], d: &[u8]| {
            let mut v = [&f[..], &(d.len() as u32).to_le_bytes(), d].concat();
            if d.len() % 2 == 1 {
                v.push(0);
            }
            v
        };
        let mut vp8x = vec![0u8; 10];
        vp8x[0] = 0x08 | 0x04 | 0x10; // EXIF, XMP, alpha
        let input = riff(&[ck(b"VP8X", &vp8x), ck(b"VP8L", b"img"), ck(b"EXIF", b"gps"), ck(b"XMP ", b"<me/>")].concat());
        let s = strip(&input).unwrap();
        assert_eq!(s.removed.len(), 2);
        assert_eq!(s.data[12 + 8], 0x10, "only the alpha flag remains");
        let riff_size = u32::from_le_bytes(s.data[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_size, s.data.len() - 8);
    }

    #[test]
    fn rejects_other_and_damaged_files() {
        assert_eq!(strip(b"%PDF-1.7").unwrap_err(), MetaError::Unsupported);
        let mut bad = jpeg();
        bad.truncate(10);
        assert!(matches!(strip(&bad), Err(MetaError::Corrupt(_))));
        assert!(is_supported_name("IMG_0001.JPG") && is_supported_name("a.webp") && !is_supported_name("a.gif"));
    }
}
