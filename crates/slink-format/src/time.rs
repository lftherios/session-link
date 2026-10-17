//! When a document says something happened. A session's times are RFC 3339
//! text, and this turns one into a moment that can be compared with another.

/// The date of a day counted from 1970-01-01: its year, month and day. Days
/// are counted in 400-year eras that start on 1 March of year 0, so that a
/// leap day is the last day of its year.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
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
    (era * 400 + year_of_era + i64::from(month <= 2), month, day)
}

/// The day count of a date, the other way round.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The moment an RFC 3339 time names: seconds since 1970 and the nanoseconds
/// after them, or `None` when the text is not one. Digits of the fraction
/// past the ninth are dropped.
pub fn instant(text: &str) -> Option<(i64, u32)> {
    let bytes = text.as_bytes();
    let digits = |from: usize, count: usize| -> Option<i64> {
        let part = bytes.get(from..from + count)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0, |sum, digit| sum * 10 + i64::from(digit - b'0'))
        })
    };
    let is = |at: usize, byte: u8| bytes.get(at) == Some(&byte);
    if !(is(4, b'-') && is(7, b'-') && is(10, b'T') && is(13, b':') && is(16, b':')) {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    let month_length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > month_length || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut at = 19;
    let mut nanos = 0;
    if is(at, b'.') {
        at += 1;
        let start = at;
        while let Some(digit) = bytes.get(at).filter(|byte| byte.is_ascii_digit()) {
            if at - start < 9 {
                nanos += u32::from(digit - b'0') * 10_u32.pow(8 - (at - start) as u32);
            }
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' if at + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if at + 6 == bytes.len() && is(at + 3, b':') => {
            let (hours, minutes) = (digits(at + 1, 2)?, digits(at + 4, 2)?);
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = hours * 3600 + minutes * 60;
            if *sign == b'+' { seconds } else { -seconds }
        }
        _ => return None,
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some((seconds, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_count_is_its_date_and_back() {
        for (days, date) in [
            (0, (1970, 1, 1)),
            (-1, (1969, 12, 31)),
            (11_016, (2000, 2, 29)),
            (11_017, (2000, 3, 1)),
            (47_541, (2100, 3, 1)),
            (-719_528, (0, 1, 1)),
            (-719_529, (-1, 12, 31)),
        ] {
            assert_eq!(civil_from_days(days), date, "{days}");
            assert_eq!(days_from_civil(date.0, date.1, date.2), days, "{date:?}");
        }
    }

    #[test]
    fn a_time_is_read_with_its_offset() {
        assert_eq!(instant("1970-01-01T00:00:00Z"), Some((0, 0)));
        assert_eq!(instant("2026-07-19T10:04:05Z"), Some((1_784_455_445, 0)));
        assert_eq!(
            instant("2026-07-19T10:04:05.123456Z"),
            Some((1_784_455_445, 123_456_000))
        );
        assert_eq!(
            instant("2026-07-19T10:04:05.1234567891Z"),
            Some((1_784_455_445, 123_456_789))
        );
        assert_eq!(
            instant("2026-07-19T12:04:05+02:00"),
            Some((1_784_455_445, 0))
        );
        assert_eq!(
            instant("2026-07-19T05:34:05-04:30"),
            Some((1_784_455_445, 0))
        );
        assert_eq!(instant("2000-02-29T00:00:00Z"), Some((951_782_400, 0)));
        for text in [
            "",
            "2026-07-19",
            "2026-07-19 10:04:05Z",
            "2026-07-19T10:04:05",
            "2026-07-19T10:04:05z",
            "2026-07-19T10:04:05.Z",
            "2026-02-30T10:04:05Z",
            "2026-13-01T10:04:05Z",
            "2026-07-19T24:00:00Z",
            "2026-07-19T10:04:60Z",
            "2026-07-19T10:04:05+0200",
            "2026-07-19T10:04:05Zx",
            "1900-02-29T00:00:00Z",
        ] {
            assert_eq!(instant(text), None, "{text}");
        }
    }
}
