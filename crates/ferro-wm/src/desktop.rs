//! The desktop surface: draws the two desktop icons (Files and Recycle Bin)
//! and handles hit testing on the root window.

use ferro_gfx::{color, icons, text_width, Rect, Surface, GLYPH_H};

pub const ICON_W: i32 = 76;
pub const ICON_H: i32 = 64;
pub const ICON_SIZE: i32 = 32;

pub const FILES: Rect = Rect::new(20, 20, ICON_W, ICON_H);
pub const RECYCLE_BIN: Rect = Rect::new(20, 96, ICON_W, ICON_H);

/// Bounding rectangle covering both desktop icons.
pub const ICONS_BOUNDS: Rect = Rect::new(10, 10, 96, 170);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopHit {
    Files,
    RecycleBin,
    Background,
}

pub fn hit(x: i32, y: i32) -> DesktopHit {
    if FILES.contains(x, y) {
        DesktopHit::Files
    } else if RECYCLE_BIN.contains(x, y) {
        DesktopHit::RecycleBin
    } else {
        DesktopHit::Background
    }
}

/// Draws the desktop icons onto a full-screen surface.
#[cfg(test)]
pub fn draw(s: &mut Surface, w: i32, h: i32, selected: Option<DesktopHit>, bg_color: u32) {
    s.fill(Rect::new(0, 0, w, h), bg_color);
    draw_icon(s, FILES, icons::FOLDER, "Files", selected == Some(DesktopHit::Files));
    draw_icon(s, RECYCLE_BIN, icons::RECYCLE, "Recycle Bin", selected == Some(DesktopHit::RecycleBin));
}

/// Draws only the icons region onto an `ICONS_BOUNDS.w x ICONS_BOUNDS.h` surface.
pub fn draw_icons_area(s: &mut Surface, selected: Option<DesktopHit>, bg_color: u32) {
    s.fill(Rect::new(0, 0, ICONS_BOUNDS.w, ICONS_BOUNDS.h), bg_color);
    let files_rel = Rect::new(FILES.x - ICONS_BOUNDS.x, FILES.y - ICONS_BOUNDS.y, FILES.w, FILES.h);
    let bin_rel = Rect::new(RECYCLE_BIN.x - ICONS_BOUNDS.x, RECYCLE_BIN.y - ICONS_BOUNDS.y, RECYCLE_BIN.w, RECYCLE_BIN.h);
    draw_icon(s, files_rel, icons::FOLDER, "Files", selected == Some(DesktopHit::Files));
    draw_icon(s, bin_rel, icons::RECYCLE, "Recycle Bin", selected == Some(DesktopHit::RecycleBin));
}

fn draw_icon(s: &mut Surface, r: Rect, icon: icons::Icon, label: &str, selected: bool) {
    s.sprite(r.x + (r.w - ICON_SIZE) / 2, r.y + 2, icon, 2);
    let lw = text_width(label);
    let lx = r.x + (r.w - lw) / 2;
    let ly = r.y + ICON_SIZE + 6;
    let label_rect = Rect::new(lx - 2, ly - 2, lw + 4, GLYPH_H + 4);
    if selected {
        s.fill(label_rect, color::NAVY);
        s.text(lx, ly, label, color::WHITE);
    } else {
        s.text(lx + 1, ly + 1, label, color::BLACK); // drop shadow for readability
        s.text(lx, ly, label, color::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_identifies_both_icons() {
        assert_eq!(hit(25, 25), DesktopHit::Files);
        assert_eq!(hit(25, 105), DesktopHit::RecycleBin);
        assert_eq!(hit(200, 200), DesktopHit::Background);
    }

    #[test]
    fn draws_desktop_with_icons() {
        let mut s = Surface::new(200, 200);
        draw(&mut s, 200, 200, None, color::DESKTOP);
        let pixels = s.pixels();
        assert!(pixels.iter().any(|p| *p == color::DESKTOP));
        assert!(pixels.iter().any(|p| *p == color::WHITE));
    }

    #[test]
    fn draws_icons_area_correctly() {
        let mut s = Surface::new(ICONS_BOUNDS.w as usize, ICONS_BOUNDS.h as usize);
        draw_icons_area(&mut s, Some(DesktopHit::Files), color::DESKTOP);
        let pixels = s.pixels();
        assert!(pixels.iter().any(|p| *p == color::NAVY));
    }
}
