//! Software rendering primitives and the Windows 95 look-and-feel.
//!
//! Everything draws into a plain `0x00RRGGBB` pixel buffer, so the same code
//! runs against a Linux framebuffer, a DRM dumb buffer or a desktop window.

use font8x8::legacy::BASIC_LEGACY;

pub mod icons;

/// The Windows 95 system palette.
pub mod color {
    pub const BLACK: u32 = 0x000000;
    pub const WHITE: u32 = 0xFFFFFF;
    pub const FACE: u32 = 0xC0C0C0;
    pub const LIGHT: u32 = 0xDFDFDF;
    pub const SHADOW: u32 = 0x808080;
    pub const DESKTOP: u32 = 0x008080;
    pub const NAVY: u32 = 0x000080;
    pub const RED: u32 = 0xC00000;
}

use color::*;

pub const GLYPH_W: i32 = 8;
pub const GLYPH_H: i32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }

    pub const fn inset(&self, d: i32) -> Self {
        Self::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }

    pub fn overlaps(&self, o: &Rect) -> bool {
        let i = self.intersect(o);
        i.w > 0 && i.h > 0
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        Rect::new(x0, y0, self.right().max(o.right()) - x0, self.bottom().max(o.bottom()) - y0)
    }
}

/// The 3D edge styles that define the Win95 look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bevel {
    /// Push button at rest.
    Raised,
    /// Push button held down, or a toggled taskbar button.
    Pressed,
    /// Outer frame of windows and menus.
    Window,
    /// Text fields and list panes.
    Sunken,
    /// One-pixel status-bar field (taskbar tray).
    Field,
}

enum Pixels<'a> {
    Owned(Vec<u32>),
    /// Someone else's memory, e.g. a mapped GPU scanout buffer.
    Borrowed(&'a mut [u32]),
    /// A 16-bit RGB565 scanout buffer; colors are converted as they're drawn.
    Borrowed565(&'a mut [u16]),
}

/// XRGB8888 to RGB565 (top 5/6/5 bits of each channel).
#[inline]
pub fn rgb565(c: u32) -> u16 {
    (((c >> 8) & 0xF800) | ((c >> 5) & 0x07E0) | ((c >> 3) & 0x001F)) as u16
}

/// A pixel buffer drawn with 32-bit XRGB colors. `stride` is the distance
/// between rows in pixels, which for GPU buffers can exceed `width`.
pub struct Surface<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pixels: Pixels<'a>,
    /// Drawing outside this rectangle is skipped (damage-limited redraws).
    clip: Rect,
}

impl Surface<'static> {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, stride: width, pixels: Pixels::Owned(vec![0; width * height]), clip: Rect::new(0, 0, width as i32, height as i32) }
    }
}

impl<'a> Surface<'a> {
    /// Draws directly into `buf`, so no private copy of the screen exists.
    pub fn borrowed(buf: &'a mut [u32], width: usize, height: usize, stride: usize) -> Self {
        assert!(stride >= width && buf.len() >= stride * (height.max(1) - 1) + width, "buffer too small");
        Self { width, height, stride, pixels: Pixels::Borrowed(buf), clip: Rect::new(0, 0, width as i32, height as i32) }
    }

    /// Like [`Surface::borrowed`], for a 16-bit RGB565 buffer.
    pub fn borrowed_565(buf: &'a mut [u16], width: usize, height: usize, stride: usize) -> Self {
        assert!(stride >= width && buf.len() >= stride * (height.max(1) - 1) + width, "buffer too small");
        Self { width, height, stride, pixels: Pixels::Borrowed565(buf), clip: Rect::new(0, 0, width as i32, height as i32) }
    }

