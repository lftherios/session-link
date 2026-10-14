//! Small pieces of terminal text every command shares.

/// At most `limit` characters, the last one an ellipsis when cut.
pub fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        text.chars()
            .take(limit.saturating_sub(1))
            .chain(['…'])
            .collect()
    }
}

/// A size as the publish gate shows it: megabytes from a million bytes,
/// else whole kibibytes, at least one.
pub fn fmt_bytes(count: usize) -> String {
    if count >= 1_000_000 {
        format!("{:.1} MB", count as f64 / 1e6)
    } else {
        format!("{} KB", (count / 1024).max(1))
    }
}

pub fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// Text in double quotes with what cannot be shown escaped, as the Go CLI
/// prints a name: a line break in a session's name must not break the line.
pub fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            hidden if hidden.is_control() && (hidden as u32) < 0x80 => {
                out.push_str(&format!("\\x{:02x}", hidden as u32));
            }
            hidden if unprintable(hidden) => {
                let code = hidden as u32;
                if code > 0xffff {
                    out.push_str(&format!("\\U{code:08x}"));
                } else {
                    out.push_str(&format!("\\u{code:04x}"));
                }
            }
            shown => out.push(shown),
        }
    }
    out.push('"');
    out
}

/// Controls, spaces other than the space, and the characters that only
/// direct layout: what a terminal shows as nothing or as something else.
fn unprintable(character: char) -> bool {
    character.is_control()
        || matches!(character as u32,
            0xa0 | 0xad | 0x1680 | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x206f | 0x3000 | 0xfeff
            | 0xe000..=0xf8ff | 0xfff9..=0xfffb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_by_characters() {
        assert_eq!(clip("abcdef", 6), "abcdef");
        assert_eq!(clip("abcdefg", 6), "abcde…");
        assert_eq!(clip("ééééé", 4), "ééé…");
        assert_eq!(plural(1, "span"), "1 span");
        assert_eq!(plural(0, "span"), "0 spans");
    }

    #[test]
    fn a_quoted_name_stays_on_its_line() {
        assert_eq!(
            quoted("fix the \"bug\"\n\tnow\\"),
            "\"fix the \\\"bug\\\"\\n\\tnow\\\\\""
        );
        assert_eq!(quoted("caf\u{e9} \u{1f600}"), "\"caf\u{e9} \u{1f600}\"");
        assert_eq!(
            quoted("a\u{1}b\u{a0}c\u{200d}d"),
            "\"a\\x01b\\u00a0c\\u200dd\""
        );
    }
}
