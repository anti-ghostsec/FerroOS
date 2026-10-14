//! What a window's frame looks like and where its parts are: the Windows 95
//! border, title bar and caption buttons, drawn with ferro-gfx. Pure
//! geometry and pixels, so it's the same in tests as on screen.

use ferro_gfx::{color, ellipsize, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

/// The raised border around a window.
pub const BORDER: i32 = 4;
pub const TITLE_H: i32 = 28;
/// Caption buttons are 22 x 20.
const BUTTON_W: i32 = 22;
const BUTTON_H: i32 = 20;
/// How much of a corner resizes both ways.
const CORNER: i32 = 16;

/// Space the frame takes around a program's own window.
pub const LEFT: i32 = BORDER;
pub const TOP: i32 = BORDER + TITLE_H + 1;
pub const RIGHT: i32 = BORDER;
pub const BOTTOM: i32 = BORDER;

/// What a point on the frame does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Title,
    Close,
    Maximize,
    Minimize,
    /// An edge or corner: which sides move (left, top, right, bottom).
    Resize(bool, bool, bool, bool),
    /// The program's own area.
    Client,
}

/// The size of the frame around a program window of this size.
pub fn outer(client_w: i32, client_h: i32) -> (i32, i32) {
    (client_w + LEFT + RIGHT, client_h + TOP + BOTTOM)
}

/// The size left for the program inside a frame of this size.
pub fn inner(frame_w: i32, frame_h: i32) -> (i32, i32) {
    ((frame_w - LEFT - RIGHT).max(1), (frame_h - TOP - BOTTOM).max(1))
}

fn title_rect(w: i32) -> Rect {
    Rect::new(BORDER, BORDER, w - 2 * BORDER, TITLE_H)
}

pub const RESIZE_BORDER: i32 = 6;

pub fn button_rect(w: i32, from_right: i32) -> Rect {
    let t = title_rect(w);
    // Close stands apart from the other two by two pixels.
    let gap = if from_right > 0 { 2 } else { 0 };
    let btn_y = t.y + (t.h - BUTTON_H) / 2;
    Rect::new(t.right() - 4 - BUTTON_W * (from_right + 1) - gap, btn_y, BUTTON_W, BUTTON_H)
}

/// Which part of a `w` x `h` frame the point is on.
pub fn hit(w: i32, h: i32, x: i32, y: i32, resizable: bool) -> Part {
    if button_rect(w, 0).contains(x, y) {
        return Part::Close;
    }
    if button_rect(w, 1).contains(x, y) {
        return Part::Maximize;
    }
    if button_rect(w, 2).contains(x, y) {
        return Part::Minimize;
    }
    let on_edge = x < RESIZE_BORDER || y < RESIZE_BORDER || x >= w - RESIZE_BORDER || y >= h - RESIZE_BORDER;
    if on_edge && resizable {
        let (l, t) = (x < CORNER, y < CORNER);
        let (r, b) = (x >= w - CORNER, y >= h - CORNER);
        return Part::Resize(
            l && (x < RESIZE_BORDER || t || b),
            t && (y < RESIZE_BORDER || l || r),
            r && (x >= w - RESIZE_BORDER || t || b),
            b && (y >= h - RESIZE_BORDER || l || r),
        );
    }
    if title_rect(w).contains(x, y) || on_edge {
        return Part::Title;
    }
    Part::Client
}

const GLYPH_CLOSE: [&str; 7] = ["##....##", ".##..##.", "..####..", "...##...", "..####..", ".##..##.", "##....##"];
const GLYPH_MAX: [&str; 9] = ["#########", "#########", "#.......#", "#.......#", "#.......#", "#.......#", "#.......#", "#.......#", "#########"];
const GLYPH_RESTORE: [&str; 9] =
    ["..#######", "..#######", "..#.....#", "#######.#", "#######.#", "#.....###", "#.....#..", "#.....#..", "#######.."];
const GLYPH_MIN: [&str; 2] = ["######", "######"];

fn caption_button(s: &mut Surface, r: Rect, glyph: &[&str], pressed: bool, low: bool) {
    s.button(r, pressed);
    let gw = glyph[0].len() as i32;
    let gh = glyph.len() as i32;
    let nudge = i32::from(pressed);
    let y = if low { r.bottom() - 4 - gh } else { r.y + (r.h - gh) / 2 };
    s.sprite_mono(r.x + (r.w - gw) / 2 + nudge, y + nudge, glyph, color::BLACK);
}

