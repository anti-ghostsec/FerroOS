//! Retro 3D Starfield simulation.

use ferro_gfx::{color, Rect, Surface};

pub const NUM_STARS: usize = 120;
pub const MAX_Z: i32 = 1000;
pub const SPEED: i32 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Star {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

pub struct Starfield {
    pub stars: Vec<Star>,
}

impl Default for Starfield {
    fn default() -> Self {
        Self::new(NUM_STARS)
    }
}

impl Starfield {
    pub fn new(count: usize) -> Self {
        let mut stars = Vec::with_capacity(count);
        let mut seed = 123456789u32;
        for i in 0..count {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let rx = ((seed % 2000) as i32) - 1000;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let ry = ((seed % 2000) as i32) - 1000;
            let rz = 100 + ((i as i32 * (MAX_Z - 100)) / count as i32);
            stars.push(Star { x: rx, y: ry, z: rz });
        }
        Self { stars }
    }

    pub fn tick(&mut self) {
        let mut seed = 987654321u32;
        for s in &mut self.stars {
            s.z -= SPEED;
            if s.z <= 10 {
                s.z = MAX_Z;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                s.x = ((seed % 2000) as i32) - 1000;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                s.y = ((seed % 2000) as i32) - 1000;
            }
        }
    }

    pub fn draw(&self, s: &mut Surface, w: i32, h: i32) {
        s.fill(Rect::new(0, 0, w, h), color::BLACK);

        let cx = w / 2;
        let cy = h / 2;
        let fov = (w.min(h) * 3) / 4;

        for star in &self.stars {
            if star.z <= 0 {
                continue;
            }
            let px = cx + (star.x * fov) / star.z;
            let py = cy + (star.y * fov) / star.z;

            if px >= 0 && px < w && py >= 0 && py < h {
                let brightness = 100 - (star.z * 100) / MAX_Z;
                let c = if brightness > 80 {
                    color::WHITE
                } else if brightness > 40 {
                    color::LIGHT
                } else {
                    color::SHADOW
                };

                s.put(px, py, c);
                if brightness > 75 && px + 1 < w && py + 1 < h {
                    s.put(px + 1, py, c);
                    s.put(px, py + 1, c);
                    s.put(px + 1, py + 1, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starfield_ticks_and_wraps() {
        let mut sf = Starfield::new(10);
        let initial_z = sf.stars[0].z;
        sf.tick();
        assert_eq!(sf.stars[0].z, initial_z - SPEED);

        // Advance until star wraps around
        for _ in 0..100 {
            sf.tick();
        }
        assert!(sf.stars.iter().all(|s| s.z > 0 && s.z <= MAX_Z));
    }
}
