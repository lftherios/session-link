//! Captures: the session documents in the home's `runs` directory, one file
//! each, named for when it was made.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::time::stamp;

/// One row of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub file: PathBuf,
    pub name: String,
    pub created_at: String,
    pub spans: usize,
    /// The models its calls name, each once, in the order they first appear.
    pub models: Vec<String>,
    pub in_progress: bool,
}

/// The captures in `dir`, newest first. A file that is not a session
/// document is left out, not reported: the directory is the user's.
pub fn list_captures(dir: &Path) -> Vec<Capture> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut captures: Vec<Capture> = entries
        .filter_map(|entry| read_capture(&entry.ok()?.path()))
        .collect();
    // The name starts with the time, so name order is time order.
    captures.sort_by(|a, b| b.file.cmp(&a.file));
    captures
}

fn read_capture(file: &Path) -> Option<Capture> {
    if !file.to_str()?.ends_with(".json") {
        return None;
    }
    let document = slink_format::parse(&fs::read_to_string(file).ok()?).ok()?;
    let document = document.as_object()?;
    if !matches!(document.get("schema")?.as_str()?, "session/v0" | "run/v0") {
        return None;
    }
    let text = |field: &str| {
        document
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let spans = document.get("spans").and_then(Value::as_array);
    let mut models: Vec<String> = Vec::new();
    for span in spans.into_iter().flatten() {
        if span["type"] != "llm_call" {
            continue;
        }
        if let Some(id) = span["model"]["id"].as_str()
            && !id.is_empty()
            && !models.iter().any(|known| known == id)
        {
            models.push(id.to_owned());
        }
    }
    Some(Capture {
        file: file.to_owned(),
        name: text("name").to_owned(),
        created_at: text("created_at").to_owned(),
        spans: spans.map_or(0, Vec::len),
        models,
        in_progress: document
            .get("metadata")
            .is_some_and(|metadata| metadata["in_progress"] == true),
    })
}

/// The id every command takes for a capture: the six hex digits that end its
/// file name. Empty for a file that is not named as a capture is.
pub fn capture_id(file: &Path) -> String {
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.strip_suffix(".json")
        .and_then(|stem| stem.rsplit_once('-'))
        .map(|(_, id)| id)
        .filter(|id| {
            id.len() == 6
                && id
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
        .unwrap_or_default()
        .to_owned()
}

/// Where a capture made at `unix` goes: `<stamp>-<six hex digits>.json`. The
/// random part keeps two made in the same second apart.
pub fn new_capture_path(dir: &Path, unix: i64, random: [u8; 3]) -> PathBuf {
    let [a, b, c] = random;
    dir.join(format!("{}-{a:02x}{b:02x}{c:02x}.json", stamp(unix)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_the_hex_that_ends_the_name() {
        assert_eq!(
            capture_id(Path::new("/x/runs/20260719-100405-a1b2c3.json")),
            "a1b2c3"
        );
        assert_eq!(capture_id(Path::new("20260719-100405-A1B2C3.json")), "");
        assert_eq!(capture_id(Path::new("20260719-100405-a1b2c.json")), "");
        assert_eq!(capture_id(Path::new("session.json")), "");
        assert_eq!(
            capture_id(Path::new("20260719-100405-a1b2c3.json.spool")),
            ""
        );
    }

    #[test]
    fn a_new_capture_is_named_for_its_second() {
        let path = new_capture_path(Path::new("runs"), 1_784_455_445, [0xa1, 0x0b, 0xc3]);
        assert_eq!(path, Path::new("runs/20260719-100405-a10bc3.json"));
        assert_eq!(capture_id(&path), "a10bc3");
    }
}
