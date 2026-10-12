//! Saved previews. `slink view` never serves a capture or a transcript as it
//! is on disk: it saves the session it was asked for under the hash of its
//! bytes and serves that, so a recorder still writing, or an agent still
//! working, cannot change a page someone is reading.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::home::{create_private_dir, write_private};

/// What follows a preview's id in the name of its reading copy: the same
/// session without the recorded results of its tool calls. Those are most of
/// a large session's bytes and nothing the reader shows on arrival, so the
/// page carries this copy and fetches the whole document behind it.
pub const READING_SUFFIX: &str = ".reading.json";

/// Below this many bytes saved, a second request costs more than it avoids.
const READING_GAIN: usize = 512 << 10;

/// Saves the session in `text` as a preview in `dir` and returns its id, the
/// SHA-256 of the bytes stored. The text must be a valid session; it is
/// stored without the white space between its tokens and otherwise as it is.
pub fn save_preview(dir: &Path, text: &str) -> Result<String, String> {
    let document =
        slink_format::parse(text).map_err(|error| format!("cannot read session JSON: {error}"))?;
    let errors = slink_format::validate(&document);
    if !errors.is_empty() {
        return Err(format!(
            "cannot preview this session: {}",
            errors.join("; ")
        ));
    }
    let data = slink_format::compact(text).ok_or("cannot read session JSON")?;
    let id = hex(&Sha256::digest(data.as_bytes()));
    let failed = |error: std::io::Error| format!("cannot save the preview: {error}");
    create_private_dir(dir).map_err(failed)?;
    let file = dir.join(format!("{id}.json"));
    // Equal names have equal bytes, so two saves at once agree.
    if fs::read(&file).ok().as_deref() != Some(data.as_bytes()) {
        write_private(&file, data.as_bytes()).map_err(failed)?;
    }
    // Best effort: without it the page carries the whole document.
    if let Some(light) = reading_copy(&document) {
        let light = slink_format::encode(&light);
        if data.len().saturating_sub(light.len()) >= READING_GAIN {
            let _ = write_private(&dir.join(format!("{id}{READING_SUFFIX}")), light.as_bytes());
        }
    }
    Ok(id)
}

/// What a preview was made from: this build of the binary, the session, and
/// each source file's size and modification time. While none of that
/// changes, the preview made from it is used again and nothing is imported.
/// `None` when a file cannot be read, which is a reason to import.
///
/// `session` names the harness and the session within it. The files alone
/// do not: every run in an Aider history is in the same file.
pub fn source_stamp(build: &str, session: &str, files: &[PathBuf]) -> Option<String> {
    if files.is_empty() || build.is_empty() {
        return None;
    }
    let mut stamps = Vec::new();
    for file in files {
        let found = fs::metadata(file).ok()?;
        let modified = found
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        stamps.push(serde_json::json!({
            "path": file.to_string_lossy(),
            "size": found.len(),
            "modified_ns": u64::try_from(modified).ok()?,
        }));
    }
    Some(slink_format::encode(
        &serde_json::json!({"build": build, "session": session, "files": stamps}),
    ))
}

/// One record for each source, replaced whenever the source changes.
fn record_path(dir: &Path, session: &str, files: &[PathBuf]) -> PathBuf {
    let names: Vec<String> = files
        .iter()
        .map(|file| file.to_string_lossy().into_owned())
        .collect();
    let sum = Sha256::digest(format!("{session}\n{}", names.join("\n")).as_bytes());
    dir.join("sources")
        .join(format!("{}.json", hex(&sum[..16])))
}

/// The preview made from exactly this stamp, if it is still what its name
/// says: a damaged file is made again, not served.
pub fn reusable_preview(
    dir: &Path,
    session: &str,
    files: &[PathBuf],
    stamp: &str,
) -> Option<String> {
    let record =
        slink_format::parse(&fs::read_to_string(record_path(dir, session, files)).ok()?).ok()?;
    if record["stamp"].as_str()? != stamp {
        return None;
    }
    let id = record["snapshot"].as_str()?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let data = fs::read(dir.join(format!("{id}.json"))).ok()?;
    (hex(&Sha256::digest(&data)) == id).then(|| id.to_owned())
}

