//! Retro Green Phosphor Digital Rain simulation.

use ferro_gfx::{color, Rect, Surface, GLYPH_H, GLYPH_W};

pub struct Column {
    pub y: i32,
    pub speed: i32,
    pub length: i32,
}

pub struct MatrixRain {
    pub cols: Vec<Column>,
}

impl MatrixRain {
    pub fn new(w: i32) -> Self {
        let num_cols = (w / GLYPH_W).max(10) as usize;
        let mut cols = Vec::with_capacity(num_cols);
        let mut seed = 554433221u32;
        for _ in 0..num_cols {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let y = -((seed % 300) as i32);
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let speed = (seed % 8 + 6) as i32;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let length = (seed % 15 + 10) as i32;
            cols.push(Column { y, speed, length });
        }
        Self { cols }
    }

    pub fn tick(&mut self, h: i32) {
        let mut seed = 112233445u32;
        for c in &mut self.cols {
            c.y += c.speed;
            if c.y - c.length * GLYPH_H > h {
                c.y = 0;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                c.speed = (seed % 8 + 6) as i32;
            }
        }
    }

    pub fn draw(&self, s: &mut Surface, w: i32, h: i32) {
        s.fill(Rect::new(0, 0, w, h), color::BLACK);

        for (i, c) in self.cols.iter().enumerate() {
            let x = i as i32 * GLYPH_W;
            if x >= w {
                break;
            }

            for j in 0..c.length {
                let py = c.y - j * GLYPH_H;
                if py >= 0 && py < h {
                    let col = if j == 0 {
                        0xE0FFE0 // bright tip
                    } else if j < 4 {
                        0x00FF33 // bright phosphor
                    } else if j < 10 {
                        0x00AA22 // medium green
                    } else {
                        0x005511 // fading green
                    };
                    s.text(x, py, "1", col);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_rain_updates() {
        let mut mr = MatrixRain::new(800);
        let init_y = mr.cols[0].y;
        mr.tick(600);
        assert_eq!(mr.cols[0].y, init_y + mr.cols[0].speed);
    }
}
