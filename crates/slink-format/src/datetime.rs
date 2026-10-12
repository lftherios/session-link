//! The `date-time` format, checked here and not left to the schema library.
//!
//! The validators in use disagree at the edges. ajv-formats takes a space or
//! a tab for the `T` and an offset without its colon; the Go library and boon
//! take a sign where a digit belongs, as in `T+1:00:00Z`. This check is RFC
//! 3339 section 5.6 with nothing added, so whatever passes here passes each
//! of the others.

/// Why a string is not an RFC 3339 date and time.
pub(crate) fn check(text: &str) -> Result<(), &'static str> {
    const SHAPE: &str = "want yyyy-mm-ddThh:mm:ss, an optional fraction, then Z or ±hh:mm";
    let bytes = text.as_bytes();
    // The shortest form is 2026-07-19T10:00:00Z.
    if bytes.len() < 20 {
        return Err(SHAPE);
    }
    let two = |at: usize| -> Option<u32> {
        match (bytes.get(at)?, bytes.get(at + 1)?) {
            (a @ b'0'..=b'9', b @ b'0'..=b'9') => {
                Some(u32::from(a - b'0') * 10 + u32::from(b - b'0'))
            }
            _ => None,
        }
    };
    let is = |at: usize, wanted: &[u8]| bytes.get(at).is_some_and(|byte| wanted.contains(byte));

    let (Some(century), Some(year), Some(month), Some(day)) = (two(0), two(2), two(5), two(8))
    else {
        return Err(SHAPE);
    };
    let (Some(hour), Some(minute), Some(second)) = (two(11), two(14), two(17)) else {
        return Err(SHAPE);
    };
    if !(is(4, b"-") && is(7, b"-") && is(10, b"Tt") && is(13, b":") && is(16, b":")) {
        return Err(SHAPE);
    }

    let mut at = 19;
    if is(at, b".") {
        let digits = bytes[at + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return Err(SHAPE);
        }
        at += 1 + digits;
    }
    // Minutes east of UTC.
    let offset = if is(at, b"Zz") && bytes.len() == at + 1 {
        0
    } else if is(at, b"+-") && is(at + 3, b":") && bytes.len() == at + 6 {
        let (Some(hours), Some(minutes)) = (two(at + 1), two(at + 4)) else {
            return Err(SHAPE);
        };
        if hours > 23 || minutes > 59 {
            return Err("the offset is out of range");
        }
        let east = (hours * 60 + minutes) as i32;
        if bytes[at] == b'-' { -east } else { east }
    } else {
        return Err(SHAPE);
    };

    let year = century * 100 + year;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err("the month is out of range"),
    };
    if day == 0 || day > days {
        return Err("the day is out of range");
    }
    if hour > 23 || minute > 59 || second > 60 {
        return Err("the time is out of range");
    }
    // A leap second is the last second of a UTC day, wherever it is written.
    if second == 60 {
        let utc = ((hour * 60 + minute) as i32 - offset).rem_euclid(24 * 60);
        if utc != 23 * 60 + 59 {
            return Err("a leap second falls only at 23:59:60 UTC");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn accepts_what_rfc_3339_writes() {
        for text in [
            "2026-07-19T10:00:00Z",
            "2026-07-19t10:00:00z",
            "2026-07-19T10:00:00.5Z",
            "2026-07-19T10:00:00.000000000000000000001Z",
            "2026-07-19T10:00:00+02:00",
            "2026-07-19T10:00:00-23:59",
            "2028-02-29T10:00:00Z",
            "2000-02-29T10:00:00Z",
            "0000-02-29T00:00:00Z",
            "9999-12-31T23:59:59Z",
            "2026-12-31T23:59:60Z",
            "2026-07-20T01:59:60+02:00",
            "2026-07-19T15:59:60-08:00",
            "2026-07-19T00:29:60+00:30",
            "2026-07-19T23:59:60.5Z",
        ] {
            assert_eq!(check(text), Ok(()), "{text}");
        }
    }

    #[test]
    fn refuses_what_only_some_validators_take() {
        for text in [
            "2026-07-19 10:00:00Z",
            "2026-07-19\t10:00:00Z",
            "2026-07-19T10:00:00+0200",
            "2026-07-19T10:00:00+02",
            "2026-07-19T+1:00:00Z",
            "2026-07-19T10:00:00++2:00",
            "2026-07-19T10:00:00+02:+1",
            "+026-07-19T10:00:00Z",
            "2026-+7-19T10:00:00Z",
        ] {
            assert!(check(text).is_err(), "{text}");
        }
    }

    #[test]
    fn refuses_dates_and_times_that_do_not_exist() {
        for text in [
            "2026-02-29T10:00:00Z",
            "2100-02-29T10:00:00Z",
            "2026-04-31T10:00:00Z",
            "2026-13-01T10:00:00Z",
            "2026-07-00T10:00:00Z",
            "2026-07-19T24:00:00Z",
            "2026-07-19T10:60:00Z",
            "2026-07-19T10:00:61Z",
            "2026-07-19T10:59:60Z",
            "2026-07-19T23:59:60+02:00",
            "2026-07-19T10:00:00+24:00",
            "2026-07-19T10:00:00+02:60",
        ] {
            assert!(check(text).is_err(), "{text}");
        }
    }

    #[test]
    fn refuses_other_shapes_without_panicking() {
        for text in [
            "",
            "2026-07-19",
            "2026-07-19T10:00:00",
            "2026-07-19T10:00:00.Z",
            "2026-07-19T10:00:00Z ",
            "2026-07-19T10:00:00ZZ",
            "2026-07-19T10:00:00.5",
            "２０２６-07-19T10:00:00Z",
            "2026-07-19T10:00:00Zé",
            "é026-07-19T10:00:00Z",
            "2026-07-19T10:00:0éZ",
            "2026-07-19T10:00:00.5é",
            "2026-07-19T10:00:00−02:00",
        ] {
            assert!(check(text).is_err(), "{text}");
        }
    }
}
