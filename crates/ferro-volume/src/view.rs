//! Retro Windows 95 Volume Control multi-channel mixer view and hit-testing.
//! Supports ALSA Master and selective per-app audio streams (Browser, Video Player, etc.) via MPRIS.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const COL_W: i32 = 94;
pub const PAD_X: i32 = 8;
pub const GAP_X: i32 = 6;
pub const H: i32 = 236;

pub const TRACK_OFFSET_X: i32 = 44;
pub const TRACK_Y: i32 = 42;
pub const TRACK_H: i32 = 96;
pub const TRACK_W: i32 = 6;

pub const KNOB_W: i32 = 24;
pub const KNOB_H: i32 = 14;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Master,
    Mpris { dest: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelState {
    pub kind: ChannelKind,
    pub label: String,
    pub sublabel: String,
    pub volume: u8,
    pub muted: bool,
    pub saved_volume: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub channels: Vec<ChannelState>,
    pub selected_col: usize,
    pub dragging: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            channels: vec![ChannelState {
                kind: ChannelKind::Master,
                label: "Volume".to_string(),
                sublabel: "Master".to_string(),
                volume: 75,
                muted: false,
                saved_volume: 75,
            }],
            selected_col: 0,
            dragging: None,
        }
    }
}

pub fn window_width(num_cols: usize) -> i32 {
    let n = num_cols.max(1) as i32;
    (PAD_X * 2 + n * COL_W + (n - 1) * GAP_X).max(180)
}

pub fn col_x(i: usize, num_cols: usize, total_w: i32) -> i32 {
    if num_cols <= 1 {
        (total_w - COL_W) / 2
    } else {
        PAD_X + (i as i32) * (COL_W + GAP_X)
    }
}

pub fn knob_y(volume: u8) -> i32 {
    let vol = volume.min(100) as i32;
    let travel = TRACK_H - KNOB_H;
    TRACK_Y + travel - (vol * travel) / 100
}

pub fn volume_from_y(y: i32) -> u8 {
    let travel = TRACK_H - KNOB_H;
    let clamped = (y - TRACK_Y).clamp(0, travel);
    let inverted = travel - clamped;
    ((inverted * 100 + travel / 2) / travel).clamp(0, 100) as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Slider(usize, u8),
    ToggleMute(usize),
    Close,
    SelectCol(usize),
    Nothing,
}