    /// Restricts drawing to `r` (or the whole surface for `None`).
    pub fn set_clip(&mut self, r: Option<Rect>) {
        let full = Rect::new(0, 0, self.width as i32, self.height as i32);
        self.clip = r.map_or(full, |r| r.intersect(&full));
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// The 32-bit pixels (empty for a 16-bit surface).
    pub fn pixels(&self) -> &[u32] {
        match &self.pixels {
            Pixels::Owned(v) => v,
            Pixels::Borrowed(b) => b,
            Pixels::Borrowed565(_) => &[],
        }
    }

    pub fn clear(&mut self, c: u32) {
        let (w, h) = (self.width as i32, self.height as i32);
        self.fill(Rect::new(0, 0, w, h), c);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: u32) {
        if self.clip.contains(x, y) {
            let i = y as usize * self.stride + x as usize;
            match &mut self.pixels {
                Pixels::Owned(v) => v[i] = c,
                Pixels::Borrowed(b) => b[i] = c,
                Pixels::Borrowed565(b) => b[i] = rgb565(c),
            }
        }
    }

    pub fn fill(&mut self, r: Rect, c: u32) {
        let r = r.intersect(&self.clip);
        let x0 = r.x.max(0) as usize;
        let y0 = r.y.max(0) as usize;
        let x1 = r.right().clamp(0, self.width as i32) as usize;
        let y1 = r.bottom().clamp(0, self.height as i32) as usize;
        if x0 >= x1 {
            return;
        }
        let stride = self.stride;
        let rows = (y0..y1).map(|y| y * stride + x0..y * stride + x1);
        match &mut self.pixels {
            Pixels::Owned(v) => rows.for_each(|r| v[r].fill(c)),
            Pixels::Borrowed(b) => rows.for_each(|r| b[r].fill(c)),
            Pixels::Borrowed565(b) => {
                let c = rgb565(c);
                rows.for_each(|r| b[r].fill(c));
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, c: u32) {
        self.fill(Rect::new(x, y, w, 1), c);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, c: u32) {
        self.fill(Rect::new(x, y, 1, h), c);
    }

    /// Bresenham line, endpoints inclusive.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u32) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Two-tone edge; bottom/right are drawn last so they own the corners.
    fn edge(&mut self, r: Rect, top_left: u32, bottom_right: u32) {
        self.hline(r.x, r.y, r.w, top_left);
        self.vline(r.x, r.y, r.h, top_left);
        self.hline(r.x, r.bottom() - 1, r.w, bottom_right);
        self.vline(r.right() - 1, r.y, r.h, bottom_right);
    }

    pub fn bevel(&mut self, r: Rect, style: Bevel) {
        let (outer, inner) = match style {
            Bevel::Raised => ((WHITE, BLACK), Some((LIGHT, SHADOW))),
            Bevel::Pressed => ((BLACK, WHITE), Some((SHADOW, LIGHT))),
            Bevel::Window => ((LIGHT, BLACK), Some((WHITE, SHADOW))),
            Bevel::Sunken => ((SHADOW, WHITE), Some((BLACK, LIGHT))),
            Bevel::Field => ((SHADOW, WHITE), None),
        };
        self.edge(r, outer.0, outer.1);
        if let Some((tl, br)) = inner {
            self.edge(r.inset(1), tl, br);
        }
    }

    /// A face-colored push button with its bevel.
    pub fn button(&mut self, r: Rect, pressed: bool) {
        self.fill(r, FACE);
        self.bevel(r, if pressed { Bevel::Pressed } else { Bevel::Raised });
    }

    /// Two-pixel etched line used for menu and dialog separators.
    pub fn etched_hline(&mut self, x: i32, y: i32, w: i32) {
        self.hline(x, y, w, SHADOW);
        self.hline(x, y + 1, w, WHITE);
    }

    /// Etched frame with a caption cut into its top edge.
    pub fn group_box(&mut self, r: Rect, label: &str) {
        self.edge(Rect::new(r.x + 1, r.y + 1, r.w - 1, r.h - 1), WHITE, WHITE);
        self.edge(Rect::new(r.x, r.y, r.w - 1, r.h - 1), SHADOW, SHADOW);
        let lw = text_width(label) + 4;
        self.fill(Rect::new(r.x + 6, r.y - 4, lw, 10), FACE);
        self.text(r.x + 8, r.y - 3, label, BLACK);
    }

    pub fn scrollbar(&mut self, sb: &Scrollbar, up_held: bool, down_held: bool) {
        // Track: the classic white/face checkerboard.
        let t = sb.track;
        for y in t.y.max(0)..t.bottom() {
            for x in t.x..t.right() {
                self.put(x, y, if (x + y) % 2 == 0 { WHITE } else { FACE });
            }
        }
        for (r, glyph, held) in [(sb.up, icons::ARROW_UP, up_held), (sb.down, icons::ARROW_DOWN, down_held)] {
            self.button(r, held);
            let off = held as i32;
            self.sprite_mono(r.x + (r.w - 7) / 2 + off, r.y + (r.h - 4) / 2 + off, glyph, BLACK);
        }
        if sb.thumb.h > 0 {
            self.button(sb.thumb, false);
        }
    }

    /// Draws `s` with its top-left at (x, y); returns the advance in pixels.
    pub fn text(&mut self, x: i32, y: i32, s: &str, c: u32) -> i32 {
        let mut cx = x;
        let clip = self.clip;
        for ch in s.chars() {
            // Skip glyphs entirely outside the clip (cheap partial redraws).
            if !Rect::new(cx, y, GLYPH_W, GLYPH_H).overlaps(&clip) {
                cx += GLYPH_W;
                continue;
            }
            for (row, bits) in glyph(ch).iter().enumerate() {
                for col in 0..8 {
                    if bits >> col & 1 == 1 {
                        self.put(cx + col, y + row as i32, c);
                    }
                }
            }
            cx += GLYPH_W;
        }
        cx - x
    }

    pub fn text_bold(&mut self, x: i32, y: i32, s: &str, c: u32) -> i32 {
        self.text(x + 1, y, s, c);
        self.text(x, y, s, c) + 1
    }

    /// Grayed-out text with the Win95 etched highlight.
    pub fn text_disabled(&mut self, x: i32, y: i32, s: &str) -> i32 {
        self.text(x + 1, y + 1, s, WHITE);
        self.text(x, y, s, SHADOW)
    }

    /// Text rotated 90 degrees counter-clockwise, reading bottom to top,
    /// starting at (x, bottom). Used for the Start menu banner.
    pub fn text_vertical(&mut self, x: i32, bottom: i32, s: &str, c: u32) -> i32 {
        let mut cy = bottom;
        for ch in s.chars() {
            for (row, bits) in glyph(ch).iter().enumerate() {
                for col in 0..8 {
                    if bits >> col & 1 == 1 {
                        self.put(x + row as i32, cy - col, c);
                    }
                }
            }
            cy -= GLYPH_W;
        }
        bottom - cy
    }

    /// Draws a character-art sprite (see [`icons`]) at `scale`x.
    pub fn sprite(&mut self, x: i32, y: i32, rows: &[&str], scale: i32) {
        let w = rows.first().map_or(0, |r| r.len() as i32);
        if !Rect::new(x, y, w * scale, rows.len() as i32 * scale).overlaps(&self.clip) {
            return;
        }
        for (j, row) in rows.iter().enumerate() {
            for (i, ch) in row.bytes().enumerate() {
                if let Some(c) = icons::palette(ch) {
                    self.fill(Rect::new(x + i as i32 * scale, y + j as i32 * scale, scale, scale), c);
                }
            }
        }
    }

    /// Draws every non-transparent pixel of a sprite in a single color.
    pub fn sprite_mono(&mut self, x: i32, y: i32, rows: &[&str], c: u32) {
        for (j, row) in rows.iter().enumerate() {
            for (i, ch) in row.bytes().enumerate() {
                if ch != b'.' {
                    self.put(x + i as i32, y + j as i32, c);
                }
            }
        }
    }
}

/// Geometry of a Win95 vertical scrollbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scrollbar {
    pub up: Rect,
    pub down: Rect,
    pub track: Rect,
    /// Zero-height when everything fits.
    pub thumb: Rect,
}

pub const SCROLLBAR_W: i32 = 16;

impl Scrollbar {
    /// `pos` is the first visible row, `page` the visible row count.
    pub fn layout(r: Rect, pos: usize, page: usize, total: usize) -> Self {
        let up = Rect::new(r.x, r.y, r.w, SCROLLBAR_W);
        let down = Rect::new(r.x, r.bottom() - SCROLLBAR_W, r.w, SCROLLBAR_W);
        let track = Rect::new(r.x, up.bottom(), r.w, (down.y - up.bottom()).max(0));
        let thumb = if total > page && track.h > 8 {
            let h = (track.h as usize * page / total).max(8) as i32;
            let y = track.y + ((track.h - h) as usize * pos / (total - page)) as i32;
            Rect::new(r.x, y, r.w, h)
        } else {
            Rect::default()
        };
        Self { up, down, track, thumb }
    }
}

fn glyph(ch: char) -> &'static [u8; 8] {
    let i = ch as usize;
    &BASIC_LEGACY[if i < 128 { i } else { b'?' as usize }]
}

pub fn text_width(s: &str) -> i32 {
    s.chars().count() as i32 * GLYPH_W
}

/// Truncates `s` to `max` characters, ending in "..." when shortened.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let mut out: String = s.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

/// Greedy word wrap to `max` characters per line; long words are broken.
pub fn wrap(s: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let fits = cur.is_empty() || cur.chars().count() + 1 + word.chars().count() <= max;
        if !fits {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        while cur.chars().count() > max {
            let head: String = cur.chars().take(max).collect();
            cur = cur.chars().skip(max).collect();
            lines.push(head);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_clips_to_surface() {
        let mut s = Surface::new(4, 4);
        s.fill(Rect::new(-2, -2, 4, 4), WHITE);
        s.fill(Rect::new(3, 3, 10, 10), RED);
        assert_eq!(s.pixels()[0], WHITE);
        assert_eq!(s.pixels()[5], WHITE);
        assert_eq!(s.pixels()[2], 0);
        assert_eq!(s.pixels()[15], RED);
    }

    #[test]
    fn clip_limits_drawing() {
        let mut s = Surface::new(8, 8);
        s.set_clip(Some(Rect::new(2, 2, 2, 2)));
        s.clear(WHITE);
        s.text(0, 0, "MM", RED);
        let painted = s.pixels().iter().filter(|&&p| p != 0).count();
        assert_eq!(painted, 4, "only the 2x2 clip area changes");
        s.set_clip(None);
        assert_eq!(s.clip(), Rect::new(0, 0, 8, 8));
    }

    #[test]
    fn borrowed_surface_respects_stride() {
        let mut buf = vec![0u32; 6 * 2]; // 4 visible pixels per 6-pixel row
        let mut s = Surface::borrowed(&mut buf, 4, 2, 6);
        s.clear(WHITE);
        s.put(3, 1, RED);
        drop(s);
        assert_eq!(&buf[..6], &[WHITE, WHITE, WHITE, WHITE, 0, 0]);
        assert_eq!(buf[6 + 3], RED);
        assert_eq!(buf[6 + 4], 0, "padding untouched");
    }

    #[test]
    fn text_helpers() {
        assert_eq!(ellipsize("My Computer", 6), "My ...");
        assert_eq!(ellipsize("abc", 6), "abc");
        assert_eq!(wrap("About FerroOS", 9), ["About", "FerroOS"]);
        assert_eq!(wrap("Recycle Bin", 11), ["Recycle Bin"]);
        assert_eq!(wrap("abcdefghijk", 4), ["abcd", "efgh", "ijk"]);
    }

    #[test]
    fn draws_into_rgb565_buffers() {
        assert_eq!(rgb565(0x00FF_FFFF), 0xFFFF);
        assert_eq!(rgb565(0x0000_8080), 0x0410); // Win95 desktop teal
        let mut buf = vec![0u16; 8 * 4];
        let mut s = Surface::borrowed_565(&mut buf, 6, 4, 8);
        s.fill(Rect::new(1, 1, 2, 2), 0x00FF_0000);
        s.put(5, 3, 0x0000_00FF);
        drop(s);
        assert_eq!(buf[8 + 1], 0xF800);
        assert_eq!(buf[2 * 8 + 2], 0xF800);
        assert_eq!(buf[3 * 8 + 5], 0x001F);
        assert_eq!(buf[0], 0);
        assert_eq!(buf[6], 0, "stride padding untouched");
    }
}
