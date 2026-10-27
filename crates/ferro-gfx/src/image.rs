//! Image decoding and rendering primitives for FerroOS.
//!
//! Provides zero-dependency BMP and PPM image loading and hardware/software
//! surface rendering with nearest-neighbor scaling.

use crate::{Rect, Surface};
use std::fs;
use std::path::Path;

/// A decoded 32-bit XRGB (`0x00RRGGBB`) image buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl DecodedImage {
    pub fn new(width: usize, height: usize, pixels: Vec<u32>) -> Self {
        assert_eq!(pixels.len(), width * height, "pixel count must match dimensions");
        Self { width, height, pixels }
    }

    /// Loads an image from a file on disk (supports BMP, PPM, PNG, JPEG, WebP).
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        Self::decode(&bytes)
    }

    /// Loads a scaled thumbnail image directly from a file on disk.
    pub fn load_thumbnail_from_file<P: AsRef<Path>>(path: P, max_w: u32, max_h: u32) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        Self::decode_thumbnail(&bytes, max_w, max_h)
    }

    /// A square `size` x `size` thumbnail with the picture's own shape kept:
    /// scaled to fit, centred, the rest white (the file list's background).
    pub fn thumbnail_fit(bytes: &[u8], size: u32) -> Option<Self> {
        let (w, h, pixels) = match image::load_from_memory(bytes) {
            Ok(img) => {
                let t = img.thumbnail(size, size).to_rgba8();
                let (w, h) = t.dimensions();
                (w as usize, h as usize, t.pixels().map(|p| ((p.0[0] as u32) << 16) | ((p.0[1] as u32) << 8) | (p.0[2] as u32)).collect::<Vec<u32>>())
            }
            Err(_) => {
                let img = Self::decode(bytes)?;
                let scale = (size as f32 / img.width.max(1) as f32).min(size as f32 / img.height.max(1) as f32).min(1.0);
                let (w, h) = (((img.width as f32 * scale) as usize).max(1), ((img.height as f32 * scale) as usize).max(1));
                let mut s = Surface::new(w, h);
                s.draw_image_scaled(Rect::new(0, 0, w as i32, h as i32), &img);
                (w, h, s.pixels().to_vec())
            }
        };
        let n = size as usize;
        let mut out = vec![0x00FF_FFFFu32; n * n];
        let (ox, oy) = ((n.saturating_sub(w)) / 2, (n.saturating_sub(h)) / 2);
        for y in 0..h.min(n) {
            for x in 0..w.min(n) {
                out[(oy + y) * n + ox + x] = pixels[y * w + x];
            }
        }
        Some(DecodedImage::new(n, n, out))
    }

    /// Decodes image data from an in-memory byte slice.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
            if let Some(img) = decode_bmp(bytes) {
                return Some(img);
            }
        } else if bytes.len() >= 2 && &bytes[0..2] == b"P6" {
            if let Some(img) = decode_ppm(bytes) {
                return Some(img);
            }
        }
        if let Ok(dyn_img) = image::load_from_memory(bytes) {
            let rgba = dyn_img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let pixels: Vec<u32> = rgba
                .pixels()
                .map(|p| {
                    let [r, g, b, _a] = p.0;
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                })
                .collect();
            return Some(DecodedImage::new(w as usize, h as usize, pixels));
        }
        None
    }

    /// Decodes a downscaled thumbnail from an in-memory byte slice without keeping full resolution in RAM.
    pub fn decode_thumbnail(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Self> {
        if let Ok(dyn_img) = image::load_from_memory(bytes) {
            let thumb = dyn_img.thumbnail_exact(max_w, max_h).to_rgba8();
            let (w, h) = thumb.dimensions();
            let pixels: Vec<u32> = thumb
                .pixels()
                .map(|p| {
                    let [r, g, b, _a] = p.0;
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                })
                .collect();
            return Some(DecodedImage::new(w as usize, h as usize, pixels));
        }
        if let Some(img) = Self::decode(bytes) {
            let mut s = Surface::new(max_w as usize, max_h as usize);
            s.draw_image_scaled(Rect::new(0, 0, max_w as i32, max_h as i32), &img);
            let pixels = match s.pixels {
                crate::Pixels::Owned(v) => v,
                _ => vec![0u32; max_w as usize * max_h as usize],
            };
            return Some(DecodedImage::new(max_w as usize, max_h as usize, pixels));
        }
        None
    }
}

