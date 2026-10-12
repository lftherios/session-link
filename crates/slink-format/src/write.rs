//! Writing a document's text. An import is found again by its bytes, and the
//! hosted service dedupes by them, so a document is written one way: keys in
//! order, no white space, and the escapes below. They are the bytes Go's
//! `encoding/json` writes for the same value, which makes the Go CLI's output
//! the oracle for every importer.

use std::fmt::Write as _;

use serde_json::Value;

/// The document as text: objects with their keys sorted, nothing between
/// tokens, `<`, `>` and `&` escaped so the text can sit in a page, and the
/// two line separators JavaScript will not take in a string escaped too.
pub fn encode(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number),
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        // serde_json keeps an object's keys sorted, by their bytes, as Go
        // sorts a map's.
        Value::Object(fields) => {
            out.push('{');
            for (index, (key, field)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, field);
            }
            out.push('}');
        }
    }
}

/// A whole number is written as it is. Anything else is written as Go writes
/// a float64: the shortest digits that read back as the same number, and an
/// exponent only below 1e-6 or from 1e21.
fn write_number(out: &mut String, number: &serde_json::Number) {
    if let Some(whole) = number.as_u64() {
        write!(out, "{whole}").expect("writing to a string");
    } else if let Some(whole) = number.as_i64() {
        write!(out, "{whole}").expect("writing to a string");
    } else {
        write_float(out, number.as_f64().unwrap_or(0.0));
    }
}

fn write_float(out: &mut String, float: f64) {
    let size = float.abs();
    if size != 0.0 && !(1e-6..1e21).contains(&size) {
        // Rust writes 1e21 and 1.5e-7 where Go writes 1e+21 and 1.5e-07, and
        // Go then drops the zero.
        let text = format!("{float:e}");
        let (digits, exponent) = text.split_once('e').expect("an exponent");
        let sign = if exponent.starts_with('-') { "" } else { "+" };
        write!(out, "{digits}e{sign}{exponent}").expect("writing to a string");
    } else {
        // Rust writes 3 for 3.0 as well, and -0 as Go does.
        write!(out, "{float}").expect("writing to a string");
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                write!(out, "\\u{:04x}", character as u32).expect("writing to a string");
            }
            control if (control as u32) < 0x20 => {
                write!(out, "\\u{:04x}", control as u32).expect("writing to a string");
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The same JSON text with the white space between its tokens removed, and
/// nothing else changed: a number keeps its spelling and a string its
/// escapes. `None` when the text is not one JSON value.
pub fn compact(text: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
            out.push(character);
        } else if !matches!(character, ' ' | '\t' | '\n' | '\r') {
            out.push(character);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_are_sorted_and_nothing_sits_between_tokens() {
        let value = json!({"b": [1, null, true, {"z": "", "a": false}], "a": {}});
        assert_eq!(
            encode(&value),
            r#"{"a":{},"b":[1,null,true,{"a":false,"z":""}]}"#
        );
    }

    #[test]
    fn strings_are_escaped_as_go_escapes_them() {
        let text = "a\"b\\c\n\r\t\u{8}\u{c}\u{1}\u{1f}<p>&\u{2028}\u{2029}\u{7f}\u{e9}\u{1f600}";
        assert_eq!(
            encode(&json!(text)),
            "\"a\\\"b\\\\c\\n\\r\\t\\b\\f\\u0001\\u001f\\u003cp\\u003e\\u0026\\u2028\\u2029\u{7f}\u{e9}\u{1f600}\""
        );
    }

    #[test]
    fn numbers_are_written_as_go_writes_a_float() {
        // The text on the left is what a transcript holds; on the right, what
        // Go writes after reading it as a float64.
        for (read, written) in [
            ("0", "0"),
            ("7", "7"),
            ("-7", "-7"),
            ("1.0", "1"),
            ("1.50", "1.5"),
            ("1e3", "1000"),
            ("0.000001", "0.000001"),
            ("0.0000001", "1e-7"),
            ("1.5e-7", "1.5e-7"),
            ("123456789012345680000", "123456789012345680000"),
            ("1e21", "1e+21"),
            ("1.5e300", "1.5e+300"),
            ("-2.5e-10", "-2.5e-10"),
            ("3.141592653589793", "3.141592653589793"),
            ("0.1", "0.1"),
            ("100000000000000000000", "100000000000000000000"),
        ] {
            let value: Value = serde_json::from_str(read).unwrap();
            assert_eq!(encode(&value), written, "{read}");
        }
    }

    #[test]
    fn compacting_removes_white_space_and_nothing_else() {
        let text = " { \"a\" : [ 1.0 , \"x y\\\" z\" ,\n\ttrue ] , \"b\" : \"\\u00e9 \" } ";
        assert_eq!(
            compact(text).unwrap(),
            "{\"a\":[1.0,\"x y\\\" z\",true],\"b\":\"\\u00e9 \"}"
        );
        assert_eq!(compact("{} x"), None);
        assert_eq!(compact(""), None);
    }
}
