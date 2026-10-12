//! Reading JSON text the way the rest of the product reads it.

use std::borrow::Cow;

use serde_json::Value;

/// Parses JSON text that may hold unpaired surrogate escapes.
///
/// A string cut in the middle of a character outside the basic plane, which
/// JavaScript does with any `slice`, is written back out as a lone surrogate
/// escape such as `\ud83d`. `JSON.parse` and Go's decoder read that;
/// serde_json refuses the whole text. Each unpaired escape is read here as
/// U+FFFD, as Go reads it, so a transcript or a document from a JavaScript
/// writer still opens.
///
/// The text is otherwise held to the JSON grammar, and to serde_json's limit
/// on nesting: arrays and objects 128 deep are refused. Go reads 10,000
/// levels. The limit stays because everything that walks a parsed value
/// recurses, and a stack that overflows cannot be caught.
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(&pair_surrogates(text))
}

/// Rewrites each unpaired surrogate escape, `\uD800` to `\uDFFF`, as the
/// escape of U+FFFD. The rewrite keeps every byte's position, so a syntax
/// error still names the right column, and it leaves text without such an
/// escape untouched.
fn pair_surrogates(text: &str) -> Cow<'_, str> {
    // Every surrogate escape starts `\ud` or `\uD`.
    if !text.contains("\\ud") && !text.contains("\\uD") {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let escape = |at: usize| -> Option<u16> {
        let digits = bytes.get(at..at + 6)?.strip_prefix(b"\\u")?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u16::from_str_radix(str::from_utf8(digits).ok()?, 16).ok()
    };

    let mut unpaired = Vec::new();
    let mut in_string = false;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => in_string = !in_string,
            b'\\' if in_string => match escape(at) {
                Some(0xD800..=0xDBFF) if matches!(escape(at + 6), Some(0xDC00..=0xDFFF)) => {
                    at += 12;
                    continue;
                }
                Some(0xD800..=0xDFFF) => {
                    unpaired.push(at);
                    at += 6;
                    continue;
                }
                // Any other escape: step over the character it escapes, so
                // that `\"` does not end the string and `\\` does not start
                // an escape.
                _ => at += 1,
            },
            _ => {}
        }
        at += 1;
    }
    if unpaired.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_owned();
    for at in unpaired {
        out.replace_range(at..at + 6, "\\ufffd");
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_with_nothing_to_rewrite_is_not_copied() {
        for text in [
            r#"{"a":"plain"}"#,
            r#"{"a":"\u00e9 \\ud83d"}"#,
            r#"{"a":"\ud83d\ude00"}"#,
        ] {
            assert!(matches!(pair_surrogates(text), Cow::Borrowed(_)), "{text}");
        }
    }

    #[test]
    fn a_pair_is_one_character() {
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), json!("\u{1f600}"));
        assert_eq!(parse(r#""\uD83D\uDE00""#).unwrap(), json!("\u{1f600}"));
    }

    #[test]
    fn each_unpaired_escape_becomes_the_replacement_character() {
        for (text, read) in [
            (r#""\ud83d""#, "\u{fffd}"),
            (r#""\ude00""#, "\u{fffd}"),
            (r#""\uDE00\uD83D""#, "\u{fffd}\u{fffd}"),
            (r#""\ud83dx""#, "\u{fffd}x"),
            (r#""a\ud83d\ud83d\ude00""#, "a\u{fffd}\u{1f600}"),
            (r#""\ud83d\u0041""#, "\u{fffd}A"),
            (r#""\ud83d\n""#, "\u{fffd}\n"),
        ] {
            assert_eq!(parse(text).unwrap(), json!(read), "{text}");
        }
    }

    #[test]
    fn an_escaped_backslash_is_not_an_escape() {
        // The six characters as written, not a surrogate.
        assert_eq!(parse(r#""\\ud83d""#).unwrap(), json!("\\ud83d"));
        assert_eq!(parse(r#""\\\ud83d""#).unwrap(), json!("\\\u{fffd}"));
        assert_eq!(
            parse(r#"["\"", "\ud83d"]"#).unwrap(),
            json!(["\"", "\u{fffd}"])
        );
    }

    #[test]
    fn keys_are_read_the_same_way() {
        assert_eq!(parse(r#"{"\ud83d":1}"#).unwrap(), json!({ "\u{fffd}": 1 }));
    }

    #[test]
    fn text_that_is_not_json_stays_an_error_at_the_same_place() {
        let plain = serde_json::from_str::<Value>(r#"{"a":"x" "b"}"#).unwrap_err();
        let error = parse(r#"{"a":"\ud83d" "b"}"#).unwrap_err();
        assert_eq!(error.line(), plain.line());
        assert_eq!(error.column(), plain.column() + 5);
        for text in [
            "",
            "{",
            r#""\ud83d"#,
            r#"\ud83d"#,
            r#""\ud8""#,
            r#""\udxyz""#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn nesting_stops_at_the_parser_limit() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(parse(&nested(127)).is_ok());
        assert!(parse(&nested(128)).is_err());
    }
}