/// A multi-frame image animation (e.g. animated GIF).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageAnimation {
    pub frames: Vec<(DecodedImage, std::time::Duration)>,
}

impl ImageAnimation {
    pub fn decode_gif(bytes: &[u8]) -> Option<Self> {
        use image::AnimationDecoder;
        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
        let raw_frames = decoder.into_frames().collect_frames().ok()?;
        if raw_frames.is_empty() {
            return None;
        }
        let mut frames = Vec::with_capacity(raw_frames.len());
        for f in raw_frames {
            let delay_ms = {
                let (numer, denom) = f.delay().numer_denom_ms();
                numer.checked_div(denom).map(|d| d.max(20)).unwrap_or(100)
            };
            let (w, h) = f.buffer().dimensions();
            let pixels: Vec<u32> = f
                .buffer()
                .pixels()
                .map(|p| {
                    let [r, g, b, _a] = p.0;
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                })
                .collect();
            frames.push((DecodedImage::new(w as usize, h as usize, pixels), std::time::Duration::from_millis(delay_ms as u64)));
        }
        Some(Self { frames })
    }
}

/// Decodes standard 24-bit or 32-bit uncompressed Windows BMP data.
pub fn decode_bmp(bytes: &[u8]) -> Option<DecodedImage> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return None;
    }

    let pixel_offset = u32::from_le_bytes(bytes[10..14].try_into().ok()?) as usize;
    let dib_header_size = u32::from_le_bytes(bytes[14..18].try_into().ok()?);
    if dib_header_size < 40 {
        return None;
    }

    let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?);
    let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?);
    let planes = u16::from_le_bytes(bytes[26..28].try_into().ok()?);
    let bpp = u16::from_le_bytes(bytes[28..30].try_into().ok()?);
    let compression = u32::from_le_bytes(bytes[30..34].try_into().ok()?);

    if planes != 1 || width <= 0 || height == 0 || (compression != 0 && compression != 3) {
        return None;
    }
    if bpp != 24 && bpp != 32 {
        return None;
    }

    let w = width as usize;
    let h = height.unsigned_abs() as usize;
    let bottom_up = height > 0;

    let row_stride = match bpp {
        24 => (w * 3).div_ceil(4) * 4,
        32 => w * 4,
        _ => return None,
    };

    if pixel_offset + row_stride * h > bytes.len() {
        return None;
    }

    let mut pixels = vec![0u32; w * h];

    for row in 0..h {
        let src_row = if bottom_up { h - 1 - row } else { row };
        let row_start = pixel_offset + src_row * row_stride;
        let dst_row_start = row * w;

        match bpp {
            24 => {
                for col in 0..w {
                    let off = row_start + col * 3;
                    let b = bytes[off] as u32;
                    let g = bytes[off + 1] as u32;
                    let r = bytes[off + 2] as u32;
                    pixels[dst_row_start + col] = (r << 16) | (g << 8) | b;
                }
            }
            32 => {
                for col in 0..w {
                    let off = row_start + col * 4;
                    let b = bytes[off] as u32;
                    let g = bytes[off + 1] as u32;
                    let r = bytes[off + 2] as u32;
                    pixels[dst_row_start + col] = (r << 16) | (g << 8) | b;
                }
            }
            _ => return None,
        }
    }

    Some(DecodedImage { width: w, height: h, pixels })
}

