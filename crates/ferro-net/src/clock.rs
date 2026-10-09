//! Keeping the clock right without trusting anyone blindly.
//!
//! TLS certificate checks need a roughly correct clock, and a dead RTC
//! battery resets it to 2000 or 1970. Two layers:
//!
//! 1. A floor: never earlier than the day this binary was built.
//! 2. The `Date` header of an HTTPS response from a server whose
//!    certificate we verified. Unlike plain NTP it can't be spoofed by
//!    anyone on the network path, and it only ever contacts a DNS provider
//!    FerroOS already uses.

/// Parses an HTTP date (RFC 9110 IMF-fixdate): `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn parse_http_date(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace().skip(1); // weekday
    let day: i64 = it.next()?.parse().ok()?;
    let month = match it.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = it.next()?.parse().ok()?;
    let mut hms = it.next()?.split(':').map(|v| v.parse::<i64>().ok());
    let (h, m, sec) = (hms.next()??, hms.next()??, hms.next()??);
    if it.next()? != "GMT" || !(1..=31).contains(&day) || h > 23 || m > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + sec;
    u64::try_from(secs).ok()
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The build day (set by xtask), or a fixed recent date.
pub fn build_floor() -> u64 {
    option_env!("FERRO_BUILD_TIME").and_then(|s| s.parse().ok()).unwrap_or(1_790_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_dates() {
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(parse_http_date("Thu, 08 Oct 2026 12:03:20 GMT"), Some(1_791_461_000));
        assert_eq!(parse_http_date("Thu, 08 Oct 2026 12:03:20 PST"), None);
        assert_eq!(parse_http_date("garbage"), None);
    }
}
