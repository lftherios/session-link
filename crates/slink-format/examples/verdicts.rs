//! The Rust side of `scripts/format-diff.mjs`. Each line on stdin is a JSON
//! string holding one document's text; each line on stdout is this crate's
//! verdict on it and what the credential scan finds in it.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};
use slink_format::{parse, scan_for_secrets, validate};

fn main() -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let text: String = serde_json::from_str(&line?)?;
        let verdict = match parse(&text) {
            Err(error) => json!({ "verdict": "unreadable", "errors": [error.to_string()] }),
            Ok(document) => {
                let errors = validate(&document);
                let verdict = if errors.is_empty() {
                    "valid"
                } else {
                    "invalid"
                };
                json!({ "verdict": verdict, "errors": errors })
            }
        };
        let hits: Vec<Value> = scan_for_secrets(&text)
            .into_iter()
            .map(|hit| json!({ "pattern": hit.pattern, "preview": hit.preview }))
            .collect();
        let mut verdict = verdict;
        verdict["secrets"] = Value::Array(hits);
        writeln!(out, "{verdict}")?;
    }
    out.flush()
}
