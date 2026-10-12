//! The credential scan run before anything is published, from the pattern
//! list the hosted service enforces at ingest and the Go CLI before upload.

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde_json::Value;

/// Generated from `packages/format/secret-patterns.mjs` and checked for
/// drift by `scripts/golden.mjs`.
const PATTERNS: &str = include_str!("../../../packages/format/secret-patterns.json");

/// The scan stops after this many findings.
pub const MAX_HITS: usize = 10;

/// One finding: which pattern matched and a masked preview of the match,
/// enough to find the leak and never the whole value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub pattern: &'static str,
    pub preview: String,
}

struct Pattern {
    name: String,
    regex: Regex,
}

static COMPILED: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let list: Value = serde_json::from_str(PATTERNS).expect("the pattern list is JSON");
    let list = list.as_array().expect("the pattern list is an array");
    list.iter()
        .map(|entry| {
            let field = |key: &str| {
                entry[key]
                    .as_str()
                    .expect("a pattern has a name and a source")
            };
            // Unicode is off so that `\b` and the classes mean what they mean
            // to Go's regexp and to a JavaScript RegExp without the `u` flag.
            // With it on, a key that follows an accented letter has no word
            // boundary in front of it and is missed.
            let regex = RegexBuilder::new(field("source"))
                .unicode(false)
                .build()
                .expect("a pattern compiles with Unicode off");
            Pattern {
                name: field("name").to_owned(),
                regex,
            }
        })
        .collect()
});

/// Scans a document's text. Patterns run in list order, each over the whole
/// text, and the scan stops at [`MAX_HITS`] findings.
pub fn scan_for_secrets(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for pattern in COMPILED.iter() {
        for found in pattern.regex.find_iter(text) {
            hits.push(Hit {
                pattern: &pattern.name,
                preview: preview(found.as_str()),
            });
            if hits.len() >= MAX_HITS {
                return hits;
            }
        }
    }
    hits
}

/// The first eight bytes and the length. Every pattern matches ASCII only, so
/// this is the same text Go builds from bytes and JavaScript from UTF-16
/// units; the boundary check is there so that a later pattern cannot panic.
fn preview(found: &str) -> String {
    let mut end = found.len().min(8);
    while !found.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…({} chars)", &found[..end], found.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert!(!COMPILED.is_empty());
    }

    #[test]
    fn a_preview_never_splits_a_character() {
        assert_eq!(preview("sk-abcdéfgh"), "sk-abcd…(12 chars)");
        assert_eq!(preview("short"), "short…(5 chars)");
    }
}
