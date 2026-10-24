//! Retro Windows 95 Date & Time Properties view and calendar math.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const W: i32 = 340;
pub const H: i32 = 330;

/// Time zones offered, west to east, labelled as Windows labels them (by
/// their standard offset; summer time follows the zone's own rules).
pub const ZONES: [(&str, &str); 42] = [
    ("Pacific/Honolulu", "(UTC-10:00) Hawaii"),
    ("America/Anchorage", "(UTC-09:00) Alaska"),
    ("America/Los_Angeles", "(UTC-08:00) Pacific Time"),
    ("America/Phoenix", "(UTC-07:00) Arizona"),
    ("America/Denver", "(UTC-07:00) Mountain Time"),
    ("America/Chicago", "(UTC-06:00) Central Time"),
    ("America/Mexico_City", "(UTC-06:00) Mexico City"),
    ("America/New_York", "(UTC-05:00) Eastern Time"),
    ("America/Bogota", "(UTC-05:00) Bogota, Lima"),
    ("America/Halifax", "(UTC-04:00) Atlantic Time"),
    ("America/Sao_Paulo", "(UTC-03:00) Brasilia"),
    ("America/Argentina/Buenos_Aires", "(UTC-03:00) Buenos Aires"),
    ("Atlantic/Azores", "(UTC-01:00) Azores"),
    ("Etc/UTC", "(UTC) Coordinated Universal"),
    ("Europe/London", "(UTC+00:00) London, Dublin"),
    ("Europe/Lisbon", "(UTC+00:00) Lisbon"),
    ("Europe/Paris", "(UTC+01:00) Paris, Brussels"),
    ("Europe/Berlin", "(UTC+01:00) Berlin, Vienna"),
    ("Europe/Madrid", "(UTC+01:00) Madrid"),
    ("Europe/Rome", "(UTC+01:00) Rome"),
    ("Europe/Amsterdam", "(UTC+01:00) Amsterdam"),
    ("Europe/Stockholm", "(UTC+01:00) Stockholm, Oslo"),
    ("Europe/Warsaw", "(UTC+01:00) Warsaw, Prague"),
    ("Africa/Lagos", "(UTC+01:00) West Africa"),
    ("Europe/Athens", "(UTC+02:00) Athens"),
    ("Europe/Helsinki", "(UTC+02:00) Helsinki, Kyiv"),
    ("Africa/Cairo", "(UTC+02:00) Cairo"),
    ("Africa/Johannesburg", "(UTC+02:00) Johannesburg"),
    ("Europe/Istanbul", "(UTC+03:00) Istanbul"),
    ("Europe/Moscow", "(UTC+03:00) Moscow"),
    ("Africa/Nairobi", "(UTC+03:00) Nairobi"),
    ("Asia/Dubai", "(UTC+04:00) Dubai"),
    ("Asia/Karachi", "(UTC+05:00) Karachi"),
    ("Asia/Kolkata", "(UTC+05:30) India"),
    ("Asia/Dhaka", "(UTC+06:00) Dhaka"),
    ("Asia/Bangkok", "(UTC+07:00) Bangkok, Jakarta"),
    ("Asia/Shanghai", "(UTC+08:00) Beijing"),
    ("Asia/Singapore", "(UTC+08:00) Singapore"),
    ("Australia/Perth", "(UTC+08:00) Perth"),
    ("Asia/Tokyo", "(UTC+09:00) Tokyo, Seoul"),
    ("Australia/Sydney", "(UTC+10:00) Sydney"),
    ("Pacific/Auckland", "(UTC+12:00) Auckland"),
];

/// Where `zone` is in [`ZONES`] (UTC when it isn't listed).
pub fn zone_index(zone: &str) -> usize {
    let zone = if zone == "UTC" || zone == "Etc/Universal" { "Etc/UTC" } else { zone };
    ZONES.iter().position(|(id, _)| *id == zone).unwrap_or_else(|| ZONES.iter().position(|(id, _)| *id == "Etc/UTC").unwrap())
}

/// How the zone is shown: its label, or (one we don't list) its name.
pub fn zone_label(zone: &str) -> String {
    match ZONES.iter().find(|(id, _)| *id == zone) {
        Some((_, label)) => (*label).to_owned(),
        None => zone.replace('_', " "),
    }
}

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
    PrevZone,
    NextZone,
    Ok,
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
    /// The time zone chosen (applied on OK).
    pub timezone: String,
    /// Whether the clock is set automatically, and whether it has been.
    pub auto: bool,
    pub synced: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { year: 2026, month: 10, day: 13, hour: 16, min: 0, sec: 0, timezone: "Etc/UTC".into(), auto: true, synced: false }
    }
}

