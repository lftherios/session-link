//! `slink list`: the captures in the home, newest first.

use slink_local::{Capture, ago, capture_dir, capture_id, list_captures, unix_now};

use crate::flags::{Parsed, Run};
use crate::text::{clip, plural};

/// What every surface that can meet an empty store says: the three ways in,
/// one on each line.
pub const EMPTY: &str = "no sessions yet

  record:  slink record -- <your agent command>
  import:  slink import
  ambient: slink setup";

pub fn run(parsed: &Parsed) -> Run {
    let limit = usize::try_from(parsed.int("limit")).unwrap_or(0).max(1);
    let captures = list_captures(&capture_dir());
    if parsed.bool("json") {
        let rows: Vec<String> = captures.iter().take(limit).map(json_row).collect();
        println!("[{}]", rows.join(","));
        return Ok(());
    }
    if captures.is_empty() {
        eprintln!("{EMPTY}");
        return Ok(());
    }
    let now = unix_now();
    for capture in captures.iter().take(limit) {
        println!("{}", row(capture, now));
    }
    if captures.len() > limit {
        // On stderr: piped stdout stays one row for each session.
        eprintln!(
            "      … and {} more — slink list --limit {}",
            captures.len() - limit,
            captures.len()
        );
    }
    eprintln!("\n  review:  slink view <id>\n  publish: slink share <id>");
    Ok(())
}

/// The one way a capture is shown in a terminal, so the id a person sees is
/// the id every command takes.
pub fn row(capture: &Capture, now: i64) -> String {
    format!(
        "  {:<7} {:<9} {:<38} {}  {}{}",
        capture_id(&capture.file),
        ago(&capture.created_at, now),
        clip(&capture.name, 38),
        plural(capture.spans, "span"),
        capture.models.join(", "),
        if capture.in_progress {
            "  (recording)"
        } else {
            ""
        },
    )
}

fn json_row(capture: &Capture) -> String {
    let text = |value: &str| slink_format::encode(&serde_json::Value::String(value.to_owned()));
    let models: Vec<String> = capture.models.iter().map(|model| text(model)).collect();
    format!(
        "{{\"id\":{},\"name\":{},\"created_at\":{},\"spans\":{},\"models\":[{}],\"file\":{},\"in_progress\":{}}}",
        text(&capture_id(&capture.file)),
        text(&capture.name),
        text(&capture.created_at),
        capture.spans,
        models.join(","),
        text(&capture.file.to_string_lossy()),
        capture.in_progress,
    )
}