pub fn hit(x: i32, y: i32, w: i32, state: &State) -> Hit {
    let num_cols = state.channels.len();

    // Check OK button (located in Master column)
    if num_cols > 0 {
        let m_cx = col_x(0, num_cols, w);
        let ok_rect = Rect::new(m_cx + (COL_W - 54) / 2, 198, 54, 22);
        if ok_rect.contains(x, y) {
            return Hit::Close;
        }
    }

    for (i, _ch) in state.channels.iter().enumerate() {
        let cx = col_x(i, num_cols, w);
        let col_rect = Rect::new(cx, 8, COL_W, 218);
        if !col_rect.contains(x, y) {
            continue;
        }

        // Mute checkbox area
        let mute_area = Rect::new(cx + 6, 168, COL_W - 12, 18);
        if mute_area.contains(x, y) {
            return Hit::ToggleMute(i);
        }

        // Slider track / knob area
        let track_x = cx + TRACK_OFFSET_X;
        let track_hit_area = Rect::new(track_x - 14, TRACK_Y - 4, TRACK_W + 28, TRACK_H + 8);
        if track_hit_area.contains(x, y) {
            return Hit::Slider(i, volume_from_y(y));
        }

        // Clicking anywhere else in the column selects it
        return Hit::SelectCol(i);
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    let num_cols = state.channels.len();
    for (i, ch) in state.channels.iter().enumerate() {
        let cx = col_x(i, num_cols, w);
        let is_selected = i == state.selected_col;

        // Group box
        let group = Rect::new(cx, 8, COL_W, 218);
        s.bevel(group, Bevel::Sunken);

        // Group Title plate
        let title_w = (COL_W - 14).min(ch.label.len() as i32 * GLYPH_W + 24);
        let title_rect = Rect::new(cx + (COL_W - title_w) / 2, 3, title_w, GLYPH_H + 4);
        s.fill(title_rect, color::FACE);

        // Icon + Label
        let icon = match ch.kind {
            ChannelKind::Master => icons::SPEAKER,
            ChannelKind::Mpris { .. } if ch.label.contains("Browser") => icons::BROWSER,
            _ => icons::SPEAKER,
        };
        s.sprite(title_rect.x + 2, 4, icon, 1);
        let text_col = if is_selected { color::NAVY } else { color::BLACK };
        s.text(title_rect.x + 18, 5, &ch.label, text_col);

        // Sublabel (e.g. "Master", "LibreWolf", "MPV")
        let max_chars = ((COL_W - 8) / GLYPH_W) as usize;
        let sub = if ch.sublabel.len() > max_chars { format!("{}.", &ch.sublabel[..max_chars.saturating_sub(1)]) } else { ch.sublabel.clone() };
        let sub_w = sub.len() as i32 * GLYPH_W;
        let sub_x = cx + (COL_W - sub_w).max(0) / 2;
        s.text(sub_x, 23, &sub, color::SHADOW);

        // Track
        let track_x = cx + TRACK_OFFSET_X;
        let track = Rect::new(track_x, TRACK_Y, TRACK_W, TRACK_H);
        s.bevel(track, Bevel::Sunken);

        // Tick marks along the track (5 ticks: 100%, 75%, 50%, 25%, 0%)
        for t in 0..=4 {
            let ty = TRACK_Y + (t * (TRACK_H - 2)) / 4;
            s.hline(track_x - 7, ty, 5, color::SHADOW);
            s.hline(track_x + TRACK_W + 2, ty, 5, color::SHADOW);
        }

        // Max and Min labels
        s.text(track_x - 26, TRACK_Y, "Max", color::BLACK);
        s.text(track_x - 26, TRACK_Y + TRACK_H - GLYPH_H, "Min", color::BLACK);

        // Knob
        let k_y = knob_y(ch.volume);
        let knob = Rect::new(track_x - (KNOB_W - TRACK_W) / 2, k_y, KNOB_W, KNOB_H);
        s.fill(knob, color::FACE);
        s.bevel(knob, Bevel::Raised);
        // Ridge line on knob
        s.hline(knob.x + 3, knob.y + KNOB_H / 2, KNOB_W - 6, color::SHADOW);

        // Percentage display
        let pct_str = format!("{}%", ch.volume);
        let px = cx + (COL_W - (pct_str.len() as i32 * GLYPH_W)) / 2;
        s.text(px, TRACK_Y + TRACK_H + 5, &pct_str, if ch.muted { color::SHADOW } else { color::NAVY });

        // Mute Checkbox
        let check_box = Rect::new(cx + 8, 170, 13, 13);
        s.fill(check_box, color::WHITE);
        s.bevel(check_box, Bevel::Sunken);
        if ch.muted {
            s.sprite(check_box.x, check_box.y, icons::GLYPH_CHECK, 1);
        }
        let mute_text = match ch.kind {
            ChannelKind::Master => "Mute all",
            ChannelKind::Mpris { .. } => "Mute",
        };
        s.text(cx + 24, 171, mute_text, if ch.muted { color::RED } else { color::BLACK });

        // Bottom area:
        // Col 0 gets "OK" button; other columns get app tag
        if i == 0 {
            let ok_rect = Rect::new(cx + (COL_W - 54) / 2, 198, 54, 22);
            ferro_ui::default_button(s, ok_rect, "OK", true);
        } else {
            let badge_rect = Rect::new(cx + 8, 200, COL_W - 16, 18);
            s.bevel(badge_rect, Bevel::Sunken);
            let tag = match ch.label.as_str() {
                "Browser" => "Web",
                "Video" => "Player",
                _ => "Media",
            };
            let tw = tag.len() as i32 * GLYPH_W;
            s.text(cx + (COL_W - tw) / 2, 203, tag, color::BLACK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_conversion_endpoints() {
        assert_eq!(knob_y(100), TRACK_Y);
        assert_eq!(knob_y(0), TRACK_Y + TRACK_H - KNOB_H);

        assert_eq!(volume_from_y(TRACK_Y), 100);
        assert_eq!(volume_from_y(TRACK_Y + TRACK_H - KNOB_H), 0);
        assert_eq!(volume_from_y(TRACK_Y - 50), 100);
        assert_eq!(volume_from_y(TRACK_Y + TRACK_H + 50), 0);
    }

    #[test]
    fn multi_col_width_and_hit() {
        let mut state = State::default();
        assert_eq!(window_width(1), 180);
        assert_eq!(window_width(2), 210);
        assert_eq!(window_width(3), 310);

        // Click OK on single col
        let w = window_width(1);
        let m_cx = col_x(0, 1, w);
        assert_eq!(hit(m_cx + 27, 209, w, &state), Hit::Close);

        // Add second col (Browser)
        state.channels.push(ChannelState {
            kind: ChannelKind::Mpris { dest: "org.mpris.MediaPlayer2.firefox".to_string() },
            label: "Browser".to_string(),
            sublabel: "LibreWolf".to_string(),
            volume: 80,
            muted: false,
            saved_volume: 80,
        });

        let w2 = window_width(2);
        let col1_x = col_x(1, 2, w2);
        assert_eq!(hit(col1_x + 10, 175, w2, &state), Hit::ToggleMute(1));
    }
}