/// Decodes binary Portable Pixmap (PPM P6) format.
pub fn decode_ppm(bytes: &[u8]) -> Option<DecodedImage> {
    if bytes.len() < 10 || &bytes[0..2] != b"P6" {
        return None;
    }

    let mut pos = 2;
    let mut tokens = Vec::new();

    while pos < bytes.len() && tokens.len() < 3 {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos < bytes.len() && bytes[pos] == b'#' {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'#' {
            pos += 1;
        }
        if start < pos {
            tokens.push(std::str::from_utf8(&bytes[start..pos]).ok()?.to_string());
        }
    }

    if tokens.len() < 3 {
        return None;
    }

    let w: usize = tokens[0].parse().ok()?;
    let h: usize = tokens[1].parse().ok()?;
    let max_val: u32 = tokens[2].parse().ok()?;
    if w == 0 || h == 0 || max_val != 255 {
        return None;
    }

    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }

    if pos + w * h * 3 > bytes.len() {
        return None;
    }

    let mut pixels = Vec::with_capacity(w * h);
    for i in 0..(w * h) {
        let off = pos + i * 3;
        let r = bytes[off] as u32;
        let g = bytes[off + 1] as u32;
        let b = bytes[off + 2] as u32;
        pixels.push((r << 16) | (g << 8) | b);
    }

    Some(DecodedImage { width: w, height: h, pixels })
}

/// Encodes 24-bit uncompressed Windows BMP data from XRGB pixels.
pub fn encode_bmp(width: usize, height: usize, pixels: &[u32]) -> Vec<u8> {
    assert_eq!(pixels.len(), width * height, "pixel count must match dimensions");
    let row_stride = (width * 3).div_ceil(4) * 4;
    let image_size = row_stride * height;
    let file_size = 54 + image_size;
    let mut out = Vec::with_capacity(file_size);

    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    out.extend_from_slice(&54u32.to_le_bytes()); // pixel offset

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes()); // header size
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes()); // bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // compression BI_RGB
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // colors used
    out.extend_from_slice(&0u32.to_le_bytes()); // colors important

    let pad_len = row_stride - width * 3;
    let pad = [0u8; 4];

    // Rows bottom-up
    for row in (0..height).rev() {
        let row_start = row * width;
        for col in 0..width {
            let c = pixels[row_start + col];
            let r = ((c >> 16) & 0xFF) as u8;
            let g = ((c >> 8) & 0xFF) as u8;
            let b = (c & 0xFF) as u8;
            out.push(b);
            out.push(g);
            out.push(r);
        }
        out.extend_from_slice(&pad[..pad_len]);
    }

    out
}

impl<'a> Surface<'a> {
    /// Draws an image onto the surface at 1:1 scale at `(x, y)`, respecting clipping.
    pub fn draw_image(&mut self, x: i32, y: i32, img: &DecodedImage) {
        let img_rect = Rect::new(x, y, img.width as i32, img.height as i32);
        let clip = self.clip();
        let visible = img_rect.intersect(&clip);
        if visible.w <= 0 || visible.h <= 0 {
            return;
        }

        for dy in 0..visible.h {
            let sy = (visible.y + dy - y) as usize;
            let src_row = sy * img.width;
            let sx_start = (visible.x - x) as usize;
            for dx in 0..visible.w {
                let sx = sx_start + dx as usize;
                let c = img.pixels[src_row + sx];
                self.put(visible.x + dx, visible.y + dy, c);
            }
        }
    }