/// What the frame is showing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look<'a> {
    pub title: &'a str,
    /// The window with the keyboard: navy title bar, others grey.
    pub active: bool,
    pub maximized: bool,
    /// The caption button being held down, if any.
    pub pressed: Option<Part>,
    pub dark_mode: bool,
}

/// Draws the frame of a `w` x `h` window. The middle (the program's area)
/// is left as face colour; the program covers it.
pub fn draw(s: &mut Surface, w: i32, h: i32, look: &Look) {
    let all = Rect::new(0, 0, w, h);
    let face = if look.dark_mode { 0x282828 } else { color::FACE };
    s.fill(all, face);
    s.bevel(all, Bevel::Window);
    let t = title_rect(w);
    let title_bg = if look.active {
        if look.dark_mode {
            0x1E3A5F
        } else {
            color::NAVY
        }
    } else if look.dark_mode {
        0x181818
    } else {
        color::SHADOW
    };
    s.fill(t, title_bg);
    let room = ((button_rect(w, 2).x - t.x - 6) / GLYPH_W).max(0) as usize;
    let text = ellipsize(look.title, room);
    let title_fg = if look.active {
        color::WHITE
    } else if look.dark_mode {
        0xA0A0A0
    } else {
        color::FACE
    };
    s.text_bold(t.x + 6, t.y + (TITLE_H - GLYPH_H) / 2, &text, title_fg);
    let held = |p: Part| look.pressed == Some(p);
    caption_button(s, button_rect(w, 0), &GLYPH_CLOSE, held(Part::Close), false);
    caption_button(s, button_rect(w, 1), if look.maximized { &GLYPH_RESTORE } else { &GLYPH_MAX }, held(Part::Maximize), false);
    caption_button(s, button_rect(w, 2), &GLYPH_MIN, held(Part::Minimize), true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_and_client_sizes_round_trip() {
        let (w, h) = outer(640, 480);
        assert_eq!((w, h), (648, 517));
        assert_eq!(inner(w, h), (640, 480));
        assert_eq!(inner(2, 2), (1, 1));
    }

    #[test]
    fn finds_the_parts() {
        let (w, h) = (300, 200);
        assert_eq!(hit(w, h, 100, 14, true), Part::Title);
        assert_eq!(hit(w, h, 150, 100, true), Part::Client);
        assert_eq!(hit(w, h, w - 16, 14, true), Part::Close);
        assert_eq!(hit(w, h, w - 40, 14, true), Part::Maximize);
        assert_eq!(hit(w, h, w - 64, 14, true), Part::Minimize);
        // Edges move one side, corners two.
        assert_eq!(hit(w, h, 1, 100, true), Part::Resize(true, false, false, false));
        assert_eq!(hit(w, h, 150, h - 1, true), Part::Resize(false, false, false, true));
        assert_eq!(hit(w, h, w - 1, h - 1, true), Part::Resize(false, false, true, true));
        assert_eq!(hit(w, h, w - 10, h - 1, true), Part::Resize(false, false, true, true));
        assert_eq!(hit(w, h, 1, 1, true), Part::Resize(true, true, false, false));
        // A window that can't be resized is dragged by its border instead.
        assert_eq!(hit(w, h, 1, 100, false), Part::Title);
    }

    #[test]
    fn active_title_bar_is_navy() {
        let (w, h) = (200, 100);
        let mut s = Surface::new(w as usize, h as usize);
        draw(&mut s, w, h, &Look { title: "Notepad", active: true, ..Look::default() });
        assert_eq!(s.pixels()[(BORDER + 1) as usize * w as usize + 100], color::NAVY);
        draw(&mut s, w, h, &Look { title: "Notepad", ..Look::default() });
        assert_eq!(s.pixels()[(BORDER + 1) as usize * w as usize + 100], color::SHADOW);
        // The program's area is plain face colour.
        assert_eq!(s.pixels()[60 * w as usize + 100], color::FACE);

        // Dark mode frame test
        draw(&mut s, w, h, &Look { title: "Notepad", active: true, dark_mode: true, ..Look::default() });
        assert_eq!(s.pixels()[(BORDER + 1) as usize * w as usize + 100], 0x1E3A5F);
    }
}
