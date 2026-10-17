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
    let (year, month, day) = slink_format::civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// How long before `now` a time was, as a listing shows it.
pub fn ago(time: &str, now: i64) -> String {
    match slink_format::instant(time) {
        Some((then, _)) => ago_unix(then, now),
        None => "?".into(),
    }
}

/// How long before `now` the moment `then` was, both in seconds since 1970.
pub fn ago_unix(then: i64, now: i64) -> String {
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
