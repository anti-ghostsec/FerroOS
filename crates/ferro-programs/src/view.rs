//! A text file to read, in a window with a scroll bar (Help).
//!
//!   ferro-programs --view TITLE FILE

use crate::proto;
use crate::win::Win;
use ferro_gfx::{color, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, Key, Res};
use x11rb::connection::Connection;
use x11rb::protocol::Event;

const W: i32 = 680;
const H: i32 = 500;
const TEXT: Rect = Rect::new(12, 12, W - 24 - 16, H - 60);
const SCROLL: Rect = Rect::new(TEXT.x + TEXT.w, TEXT.y, 16, TEXT.h);
const LINE_H: i32 = GLYPH_H + 4;
const CLOSE_BTN: Rect = Rect::new(W - 87, H - 36, 75, 24);

fn rows() -> usize {
    ((TEXT.h - 8) / LINE_H) as usize
}

/// The scroll bar's thumb, for `top` of `total` lines with `rows` shown.
fn thumb(top: usize, total: usize, rows: usize) -> Rect {
    let track = Rect::new(SCROLL.x, SCROLL.y + 16, SCROLL.w, SCROLL.h - 32);
    if total <= rows {
        return track;
    }
    let h = ((track.h as usize * rows / total) as i32).max(12);
    let y = track.y + ((track.h - h) as usize * top / (total - rows)) as i32;
    Rect::new(track.x, y, track.w, h)
}

fn arrow(s: &mut Surface, r: Rect, up: bool) {
    s.fill(r, color::FACE);
    s.bevel(r, Bevel::Raised);
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    for i in 0..4 {
        let y = if up { cy - 2 + i } else { cy + 1 - i };
        s.hline(cx - i, y, 2 * i + 1, color::BLACK);
    }
}

fn draw(s: &mut Surface, lines: &[String], top: usize) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.fill(TEXT, color::WHITE);
    s.bevel(Rect::new(TEXT.x, TEXT.y, TEXT.w + SCROLL.w, TEXT.h), Bevel::Sunken);
    for (n, l) in lines.iter().skip(top).take(rows()).enumerate() {
        s.text(TEXT.x + 6, TEXT.y + 5 + n as i32 * LINE_H, l, color::BLACK);
    }
    // Windows 95's scroll bar: a dotted track, the thumb, an arrow each end.
    let track = Rect::new(SCROLL.x, SCROLL.y + 16, SCROLL.w, SCROLL.h - 32);
    for y in track.y..track.bottom() {
        for x in track.x..track.right() {
            s.fill(Rect::new(x, y, 1, 1), if (x + y) % 2 == 0 { color::WHITE } else { color::LIGHT });
        }
    }
    let t = thumb(top, lines.len(), rows());
    s.fill(t, color::FACE);
    s.bevel(t, Bevel::Raised);
    arrow(s, Rect::new(SCROLL.x, SCROLL.y, 16, 16), true);
    arrow(s, Rect::new(SCROLL.x, SCROLL.bottom() - 16, 16, 16), false);
    button(s, CLOSE_BTN, "Close", true);
}

pub fn run(title: &str, file: &str) -> Res<()> {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| format!("{file} couldn't be read: {e}."));
    let cols = ((TEXT.w - 12) / GLYPH_W) as usize;
    let lines: Vec<String> = text.lines().flat_map(|l| proto::wrap(&proto::printable(l), cols)).collect();
    let most = lines.len().saturating_sub(rows());
    let w = Win::open(title, W, H)?;
    let mut s = Surface::new(W as usize, H as usize);
    let mut top = 0usize;
    loop {
        let before = top;
        match w.conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => {}
            Event::MapNotify(_) => {
                w.focus()?;
                continue;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == w.delete => return Ok(()),
            Event::ButtonPress(b) => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                match b.detail {
                    4 => top = top.saturating_sub(3),
                    5 => top = (top + 3).min(most),
                    1 if CLOSE_BTN.contains(x, y) => return Ok(()),
                    1 if SCROLL.contains(x, y) => {
                        let t = thumb(top, lines.len(), rows());
                        top = if y < SCROLL.y + 16 {
                            top.saturating_sub(1)
                        } else if y >= SCROLL.bottom() - 16 {
                            (top + 1).min(most)
                        } else if y < t.y {
                            top.saturating_sub(rows())
                        } else if y >= t.bottom() {
                            (top + rows()).min(most)
                        } else {
                            top
                        };
                    }
                    _ => continue,
                }
                if top == before {
                    continue;
                }
            }
            Event::KeyPress(k) => {
                let key = w.keymap.typed(&k);
                match key {
                    Key::Escape | Key::Enter => return Ok(()),
                    Key::Up => top = top.saturating_sub(1),
                    Key::Down => top = (top + 1).min(most),
                    Key::Home => top = 0,
                    Key::End => top = most,
                    // Page Up / Page Down, and Space for a page on.
                    Key::Char(' ') => top = (top + rows()).min(most),
                    Key::Other => match k.detail {
                        112 => top = top.saturating_sub(rows()),
                        117 => top = (top + rows()).min(most),
                        _ => continue,
                    },
                    _ => continue,
                }
                if top == before {
                    continue;
                }
            }
            _ => continue,
        }
        draw(&mut s, &lines, top);
        w.show(&s)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_goes_from_top_to_bottom() {
        let track_top = SCROLL.y + 16;
        let track_bottom = SCROLL.bottom() - 16;
        assert_eq!(thumb(0, 500, 30).y, track_top);
        assert_eq!(thumb(470, 500, 30).bottom(), track_bottom);
        assert_eq!(thumb(0, 10, 30).h, SCROLL.h - 32);
    }
}
