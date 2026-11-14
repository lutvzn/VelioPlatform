//! Times as the Worker kept them: milliseconds since 1970, shown in UTC.

use std::time::{SystemTime, UNIX_EPOCH};

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;

pub fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}

/// The year, month and day of a day since 1970 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A day as 2026-11-10, as `toISOString().slice(0, 10)`.
pub fn day(ms: i64) -> String {
    let (year, month, day) = civil(ms.div_euclid(DAY_MS));
    format!("{year:04}-{month:02}-{day:02}")
}

/// As `toISOString()`: 2026-11-10T09:05:03.042Z.
pub fn iso(ms: i64) -> String {
    let in_day = ms.rem_euclid(DAY_MS);
    let (h, m, s, milli) = (in_day / HOUR_MS, in_day / 60_000 % 60, in_day / 1000 % 60, in_day % 1000);
    format!("{}T{h:02}:{m:02}:{s:02}.{milli:03}Z", day(ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_as_javascript_does() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        // new Date(1794560703042).toISOString()
        assert_eq!(iso(1_794_560_703_042), "2026-11-13T09:05:03.042Z");
        assert_eq!(day(951_782_400_000), "2000-02-29");
        assert_eq!(day(-1), "1969-12-31");
    }
}