    /// Draws an image scaled to fill `dst` rectangle using nearest-neighbor sampling.
    pub fn draw_image_scaled(&mut self, dst: Rect, img: &DecodedImage) {
        let clip = self.clip();
        let visible = dst.intersect(&clip);
        if visible.w <= 0 || visible.h <= 0 || dst.w <= 0 || dst.h <= 0 || img.width == 0 || img.height == 0 {
            return;
        }

        for dy in 0..visible.h {
            let cur_y = visible.y + dy;
            let sy = (((cur_y - dst.y) as usize * img.height) / dst.h as usize).min(img.height - 1);
            let src_row = sy * img.width;

            for dx in 0..visible.w {
                let cur_x = visible.x + dx;
                let sx = (((cur_x - dst.x) as usize * img.width) / dst.w as usize).min(img.width - 1);
                let c = img.pixels[src_row + sx];
                self.put(cur_x, cur_y, c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_bmp_24bit() {
        // Construct a 2x2 24-bit BMP in memory:
        // Rows bottom-up: row 0 = red, green; row 1 = blue, white.
        let mut data = vec![
            b'B', b'M', 70, 0, 0, 0, // file size = 54 header + 16 pixel data = 70
            0, 0, 0, 0, 54, 0, 0, 0, // offset = 54
            40, 0, 0, 0, // header size = 40
            2, 0, 0, 0, // w = 2
            2, 0, 0, 0, // h = 2
            1, 0, // planes = 1
            24, 0, // bpp = 24
            0, 0, 0, 0, // compression = 0
            16, 0, 0, 0, // biSizeImage
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        // Row 0 (bottom): red (BGR: 0, 0, 255), green (BGR: 0, 255, 0) + 2 pad bytes
        data.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        // Row 1 (top): blue (BGR: 255, 0, 0), white (BGR: 255, 255, 255) + 2 pad bytes
        data.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);

        let img = decode_bmp(&data).expect("failed to decode BMP");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        // Top-left is blue (row 1, col 0)
        assert_eq!(img.pixels[0], 0x000000FF);
        // Top-right is white (row 1, col 1)
        assert_eq!(img.pixels[1], 0x00FFFFFF);
        // Bottom-left is red (row 0, col 0)
        assert_eq!(img.pixels[2], 0x00FF0000);
        // Bottom-right is green (row 0, col 1)
        assert_eq!(img.pixels[3], 0x0000FF00);
    }

    #[test]
    fn round_trip_bmp_encode_decode() {
        let pixels = vec![0x00FF0000, 0x0000FF00, 0x000000FF, 0x00FFFFFF];
        let bytes = encode_bmp(2, 2, &pixels);
        let dec = decode_bmp(&bytes).unwrap();
        assert_eq!(dec.width, 2);
        assert_eq!(dec.height, 2);
        assert_eq!(dec.pixels, pixels);
    }

    #[test]
    fn test_load_jpeg_or_png() {
        if let Ok(bytes) = std::fs::read("/home/nully/Pictures/anime-paranoia-agent.jpg") {
            let img = DecodedImage::decode(&bytes);
            assert!(img.is_some(), "should decode jpeg");
            let img = img.unwrap();
            assert!(img.width > 0 && img.height > 0);
            let thumb = DecodedImage::decode_thumbnail(&bytes, 32, 32);
            assert!(thumb.is_some(), "should decode thumbnail");
            let thumb = thumb.unwrap();
            assert_eq!(thumb.width, 32);
            assert_eq!(thumb.height, 32);
        }
    }

    #[test]
    fn generate_default_wallpapers() {
        let _ = std::fs::create_dir_all("../../heavy/wallpapers");

        // 1. Classic Teal Weave / Dither pattern (64x64)
        let mut pixels = Vec::with_capacity(64 * 64);
        for y in 0..64 {
            for x in 0..64 {
                let c = if (x + y) % 4 == 0 {
                    0x00A0A0
                } else if (x + y) % 2 == 0 {
                    0x008080
                } else if (x ^ y) % 4 == 1 {
                    0x006060
                } else {
                    0x008080
                };
                pixels.push(c);
            }
        }
        let bytes = encode_bmp(64, 64, &pixels);
        let _ = std::fs::write("../../heavy/wallpapers/ferro-classic.bmp", bytes);

        // 2. Retro Slate Grid / Mesh (64x64)
        let mut pixels = Vec::with_capacity(64 * 64);
        for y in 0..64 {
            for x in 0..64 {
                let c = if x % 8 == 0 || y % 8 == 0 {
                    0x2A343D
                } else if x % 4 == 0 || y % 4 == 0 {
                    0x1F2830
                } else {
                    0x182026
                };
                pixels.push(c);
            }
        }
        let bytes = encode_bmp(64, 64, &pixels);
        let _ = std::fs::write("../../heavy/wallpapers/ferro-mesh.bmp", bytes);
    }

    #[test]
    fn scales_image_to_surface() {
        let img = DecodedImage::new(2, 2, vec![0x111111, 0x222222, 0x333333, 0x444444]);
        let mut s = Surface::new(4, 4);
        s.draw_image_scaled(Rect::new(0, 0, 4, 4), &img);
        assert_eq!(s.pixels()[0], 0x111111);
        assert_eq!(s.pixels()[1], 0x111111);
        assert_eq!(s.pixels()[2], 0x222222);
        assert_eq!(s.pixels()[3], 0x222222);
    }
}
