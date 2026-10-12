//! The little this crate needs of time: a stamp for a file name, when a
//! document says it was made, and how long ago that was.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970, from the system clock.
pub fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

/// `YYYYMMDD-HHMMSS` in UTC: the start of a capture's file name, which is why
/// captures list in the order they were made.
pub fn stamp(unix: i64) -> String {
    let (days, seconds) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// The date of a day counted from 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// How long before `now` a time was, as a listing shows it.
pub fn ago(time: &str, now: i64) -> String {
    let Some((then, _)) = slink_format::instant(time) else {
        return "?".into();
    };
    let seconds = now - then;
    match seconds {
        // A clock that is behind, or a stamp from the future.
        i64::MIN..0 => "just now".into(),
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_is_the_utc_date_and_time() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_784_455_445), "20260719-100405");
        assert_eq!(stamp(951_782_399), "20000228-235959");
        assert_eq!(stamp(951_782_400), "20000229-000000");
        assert_eq!(stamp(-1), "19691231-235959");
    }

    #[test]
    fn an_age_is_in_the_largest_whole_unit() {
        let then = "2026-07-19T10:04:05Z";
        let at = |later: i64| ago(then, 1_784_455_445 + later);
        assert_eq!(at(-5), "just now");
        assert_eq!(at(0), "0s ago");
        assert_eq!(at(59), "59s ago");
        assert_eq!(at(60), "1m ago");
        assert_eq!(at(3599), "59m ago");
        assert_eq!(at(3600), "1h ago");
        assert_eq!(at(86_399), "23h ago");
        assert_eq!(at(86_400), "1d ago");
        assert_eq!(at(86_400 * 400), "400d ago");
        assert_eq!(ago("yesterday", 0), "?");
    }
}