// Geometry constants
const PREV_BTN: Rect = Rect::new(20, 34, 18, 18);
const NEXT_BTN: Rect = Rect::new(154, 34, 18, 18);
const CAL_GRID_X: i32 = 18;
const CAL_GRID_Y: i32 = 72;
const CELL_W: i32 = 22;
const CELL_H: i32 = 18;

const ZONE_GRP: Rect = Rect::new(10, 242, 320, 44);
const ZONE_PREV: Rect = Rect::new(20, 258, 18, 18);
const ZONE_NEXT: Rect = Rect::new(302, 258, 18, 18);

const OK_BTN: Rect = Rect::new(85, 296, 75, 23);
const CANCEL_BTN: Rect = Rect::new(175, 296, 75, 23);

pub fn hit(x: i32, y: i32, state: &State) -> Hit {
    if OK_BTN.contains(x, y) {
        return Hit::Ok;
    }
    if CANCEL_BTN.contains(x, y) {
        return Hit::Close;
    }
    if ZONE_PREV.contains(x, y) {
        return Hit::PrevZone;
    }
    if ZONE_NEXT.contains(x, y) {
        return Hit::NextZone;
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

    // How the clock is kept right.
    let (line1, line2) = match (state.auto, state.synced) {
        (true, true) => ("Set automatically", "from the internet"),
        (true, false) => ("Set automatically", "(not yet synced)"),
        (false, _) => ("Set by hand", ""),
    };
    s.text(205, 158, line1, color::SHADOW);
    s.text(205, 172, line2, color::SHADOW);

    // Time zone group: < label >
    s.bevel(ZONE_GRP, Bevel::Sunken);
    let zone_title_bg = Rect::new(18, ZONE_GRP.y - 2, 80, GLYPH_H + 2);
    s.fill(zone_title_bg, color::FACE);
    s.text(20, ZONE_GRP.y - 1, "Time zone", color::BLACK);
    s.button(ZONE_PREV, false);
    s.text(ZONE_PREV.x + 5, ZONE_PREV.y + 5, "<", color::BLACK);
    s.button(ZONE_NEXT, false);
    s.text(ZONE_NEXT.x + 5, ZONE_NEXT.y + 5, ">", color::BLACK);
    let room = ((ZONE_NEXT.x - ZONE_PREV.right() - 8) / GLYPH_W).max(1) as usize;
    let label: String = zone_label(&state.timezone).chars().take(room).collect();
    let lx = ZONE_PREV.right() + (ZONE_NEXT.x - ZONE_PREV.right() - label.chars().count() as i32 * GLYPH_W) / 2;
    s.text(lx, ZONE_PREV.y + 5, &label, color::BLACK);

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
    fn zones() {
        assert_eq!(ZONES[zone_index("America/New_York")].0, "America/New_York");
        assert_eq!(ZONES[zone_index("UTC")].0, "Etc/UTC");
        assert_eq!(ZONES[zone_index("Mars/Olympus")].0, "Etc/UTC");
        assert_eq!(zone_label("Asia/Ho_Chi_Minh"), "Asia/Ho Chi Minh");
        // Every label fits between the arrows.
        assert!(ZONES.iter().all(|(_, l)| l.len() <= 32), "a label is too long");
    }

    #[test]
    fn hit_testing() {
        let state = State::default();
        assert_eq!(hit(OK_BTN.x + 2, OK_BTN.y + 2, &state), Hit::Ok);
        assert_eq!(hit(ZONE_PREV.x + 2, ZONE_PREV.y + 2, &state), Hit::PrevZone);
        assert_eq!(hit(ZONE_NEXT.x + 2, ZONE_NEXT.y + 2, &state), Hit::NextZone);
        assert_eq!(hit(CANCEL_BTN.x + 2, CANCEL_BTN.y + 2, &state), Hit::Close);
        assert_eq!(hit(PREV_BTN.x + 2, PREV_BTN.y + 2, &state), Hit::PrevMonth);
        assert_eq!(hit(NEXT_BTN.x + 2, NEXT_BTN.y + 2, &state), Hit::NextMonth);
    }
}
