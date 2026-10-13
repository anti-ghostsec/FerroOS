//! Retro Windows 95 Date & Time Properties view and calendar math.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const W: i32 = 340;
pub const H: i32 = 280;

pub const MONTH_NAMES: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 30,
    }
}

/// Returns day of week: 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
pub fn day_of_week(year: i32, month: u32, day: u32) -> u32 {
    // Tomohiko Sakamoto's algorithm
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    let m = (month - 1) as usize;
    let dow = (y + y / 4 - y / 100 + y / 400 + t[m] + day as i32) % 7;
    (dow + 7) as u32 % 7
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    PrevMonth,
    NextMonth,
    SelectDay(u32),
    Close,
    Nothing,
}

pub struct State {
    pub year: i32,
    pub month: u32, // 1..=12
    pub day: u32,   // 1..=31
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub timezone: String,
}

impl Default for State {
    fn default() -> Self {
        Self { year: 2026, month: 10, day: 13, hour: 16, min: 0, sec: 0, timezone: "UTC".into() }
    }
}

// Geometry constants
const PREV_BTN: Rect = Rect::new(20, 34, 18, 18);
const NEXT_BTN: Rect = Rect::new(154, 34, 18, 18);
const CAL_GRID_X: i32 = 18;
const CAL_GRID_Y: i32 = 72;
const CELL_W: i32 = 22;
const CELL_H: i32 = 18;

const OK_BTN: Rect = Rect::new(85, 245, 75, 23);
const CANCEL_BTN: Rect = Rect::new(175, 245, 75, 23);

pub fn hit(x: i32, y: i32, state: &State) -> Hit {
    if OK_BTN.contains(x, y) || CANCEL_BTN.contains(x, y) {
        return Hit::Close;
    }
    if PREV_BTN.contains(x, y) {
        return Hit::PrevMonth;
    }
    if NEXT_BTN.contains(x, y) {
        return Hit::NextMonth;
    }

    // Calendar day hit
    let first_dow = day_of_week(state.year, state.month, 1);
    let total_days = days_in_month(state.year, state.month);

    for d in 1..=total_days {
        let slot = (first_dow + d - 1) as i32;
        let col = slot % 7;
        let row = slot / 7;
        let r = Rect::new(CAL_GRID_X + col * CELL_W, CAL_GRID_Y + row * CELL_H, CELL_W, CELL_H);
        if r.contains(x, y) {
            return Hit::SelectDay(d);
        }
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // Tab Header
    let tab = Rect::new(10, 6, 90, 20);
    s.fill(tab, color::FACE);
    s.bevel(tab, Bevel::Raised);
    s.text(16, 9, "Date & Time", color::BLACK);

    // Date Group Box
    let date_grp = Rect::new(10, 24, 175, 210);
    s.bevel(date_grp, Bevel::Sunken);
    let date_title_bg = Rect::new(18, 22, 40, GLYPH_H + 2);
    s.fill(date_title_bg, color::FACE);
    s.text(20, 23, "Date", color::BLACK);

    // Month & Year header with < and > buttons
    s.button(PREV_BTN, false);
    s.text(PREV_BTN.x + 5, PREV_BTN.y + 3, "<", color::BLACK);

    let m_name = MONTH_NAMES.get((state.month.saturating_sub(1)) as usize).unwrap_or(&"Month");
    let header_str = format!("{} {}", m_name, state.year);
    let hx = 42 + (110 - header_str.len() as i32 * GLYPH_W) / 2;
    s.text(hx, 37, &header_str, color::BLACK);

    s.button(NEXT_BTN, false);
    s.text(NEXT_BTN.x + 5, NEXT_BTN.y + 3, ">", color::BLACK);

    // Day of week column headers
    let dows = ["S", "M", "T", "W", "T", "F", "S"];
    for (i, d) in dows.iter().enumerate() {
        let cx = CAL_GRID_X + i as i32 * CELL_W + 7;
        s.text(cx, 56, d, color::SHADOW);
    }
    s.hline(CAL_GRID_X, 68, 7 * CELL_W, color::SHADOW);

    // Day numbers
    let first_dow = day_of_week(state.year, state.month, 1);
    let total_days = days_in_month(state.year, state.month);

    for d in 1..=total_days {
        let slot = (first_dow + d - 1) as i32;
        let col = slot % 7;
        let row = slot / 7;
        let r = Rect::new(CAL_GRID_X + col * CELL_W, CAL_GRID_Y + row * CELL_H, CELL_W, CELL_H);

        let d_str = format!("{d}");
        let dx = r.x + (CELL_W - d_str.len() as i32 * GLYPH_W) / 2;
        let dy = r.y + (CELL_H - GLYPH_H) / 2;

        if d == state.day {
            s.fill(r, color::NAVY);
            s.text(dx, dy, &d_str, color::WHITE);
        } else {
            s.text(dx, dy, &d_str, color::BLACK);
        }
    }

    // Time Group Box
    let time_grp = Rect::new(195, 24, 135, 210);
    s.bevel(time_grp, Bevel::Sunken);
    let time_title_bg = Rect::new(203, 22, 40, GLYPH_H + 2);
    s.fill(time_title_bg, color::FACE);
    s.text(205, 23, "Time", color::BLACK);

    // Clock Icon centered in Time box
    s.sprite(248, 55, icons::CLOCK, 2);

    // Digital time display box
    let time_box = Rect::new(212, 115, 100, 26);
    s.fill(time_box, color::WHITE);
    s.bevel(time_box, Bevel::Sunken);

    let time_str = format!("{:02}:{:02}:{:02}", state.hour, state.min, state.sec);
    let tx = time_box.x + (time_box.w - time_str.len() as i32 * GLYPH_W) / 2;
    s.text_bold(tx, time_box.y + 7, &time_str, color::BLACK);

    // Timezone label
    let tz_lbl = format!("Zone: {}", state.timezone);
    s.text(208, 165, &tz_lbl, color::SHADOW);

    // Bottom buttons: OK, Cancel
    s.button(OK_BTN, false);
    s.text(OK_BTN.x + (OK_BTN.w - 2 * GLYPH_W) / 2, OK_BTN.y + 6, "OK", color::BLACK);

    s.button(CANCEL_BTN, false);
    s.text(CANCEL_BTN.x + (CANCEL_BTN.w - 6 * GLYPH_W) / 2, CANCEL_BTN.y + 6, "Cancel", color::BLACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_year_calculations() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2026));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));

        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 10), 31);
    }

    #[test]
    fn day_of_week_known_dates() {
        // Oct 1, 2026 was Thursday (4)
        assert_eq!(day_of_week(2026, 10, 1), 4);
        // Oct 13, 2026 is Tuesday (2)
        assert_eq!(day_of_week(2026, 10, 13), 2);
    }

    #[test]
    fn hit_testing() {
        let state = State::default();
        assert_eq!(hit(OK_BTN.x + 2, OK_BTN.y + 2, &state), Hit::Close);
        assert_eq!(hit(CANCEL_BTN.x + 2, CANCEL_BTN.y + 2, &state), Hit::Close);
        assert_eq!(hit(PREV_BTN.x + 2, PREV_BTN.y + 2, &state), Hit::PrevMonth);
        assert_eq!(hit(NEXT_BTN.x + 2, NEXT_BTN.y + 2, &state), Hit::NextMonth);
    }
}
