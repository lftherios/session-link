//! Reading a transcript's JSON the way the importers were first written, in
//! JavaScript: a field that is missing, null or of another type is simply
//! not there, "set" means what `if (x)` means, and a value or a time is
//! made text as `String(x)` and `toISOString()` make it.

use serde_json::{Map, Value, json};

/// A field of an object; `None` when the value is not an object, the field
/// is missing, or it is null.
pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| !found.is_null())
}

/// The field as text, or `""`.
pub(crate) fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// The field as it is, with null for a missing one: what Go and JavaScript
/// both write when a span is given a value that was never there.
pub(crate) fn or_null(value: &Value, name: &str) -> Value {
    value.get(name).cloned().unwrap_or(Value::Null)
}

/// JavaScript's truthiness.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(set)) => *set,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|float| float != 0.0),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// Copies a field under another name when the source has it at all, null
/// included: JSON leaves out what was never assigned and keeps what was.
pub(crate) fn put(target: &mut Map<String, Value>, key: &str, source: &Value, source_key: &str) {
    if let Some(found) = source.get(source_key) {
        target.insert(key.to_owned(), found.clone());
    }
}

/// JavaScript's `String(x)` of a JSON value. A number is written as Go
/// writes the float64 it read it as, so a whole number too large for one
/// loses the same digits here.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(set) => set.to_string(),
        Value::Number(number) => slink_format::encode(&json!(number.as_f64())),
        Value::String(text) => text.clone(),
        // `Array.prototype.join`: a null is nothing, and commas go between.
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(item),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// `new Date(ms).toISOString()`, for any count of milliseconds an i64
/// holds. Go writes at least four digits of a year, after the sign of one
/// before year 0, and so does this.
pub(crate) fn iso_millis(ms: i64) -> String {
    let (days, of_day) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // The date of a day count, in 400-year eras that start on 1 March of
    // year 0, so a leap day is the last day of its year.
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
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    let sign = if year < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        year.unsigned_abs(),
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000,
    )
}

/// What the format keeps of content it has no part for.
pub(crate) fn data_part(data: &Value) -> Value {
    json!({"type": "data", "data": data})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_text_as_javascript_makes_it() {
        for (value, text) in [
            (json!(null), "null"),
            (json!("gpt-5.5"), "gpt-5.5"),
            (json!(""), ""),
            (json!(true), "true"),
            (json!(false), "false"),
            (json!(5), "5"),
            (json!(5.0), "5"),
            (json!(5.5), "5.5"),
            (json!(-1.5), "-1.5"),
            (json!(1e21), "1e+21"),
            (json!(1e-7), "1e-7"),
            // Whole numbers past what a float64 holds exactly, as the float.
            (json!(9_007_199_254_740_993_i64), "9007199254740992"),
            (json!(12345678901234567890_u64), "12345678901234567000"),
            (
                json!([1, null, "a", [2, [3, null]], {"x": 1}, true]),
                "1,,a,2,3,,[object Object],true",
            ),
            (json!([]), ""),
            (json!({"a": 1}), "[object Object]"),
        ] {
            assert_eq!(js_string(&value), text, "{value}");
        }
    }

    #[test]
    fn a_time_is_written_as_javascript_writes_a_date() {
        for (ms, written) in [
            (0, "1970-01-01T00:00:00.000Z"),
            (1_752_900_003_250, "2025-07-19T04:40:03.250Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
            // A leap day, and the day after 28 February in a year without one.
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            (4_107_542_400_000, "2100-03-01T00:00:00.000Z"),
            // Years that are not four digits long.
            (-62_167_219_200_000, "0000-01-01T00:00:00.000Z"),
            (-62_167_219_200_001, "-0001-12-31T23:59:59.999Z"),
            (253_402_300_800_000, "10000-01-01T00:00:00.000Z"),
        ] {
            assert_eq!(iso_millis(ms), written, "{ms}");
        }
    }
}