/// Notes which preview a stamp made. Best effort: without the note the next
/// view imports again.
pub fn remember_preview(dir: &Path, session: &str, files: &[PathBuf], stamp: &str, id: &str) {
    let record = slink_format::encode(&serde_json::json!({"stamp": stamp, "snapshot": id}));
    let _ = write_private(&record_path(dir, session, files), record.as_bytes());
}

/// The document without recorded tool results, or `None` when it has none.
/// The spans themselves stay, so both copies address the same material.
fn reading_copy(document: &Value) -> Option<Value> {
    let spans = document.get("spans")?.as_array()?;
    let mut omitted = false;
    let trimmed: Vec<Value> = spans
        .iter()
        .map(|span| {
            let Some(output) = span.get("output").and_then(Value::as_object) else {
                return span.clone();
            };
            if output.get("result").is_none_or(Value::is_null) {
                return span.clone();
            }
            omitted = true;
            let mut light: Map<String, Value> = output.clone();
            light.remove("result");
            light.insert("result_omitted".into(), Value::Bool(true));
            let mut span = span.clone();
            span["output"] = Value::Object(light);
            span
        })
        .collect();
    omitted.then(|| {
        let mut copy = document.clone();
        copy["spans"] = Value::Array(trimmed);
        copy
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_preview_is_used_again_until_its_source_changes() {
        let dir = std::env::temp_dir().join(format!("slink-local-reuse-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("session.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let files = vec![transcript.clone()];
        let previews = dir.join("previews");
        let text = r#"{"schema":"session/v0","created_at":"2026-07-19T10:00:00Z","spans":[{"id":"root","type":"agent"}]}"#;

        let stamp = source_stamp("build 1", "pi", &files).unwrap();
        assert_eq!(reusable_preview(&previews, "pi", &files, &stamp), None);
        let id = save_preview(&previews, text).unwrap();
        remember_preview(&previews, "pi", &files, &stamp, &id);
        assert_eq!(
            reusable_preview(&previews, "pi", &files, &stamp).as_deref(),
            Some(id.as_str())
        );
        // Another session in the same file, another build, or a file that
        // grew: each is another source.
        assert_eq!(reusable_preview(&previews, "omp", &files, &stamp), None);
        let rebuilt = source_stamp("build 2", "pi", &files).unwrap();
        assert_eq!(reusable_preview(&previews, "pi", &files, &rebuilt), None);
        fs::write(&transcript, "{}\n{}\n").unwrap();
        let grown = source_stamp("build 1", "pi", &files).unwrap();
        assert_ne!(grown, stamp);
        assert_eq!(reusable_preview(&previews, "pi", &files, &grown), None);
        // A preview that is no longer what its name says is not served.
        fs::write(previews.join(format!("{id}.json")), "{}").unwrap();
        assert_eq!(reusable_preview(&previews, "pi", &files, &stamp), None);
        assert_eq!(source_stamp("build 1", "pi", &[dir.join("absent")]), None);
        assert_eq!(source_stamp("build 1", "pi", &[]), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_reading_copy_leaves_out_results_and_says_so() {
        let document = json!({"spans": [
            {"id": "root", "type": "agent"},
            {"id": "s1", "type": "tool_call", "output": {"result": {"big": "x"}, "is_error": true}},
            {"id": "s2", "type": "tool_call", "output": {"result": null}},
            {"id": "s3", "type": "llm_call", "output": {"messages": []}},
        ]});
        let light = reading_copy(&document).unwrap();
        assert_eq!(light["spans"][0], document["spans"][0]);
        assert_eq!(
            light["spans"][1]["output"],
            json!({"is_error": true, "result_omitted": true})
        );
        assert_eq!(light["spans"][2], document["spans"][2]);
        assert_eq!(light["spans"][3], document["spans"][3]);
        assert_eq!(reading_copy(&json!({"spans": [{"id": "root"}]})), None);
    }
}
