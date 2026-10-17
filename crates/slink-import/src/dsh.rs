//! dsh (DeepSeek Harness). It keeps a session as an append-only log: a
//! header line, then one event on each line with its sequence number and
//! time. The events dsh builds the model's history from are the
//! conversation. System, developer and user messages gather as the input of
//! the next step, each assistant message is an `llm_call`, and a `tool/call`
//! event opens the `tool_call` span its `tool/result` closes. Events that
//! only steer the harness (policies, inbox splices, request headers) are not
//! replayed.
//!
//! dsh keeps a directory for each session, grouped by project, under
//! `sessions/` in its home. A session's directory holds its log, compressed
//! unless dsh was configured not to, once for each format generation dsh has
//! written it in.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::discover::{
    Candidate, Elsewhere, Found, expand_home, first_json_line, modified, newest_of, read_lines,
    sorted_entries, user_home,
};
use crate::images::image_part;
use crate::value::{data_part, field, iso_millis as iso, text};
use crate::{Agent, Input, LABEL};

/// The session format versions read here. v4 is what dsh 0.2 writes. v3, the
/// format of the 0.1 releases, records a tool result inside a wrapper block
/// and is otherwise read the same way. dsh refuses a log whose version it
/// does not know rather than guess at it, and so does this.
const FORMATS: [i64; 2] = [3, 4];

pub(crate) fn import(input: &Input) -> Result<Option<Value>, String> {
    let mut header: Option<Value> = None;
    let mut events: Vec<Value> = Vec::new();
    for (index, line) in input.lines.iter().enumerate() {
        // dsh discards a line it was cut off while writing. A line that is
        // only `null` stays, as an event that says nothing: Go reads it so,
        // and it counts among the events a fork inherited.
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        if !entry.is_object() && !entry.is_null() {
            continue;
        }
        if index == 0 && text(&entry, "type") == "session" {
            header = Some(entry);
            continue;
        }
        events.push(entry);
    }
    let Some(header) = header else {
        return Err("not a dsh session log: it does not start with a session header".to_owned());
    };
    let version = header["version"]
        .as_f64()
        .map_or(-1, |version| version as i64);
    if !FORMATS.contains(&version) {
        return Err(format!(
            "dsh session format v{version} is not one slink reads (v3 and v4 are)"
        ));
    }
    let (events, inherited) = if input.sub_agent {
        own_events(&events)
    } else {
        (events.as_slice(), 0)
    };

    let created = millis(&header["createdAt"]).unwrap_or(0);
    let mut spans = vec![json!({
        "id": "root",
        "parent_id": null,
        "type": "agent",
        "started_at": iso(created),
    })];
    let mut sequence = 0_u64;
    let mut next = || {
        sequence += 1;
        format!("s{sequence}")
    };

    let (mut title, mut prompt) = (String::new(), String::new());
    let mut pending: Vec<Value> = Vec::new();
    let (mut last_time, mut call_start) = (created, created);
    // The llm_call the open step belongs to.
    let mut last_call = String::new();
    // Tool call id -> where its span is in `spans`, until the result arrives.
    let mut open: HashMap<String, usize> = HashMap::new();
    // Delegation calls still running, oldest first.
    let mut delegations: Vec<String> = Vec::new();
    // Sub-agent session id -> the call that started it.
    let mut agent_calls: HashMap<String, String> = HashMap::new();
    // What the request under way was prepared for.
    let mut request = &Value::Null;
    let (mut compactions, mut retries) = (0_u64, 0_u64);

    for event in events {
        let data = &event["data"];
        let at = match millis(&event["time"]) {
            Some(at) => {
                last_time = at;
                at
            }
            None => last_time,
        };
        match text(event, "type") {
            "session/title" => {
                if let Some(given) = given_title(data) {
                    title = given.to_owned();
                }
            }
            "request/context" => request = data,
            "step/start" | "llm/retry-started" => call_start = at,
            "llm/retry" => retries += 1,
            "compaction/end" => compactions += 1,

            "system/message" | "developer/message" => {
                let message = &data["message"];
                pending.push(json!({
                    "role": message["role"].as_str().unwrap_or("system"),
                    "content": content(&message["content"], &input.blobs),
                }));
            }
            "user/message" => {
                // dsh says who wrote each message. Only the person's own are
                // theirs; the rest is context the harness supplied in their
                // turn.
                let typed = text(&data["source"], "kind") == "user";
                if typed && prompt.is_empty() {
                    prompt = first_text(&data["content"]).to_owned();
                }
                pending.push(json!({
                    "role": if typed { "user" } else { "system" },
                    "content": content(&data["content"], &input.blobs),
                }));
            }

            "assistant/attempt" => {
                // A model request that failed before it produced a message.
                // dsh keeps the attempt, and its retry is the call that
                // follows. The messages waiting were sent with the first of
                // them.
                let mut failure = "the model request did not complete".to_owned();
                for record in data["stream"].as_array().into_iter().flatten() {
                    let reason = &record["chunk"]["reason"];
                    let failed = &reason["failure"];
                    if !failed.is_object() {
                        continue;
                    }
                    failure = failed["message"]
                        .as_str()
                        .or(reason["kind"].as_str())
                        .unwrap_or("error")
                        .to_owned();
                    let code = text(failed, "code");
                    if !code.is_empty() {
                        failure = format!("{failure} ({code})");
                    }
                }
                spans.push(json!({
                    "id": next(),
                    "parent_id": "root",
                    "type": "llm_call",
                    "name": "turn",
                    "started_at": iso(call_start),
                    "ended_at": iso(at),
                    "status": "error",
                    "input": {"messages": std::mem::take(&mut pending)},
                    "output": {"messages": []},
                    "error": {"message": failure},
                    "model": model(&Value::Null, request),
                }));
                call_start = at;
            }
            "assistant/message" => {
                let message = &data["message"];
                let id = next();
                let mut span = json!({
                    "id": id,
                    "parent_id": "root",
                    "type": "llm_call",
                    "name": "turn",
                    "started_at": iso(call_start),
                    "ended_at": iso(at),
                    "status": "ok",
                    "input": {"messages": std::mem::take(&mut pending)},
                    "output": {"messages": [{
                        "role": "assistant",
                        "content": content(&message["content"], &input.blobs),
                    }]},
                    "model": model(&message["source"], request),
                });
                if let Some(counts) = usage(&data["usage"]) {
                    span["usage"] = counts;
                }
                if data["interrupted"] == true {
                    span["metadata"] = json!({"interrupted": true});
                }
                spans.push(span);
                last_call = id;
                call_start = at;
            }

            "tool/call" => {
                let id = text(data, "callId");
                let name = data["name"].as_str().unwrap_or("tool");
                spans.push(json!({
                    "id": next(),
                    "parent_id": if last_call.is_empty() { "root" } else { last_call.as_str() },
                    "type": "tool_call",
                    "name": name,
                    "started_at": iso(at),
                    "ended_at": iso(at),
                    "input": {
                        "tool_call_id": id,
                        "name": name,
                        "arguments": arguments(&data["arguments"]),
                    },
                }));
                if !id.is_empty() {
                    open.insert(id.to_owned(), spans.len() - 1);
                    if name == "subagent" || name == "subagent_fork" {
                        delegations.push(id.to_owned());
                    }
                }
            }
            "subagent/catalog" => {
                // The parent's note that a sub-agent now exists. It does not
                // name the call that asked for it; that is the delegation
                // still waiting, and the oldest of them when several are.
                let child = text(data, "childId");
                if !child.is_empty() && !delegations.is_empty() {
                    agent_calls.insert(child.to_owned(), delegations.remove(0));
                }
            }
            "tool/result" => {
                let message = &data["message"];
                let mut blocks = &message["content"];
                let mut id = message["toolCallId"]
                    .as_str()
                    .or(message["source"]["callId"].as_str())
                    .unwrap_or_default();
                let mut failed = message["isError"] == true;
                // v3 wraps the result in one block that carries the call's id.
                if let Some([wrapper]) = blocks.as_array().map(Vec::as_slice)
                    && text(wrapper, "type") == "tool-result"
                {
                    blocks = &wrapper["content"];
                    id = wrapper["toolCallId"].as_str().unwrap_or(id);
                    failed = wrapper["isError"] == true;
                }
                let slot = open.remove(id).unwrap_or_else(|| {
                    spans.push(json!({
                        "id": next(),
                        "parent_id": "root",
                        "type": "tool_call",
                        "name": "tool",
                        "started_at": iso(at),
                        "input": {"tool_call_id": id, "name": "tool"},
                    }));
                    spans.len() - 1
                });
                if let Some(waiting) = delegations.iter().position(|waiting| waiting == id) {
                    delegations.remove(waiting);
                }
                let failed = failed || field(data, "error").is_some();
                let mut output = Map::new();
                output.insert("result".into(), result(content(blocks, &input.blobs)));
                if failed {
                    output.insert("is_error".into(), Value::Bool(true));
                }
                let span = &mut spans[slot];
                span["status"] = json!(if failed { "error" } else { "ok" });
                span["output"] = Value::Object(output);
                span["ended_at"] = json!(iso(at));
            }
            _ => {}
        }
    }

    let mut name = title;
    if name.is_empty() {
        name = prompt;
        if name.chars().count() > 80 {
            name = name.chars().take(80).chain(['…']).collect();
        }
    }
    if name.is_empty() {
        name = header["id"].as_str().unwrap_or(&input.fallback).to_owned();
    }
    if name.is_empty() {
        name = "dsh session".to_owned();
    }
    let root = &mut spans[0];
    root["name"] = json!(name);
    root["ended_at"] = json!(iso(last_time));
    root["status"] = json!("ok");
    if !pending.is_empty() {
        spans.push(json!({
            "id": next(),
            "parent_id": "root",
            "type": "custom",
            "name": "trailing message(s) — no assistant reply",
            "started_at": iso(last_time),
            "ended_at": iso(last_time),
            "input": {"messages": pending},
        }));
    }

    let mut metadata = Map::new();
    metadata.insert("session_format".into(), json!(version));
    for (name, theirs) in [
        ("session_id", "id"),
        ("cwd", "cwd"),
        ("parent_session_id", "parentSession"),
        ("agent_preset", "agentPreset"),
    ] {
        if let Some(recorded) = header[theirs].as_str() {
            metadata.insert(name.into(), json!(recorded));
        }
    }
    for (name, count) in [
        ("inherited_events_skipped", inherited as u64),
        ("compactions", compactions),
        ("model_retries", retries),
    ] {
        if count > 0 {
            metadata.insert(name.into(), json!(count));
        }
    }

    let mut run = json!({
        "schema": "session/v0",
        "name": name,
        "created_at": iso(created),
        "source": {"kind": "import", "harness": "dsh", "label": LABEL, "fidelity": "reconstructed"},
        "metadata": metadata,
        "spans": spans,
    });
    adopt_agents(&mut run, &input.agents, &agent_calls, |agent: &Agent| {
        let mut input = agent.input.clone();
        input.sub_agent = true;
        import(&input).ok().flatten()
    });
    Ok(Some(run))
}

/// Drops the history a forked sub-agent starts with, and says how many
/// events that was. dsh seeds the fork with a copy of the parent's completed
/// turns and marks where its own work begins; the copy is already in the
/// parent this is nested under.
fn own_events(events: &[Value]) -> (&[Value], usize) {
    let own = events
        .iter()
        .rposition(|event| {
            text(event, "type") == "session/end-seed" && event["data"]["inherited"] == true
        })
        .map_or(0, |cut| cut + 1);
    (&events[own..], own)
}

/// A time dsh wrote, in milliseconds since 1970; `None` when it is not a
/// number.
fn millis(recorded: &Value) -> Option<i64> {
    recorded.as_f64().map(|ms| ms as i64)
}

/// The title a `session/title` event gives. dsh starts every session with a
/// title cut from the prompt; that one is not a title.
fn given_title(data: &Value) -> Option<&str> {
    let title = text(data, "title").trim();
    (!title.is_empty() && text(&data["source"], "kind") != "fallback").then_some(title)
}

/// The first text among a message's blocks that is not blank, or `""`.
fn first_text(blocks: &Value) -> &str {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| text(block, "text").trim())
        .find(|said| !said.is_empty())
        .unwrap_or_default()
}

/// A tool call's arguments. dsh keeps the JSON text the model produced; text
/// that is not JSON stays as it was written.
fn arguments(raw: &Value) -> Value {
    raw.as_str()
        .and_then(|written| slink_format::parse(written).ok())
        .unwrap_or_else(|| raw.clone())
}

/// Maps the blocks of one message. dsh keeps an image's bytes in its
/// attachment store and a reference in the log; an image whose bytes are not
/// at hand, like any block this does not know, is kept as recorded.
fn content(blocks: &Value, attachments: &Path) -> Vec<Value> {
    let mut parts = Vec::new();
    for block in blocks.as_array().into_iter().flatten() {
        parts.push(match text(block, "type") {
            "text" => json!({"type": "text", "text": text(block, "text")}),
            "reasoning" => json!({"type": "thinking", "text": text(block, "text")}),
            "tool-call" => {
                let mut part = Map::new();
                part.insert("type".into(), json!("tool_call"));
                part.insert("arguments".into(), arguments(&block["arguments"]));
                for key in ["id", "name"] {
                    if let Some(recorded) = block[key].as_str() {
                        part.insert(key.into(), json!(recorded));
                    }
                }
                Value::Object(part)
            }
            "image" => {
                let reference = &block["attachment"];
                let (id, mime) = (
                    text(reference, "attachmentId"),
                    text(reference, "mediaType"),
                );
                image(id, mime, attachments).unwrap_or_else(|| data_part(block))
            }
            _ => data_part(block),
        });
    }
    parts
}

/// Reads an image out of dsh's attachment store, which names stored bytes
/// `sha256:` and their hash, and files each object under the first two
/// characters of that hash.
fn image(id: &str, mime: &str, attachments: &Path) -> Option<Value> {
    let hash = id.strip_prefix("sha256:").filter(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
    })?;
    if attachments.as_os_str().is_empty() {
        return None;
    }
    let file = attachments.join(&hash[..2]).join(hash);
    if fs::metadata(&file).ok()?.len() > 32 << 20 {
        return None;
    }
    let raw = fs::read(&file).ok()?;
    let mime = if mime.starts_with("image/") { mime } else { "" };
    image_part(&base64(&raw), mime)
}

/// Standard base64, padded.
fn base64(raw: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(raw.len().div_ceil(3) * 4);
    for chunk in raw.chunks(3) {
        let bits = chunk
            .iter()
            .fold(0_u32, |bits, byte| bits << 8 | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for index in 0..4 {
            out.push(if index <= chunk.len() {
                ALPHABET[(bits >> (18 - 6 * index)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// What a tool returned: its text when that is all there is, or its parts
/// when it also returned an image.
fn result(parts: Vec<Value>) -> Value {
    if parts.iter().any(|part| text(part, "type") != "text") {
        return Value::Array(parts);
    }
    let texts: Vec<&str> = parts.iter().map(|part| text(part, "text")).collect();
    Value::String(texts.join("\n"))
}

/// The token counts dsh recorded for a reply, under the format's names.
/// `None` when it recorded none of them.
fn usage(recorded: &Value) -> Option<Value> {
    let mut counts = Map::new();
    for (theirs, name) in [
        ("inputTokens", "input_tokens"),
        ("outputTokens", "output_tokens"),
        ("totalTokens", "total_tokens"),
        ("cacheReadTokens", "cache_read_tokens"),
        ("cacheWriteTokens", "cache_write_tokens"),
        ("reasoningTokens", "reasoning_tokens"),
    ] {
        let count = &recorded[theirs];
        if count.as_f64().is_some_and(|count| count >= 0.0) {
            counts.insert(name.to_owned(), count.clone());
        }
    }
    (!counts.is_empty()).then_some(Value::Object(counts))
}

/// Names the model of a call: the one the reply says it came from, or the
/// one the request was prepared for.
fn model(source: &Value, request: &Value) -> Value {
    let named = |key: &str| source[key].as_str().or(request[key].as_str());
    let mut call = Map::new();
    call.insert("id".into(), json!(named("model").unwrap_or("unknown")));
    if let Some(provider) = named("provider").filter(|provider| !provider.is_empty()) {
        call.insert("provider".into(), json!(provider));
    }
    Value::Object(call)
}

/* ------------------------------------------------ where its sessions are */

/// The sessions directory of dsh's home: `DSH_HOME` when it names one,
/// `~/.dsh` otherwise. A profile can be set to keep its logs somewhere else;
/// that is not followed.
pub(crate) fn store() -> PathBuf {
    let configured = env::var_os("DSH_HOME").unwrap_or_default();
    store_for(&configured.to_string_lossy())
}

/// The sessions directory for what `DSH_HOME` holds. dsh reads a blank one
/// as unset, a leading `~` as the home directory, and a path that is not
/// whole from the directory it is run in.
fn store_for(configured: &str) -> PathBuf {
    if configured.trim().is_empty() {
        return clean(&user_home().join(".dsh").join("sessions"));
    }
    let root = expand_home(configured);
    let root = env::current_dir().map_or_else(|_| root.clone(), |cwd| cwd.join(&root));
    clean(&root.join("sessions"))
}

/// A path as its text alone resolves it, the way Go's `filepath.Clean`
/// leaves one: no `.`, each `..` taken with the name before it, and `.` for
/// a path with nothing left. dsh's directory names are made from such text.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// The directory a path is in. For a bare name that is `.`, as it is in Go:
/// a log named without the directories dsh keeps it in has nothing above it
/// to look in.
fn dir_of(path: &Path) -> PathBuf {
    clean(path.parent().unwrap_or(path))
}

/// The last name in a path, or the path itself when it has none.
fn base(path: &Path) -> String {
    path.components()
        .next_back()
        .map(|last| last.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether dsh writes a character into a path as it is.
fn safe(unit: u16) -> bool {
    u8::try_from(unit)
        .is_ok_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Writes one UTF-16 code unit as dsh writes it in a directory name: as it
/// is, or as `~` and its four hexadecimal digits.
fn escape(out: &mut String, unit: u16) {
    if safe(unit) {
        out.push(unit as u8 as char);
    } else {
        write!(out, "~{unit:04X}").expect("writing to a string");
    }
}

/// A session id as dsh writes it for a directory name: every character it
/// does not write as it is becomes `~` and its UTF-16 code unit.
fn segment(id: &str) -> String {
    match id {
        "." => "~002E".to_owned(),
        ".." => "~002E~002E".to_owned(),
        _ => {
            let mut out = String::new();
            for unit in id.encode_utf16() {
                escape(&mut out, unit);
            }
            out
        }
    }
}

/// The directory dsh groups a project's sessions under: the path with each
/// run of separators as one dash and other characters escaped as in a
/// session id, cut to 251 characters, between double dashes. Paths that
/// differ only in what was replaced or cut share a directory, so it is each
/// session's header that says which project it belongs to.
fn project_key(cwd: &Path) -> String {
    let mut readable = String::new();
    let mut separators = false;
    for unit in cwd.to_string_lossy().encode_utf16() {
        if matches!(u8::try_from(unit), Ok(b'/' | b'\\' | b':')) {
            if !separators {
                readable.push('-');
            }
            separators = true;
            continue;
        }
        escape(&mut readable, unit);
        separators = false;
    }
    let slug = match readable.trim_start_matches('-') {
        "" => "root",
        slug => slug,
    };
    format!("--{}--", &slug[..slug.len().min(251)])
}

/// The format generation in the name of a session's log: `session.jsonl` or
/// `session.v<n>.jsonl`, with `.zstd` after it when compressed. `None` for
/// any other file.
fn generation(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("session")?;
    let rest = rest.strip_suffix(".zstd").unwrap_or(rest);
    let version = rest.strip_suffix(".jsonl")?;
    if version.is_empty() {
        return Some(0);
    }
    let digits = version.strip_prefix(".v")?;
    let counted = !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit());
    // A number too long to hold is still the highest there is.
    counted.then(|| digits.parse().unwrap_or(i64::MAX))
}

/// The log to read in a session's directory: the highest format generation
/// there, which is the one dsh reads.
fn log(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, i64, i64)> = None;
    for entry in sorted_entries(dir) {
        let Some(version) = entry
            .file_name()
            .and_then(|name| generation(&name.to_string_lossy()))
        else {
            continue;
        };
        // A link to a log is not one.
        if !fs::symlink_metadata(&entry).is_ok_and(|info| info.is_file()) {
            continue;
        }
        let written = modified(&entry).unwrap_or(0);
        if best.as_ref().is_none_or(|(_, highest, newest)| {
            version > *highest || (version == *highest && written > *newest)
        }) {
            best = Some((entry, version, written));
        }
    }
    best.map(|(file, _, _)| file)
}

/// The logs of the sessions dsh ran in `cwd`. A sub-agent's session is one
/// of them only when asked for. dsh records the directory it was started in
/// with links resolved, so `cwd` is looked up that way too.
fn sessions(root: &Path, cwd: &Path, sub_agents: bool) -> Vec<PathBuf> {
    let mut dirs = vec![clean(cwd)];
    if let Ok(real) = fs::canonicalize(cwd)
        && real != dirs[0]
    {
        dirs.push(real);
    }
    let mut files = Vec::new();
    for dir in dirs {
        for session in sorted_entries(&root.join(project_key(&dir))) {
            let Some(file) = log(&session) else {
                continue;
            };
            let header = first_json_line(&file);
            if clean(Path::new(text(&header, "cwd"))) == dir
                && (sub_agents || text(&header, "origin") != "subagent")
            {
                files.push(file);
            }
        }
    }
    files
}

/// A sub-agent a session started: its session id, what the parent called it
/// and its own log.
struct Child {
    id: String,
    label: String,
    file: PathBuf,
}

/// The sub-agents a session started. dsh notes each one's session id and
/// label in the parent's log; the sub-agent's own log is in the same store,
/// in the directory named for that id, under the project it worked in.
fn children(transcript: &Path) -> Vec<Child> {
    let Ok(lines) = read_lines(transcript) else {
        return Vec::new();
    };
    let project = dir_of(&dir_of(transcript));
    let root = dir_of(&project);
    let mut projects = vec![project.clone()];
    projects.extend(
        sorted_entries(&root)
            .into_iter()
            .filter(|dir| *dir != project),
    );
    let mut children: Vec<Child> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for line in &lines {
        if !line.contains("\"subagent/catalog\"") && !line.contains("\"session/end-seed\"") {
            continue;
        }
        let Ok(event) = slink_format::parse(line) else {
            continue;
        };
        let data = &event["data"];
        // A forked session begins with a copy of its parent's history, the
        // parent's sub-agents included. Its own come after the copy ends.
        if text(&event, "type") == "session/end-seed" && data["inherited"] == true {
            children.clear();
            seen.clear();
        }
        if text(&event, "type") != "subagent/catalog" {
            continue;
        }
        let id = text(data, "childId");
        if id.is_empty() || !seen.insert(id.to_owned()) {
            continue;
        }
        if let Some(file) = projects.iter().find_map(|dir| log(&dir.join(segment(id)))) {
            children.push(Child {
                id: id.to_owned(),
                label: text(data, "label").to_owned(),
                file,
            });
        }
    }
    children
}

/// The sessions of a session's sub-agents, and of theirs.
fn agents(transcript: &Path, depth: usize) -> Vec<Agent> {
    let mut found = Vec::new();
    for child in children(transcript) {
        let Ok(lines) = read_lines(&child.file) else {
            continue;
        };
        let theirs = if depth < 8 {
            agents(&child.file, depth + 1)
        } else {
            Vec::new()
        };
        found.push(Agent {
            call: String::new(),
            name: child.label,
            input: Input {
                lines,
                fallback: child.id.clone(),
                blobs: attachments(&child.file),
                agents: theirs,
                ..Input::default()
            },
            id: child.id,
        });
    }
    found
}

/// The logs of a session's sub-agents, and of theirs, as far down as the
/// import reads them. A preview made from the session depends on every one
/// of them. Go's listing has no such end: two sessions that each name the
/// other as a sub-agent send it down until its stack is gone.
fn agent_files(transcript: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for child in children(transcript) {
        let theirs = if depth < 8 {
            agent_files(&child.file, depth + 1)
        } else {
            Vec::new()
        };
        files.push(child.file);
        files.extend(theirs);
    }
    files
}

/// Where dsh keeps the images a log refers to: its attachment store, beside
/// the sessions directory.
fn attachments(transcript: &Path) -> PathBuf {
    let home = dir_of(&dir_of(&dir_of(&dir_of(transcript))));
    clean(&home.join("attachments").join("v1").join("objects"))
}

/// What a listing shows of a session: the title dsh gave it, the first thing
/// the person typed, and when it began.
fn titles(file: &Path) -> (String, String, Option<(i64, u32)>) {
    let lines = read_lines(file).unwrap_or_default();
    let Some((header, events)) = lines.split_first() else {
        return (String::new(), String::new(), None);
    };
    let started = slink_format::parse(header)
        .ok()
        .and_then(|header| header["createdAt"].as_f64())
        .filter(|ms| *ms > 0.0)
        .map(|ms| ms as i64)
        .map(|ms| (ms / 1000, (ms % 1000) as u32 * 1_000_000));
    let (mut name, mut prompt) = (String::new(), String::new());
    for line in events {
        let titled = line.contains("\"session/title\"");
        let typed = prompt.is_empty() && line.contains("\"user/message\"");
        if !titled && !typed {
            continue;
        }
        let Ok(event) = slink_format::parse(line) else {
            continue;
        };
        let data = &event["data"];
        match text(&event, "type") {
            "session/title" => {
                if let Some(given) = given_title(data) {
                    name = given.to_owned();
                }
            }
            "user/message" if prompt.is_empty() && text(&data["source"], "kind") == "user" => {
                prompt = first_text(&data["content"]).to_owned();
            }
            _ => {}
        }
    }
    (name, prompt, started)
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    latest_in(&store(), cwd)
}

/// The session dsh last wrote for `cwd` under `root`, a sub-agent's left
/// out.
fn latest_in(root: &Path, cwd: &Path) -> Option<Found> {
    newest_of(sessions(root, cwd, false), "dsh")
}

pub(crate) fn anywhere() -> Option<Elsewhere> {
    anywhere_in(&store())
}

/// The newest session under `root` that is not a sub-agent's, whichever
/// project it is filed under.
fn anywhere_in(root: &Path) -> Option<Elsewhere> {
    let mut best: Option<(PathBuf, i64)> = None;
    for project in sorted_entries(root) {
        for session in sorted_entries(&project) {
            let Some(file) = log(&session) else {
                continue;
            };
            if let Some(written) = modified(&file)
                && written > best.as_ref().map_or(0, |best| best.1)
                && text(&first_json_line(&file), "origin") != "subagent"
            {
                best = Some((file, written));
            }
        }
    }
    let (file, recency) = best?;
    let (name, prompt, _) = titles(&file);
    Some(Elsewhere {
        harness: "dsh",
        dir: text(&first_json_line(&file), "cwd").to_owned(),
        title: if name.is_empty() { prompt } else { name },
        file: Some(file),
        recency,
    })
}

/// The sessions under `store` for each of `dirs`; one named by `id` only,
/// when an id is given.
pub(crate) fn candidates(
    store: &Path,
    dirs: &[PathBuf],
    id: &str,
) -> Result<Vec<Candidate>, String> {
    let mut found = Vec::new();
    for dir in dirs {
        // Only an explicit id reaches a sub-agent's own session.
        for file in sessions(store, dir, !id.is_empty()) {
            let session = match first_json_line(&file)["id"].as_str() {
                Some(session) => session.to_owned(),
                None => base(&dir_of(&file)),
            };
            if id.is_empty() || session == id {
                found.extend(Candidate::file("dsh", &file, dir, session));
            }
        }
    }
    Ok(found)
}

/// Fills in a candidate's title, first prompt, start and sub-agent files.
pub(crate) fn describe(candidate: &mut Candidate) {
    let Some(file) = candidate.file.clone() else {
        return;
    };
    candidate.related = agent_files(&file, 0);
    (candidate.name, candidate.prompt, candidate.started) = titles(&file);
    candidate.title = if candidate.name.is_empty() {
        candidate.prompt.clone()
    } else {
        candidate.name.clone()
    };
}

/// A session log's lines with its attachments and its sub-agents' sessions.
/// The session's directory names it when its header does not.
pub(crate) fn input(file: &Path, lines: Vec<String>) -> Input {
    Input {
        lines,
        fallback: base(&dir_of(file)),
        blobs: attachments(file),
        agents: agents(file, 0),
        ..Input::default()
    }
}

/// Whether a transcript's first entry is a dsh session header: a "session"
/// line, as pi's is, that also says whether it was seeded from another.
pub(crate) fn recognizes(head: &[Value]) -> bool {
    head.first()
        .is_some_and(|entry| entry.get("isSeeded").is_some() && entry["type"] == "session")
}

/// The session whose agent is running this process, when dsh says.
pub(crate) fn active() -> Option<Found> {
    active_in(&store(), &active_id()?)
}

/// The id of the session whose agent is running this process. dsh gives
/// every command of its shell tool the id of the session that ran it in
/// `DSH_SESSION_ID`, beside `DSH_SHELL` and its own home.
pub(crate) fn active_id() -> Option<String> {
    let id = env::var("DSH_SESSION_ID").ok()?;
    let shell = env::var_os("DSH_SHELL").unwrap_or_default();
    (!id.is_empty() && !shell.is_empty()).then_some(id)
}

/// The session `id` names under `root`, as it is now: whichever project it
/// is filed under, and whichever session was written to last.
fn active_in(root: &Path, id: &str) -> Option<Found> {
    Found::file("dsh", &by_id(root, id)?)
}

/// A session's log by its id, whichever project it is filed under.
fn by_id(root: &Path, id: &str) -> Option<PathBuf> {
    sorted_entries(root)
        .into_iter()
        .filter_map(|project| log(&project.join(segment(id))))
        .find(|file| text(&first_json_line(file), "id") == id)
}

/// The log of a pasted session id, for one working directory.
pub(crate) fn transcript(cwd: &Path, id: &str) -> Option<PathBuf> {
    transcript_in(&store(), cwd, id)
}

/// The log of the session with this id among the sessions of `cwd` under
/// `root`, a sub-agent's included.
fn transcript_in(root: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    sessions(root, cwd, true)
        .into_iter()
        .find(|file| text(&first_json_line(file), "id") == id)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    const TYPED: &str = r#"{"type":"user/message","seq":8,"time":1791451208534,"data":{"content":[{"type":"text","text":"Find the race in the upload test"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}"#;
    const TITLED: &str = r#"{"type":"session/title","seq":18,"time":1791451208588,"data":{"title":"Upload test race","messageSeqs":[8],"source":{"kind":"provider","provider":"session-title-first-prompt-llm"}}}"#;

    /// A store of one test's own, empty. A unit test is given no directory
    /// under the build's, so it is under the system's.
    fn scratch(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("slink-import-dsh-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("sessions")
    }

    /// Writes a small v4 log for a session of `cwd`, as it was at one
    /// moment, and says where.
    fn write_log(
        root: &Path,
        id: &str,
        cwd: &str,
        header: &str,
        events: &[&str],
        at: SystemTime,
    ) -> PathBuf {
        let file = root
            .join(project_key(Path::new(cwd)))
            .join(segment(id))
            .join("session.v4.jsonl");
        let mut lines = vec![format!(
            r#"{{"type":"session","version":4,"id":"{id}","createdAt":1791451208304,"cwd":{},"isSeeded":false,"delegationDepth":0{header}}}"#,
            json!(cwd)
        )];
        lines.extend(events.iter().map(|event| (*event).to_owned()));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, lines.join("\n") + "\n").unwrap();
        let log = fs::File::options().write(true).open(&file).unwrap();
        log.set_modified(at).unwrap();
        file
    }

    /// Two sessions that each name the other as a sub-agent are a store
    /// dsh would not write, and one a listing must still come back from.
    #[test]
    fn sessions_that_name_each_other_as_sub_agents_end() {
        let root = scratch("each-other");
        let spawned = |child: &str| {
            format!(
                r#"{{"type":"subagent/catalog","seq":17,"time":1791451208590,"data":{{"childId":"{child}","label":"Again"}}}}"#
            )
        };
        let now = SystemTime::now();
        let first = write_log(
            &root,
            "first",
            "/work/loop",
            "",
            &[TYPED, &spawned("second")],
            now,
        );
        let second = write_log(
            &root,
            "second",
            "/work/loop",
            "",
            &[TYPED, &spawned("first")],
            now,
        );
        // As far down as the import goes, and no further: the two logs by
        // turns, nine deep.
        let files = agent_files(&first, 0);
        assert_eq!(files.len(), 9);
        assert!(
            files.iter().step_by(2).all(|file| *file == second),
            "{files:?}"
        );
        assert_eq!(agents(&first, 0).len(), 1);
        // The listing a sessions page asks for comes back too.
        let mut listed = candidates(&root, &[PathBuf::from("/work/loop")], "first").unwrap();
        assert_eq!(listed.len(), 1);
        describe(&mut listed[0]);
        assert_eq!(listed[0].related.len(), 9);
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    /// The id in the header of the log a session is read from.
    fn id_of(found: &Found) -> String {
        let read = found.load().unwrap();
        text(&slink_format::parse(&read.lines[0]).unwrap(), "id").to_owned()
    }

    #[test]
    fn directories_are_named_as_dsh_names_them() {
        for (cwd, key) in [
            ("/work/checks", "--work-checks--"),
            // A dash after a separator stays.
            ("/work/a/-b", "--work-a--b--"),
            ("/work//double", "--work-double--"),
            ("/work/my app/\u{fc}", "--work-my~0020app-~00FC--"),
            ("C:\\Users\\dev\\proj", "--C-Users-dev-proj--"),
            ("/", "--root--"),
            ("/work/tilde~1", "--work-tilde~007E1--"),
        ] {
            assert_eq!(project_key(Path::new(cwd)), key, "{cwd}");
        }
        let long = format!("/{}", "a".repeat(300));
        assert_eq!(
            project_key(Path::new(&long)),
            format!("--{}--", "a".repeat(251))
        );
        for (id, name) in [
            ("session-449e7620-3a4f", "session-449e7620-3a4f"),
            ("a/b", "a~002Fb"),
            ("..", "~002E~002E"),
            (".", "~002E"),
            ("x~y z", "x~007Ey~0020z"),
            // By UTF-16 code unit, as dsh does.
            ("\u{1f600}", "~D83D~DE00"),
        ] {
            assert_eq!(segment(id), name, "{id}");
        }
    }

    #[test]
    fn the_store_follows_dsh_home() {
        let home = user_home();
        let default = clean(&home.join(".dsh").join("sessions"));
        assert_eq!(store_for(""), default);
        // dsh treats a blank DSH_HOME as unset, and expands a leading ~.
        assert_eq!(store_for("   "), default);
        assert_eq!(
            store_for("~/work-dsh"),
            clean(&home.join("work-dsh").join("sessions"))
        );
        let other = env::temp_dir().join("other-dsh");
        assert_eq!(
            store_for(&other.to_string_lossy()),
            clean(&other.join("sessions"))
        );
        // One that is not a whole path is found from the directory dsh is
        // run in, and is read as text: no `..` is left in it.
        let here = env::current_dir().unwrap();
        assert_eq!(
            store_for("dsh/x/../home"),
            clean(&here.join("dsh").join("home").join("sessions"))
        );
    }

    #[test]
    fn a_path_is_resolved_from_its_text_as_go_resolves_it() {
        let shown = |path: PathBuf| path.to_string_lossy().replace('\\', "/");
        for (path, cleaned) in [
            ("", "."),
            (".", "."),
            ("./x", "x"),
            ("a/..", "."),
            ("a/./b//c/", "a/b/c"),
            ("../a", "../a"),
            ("../..", "../.."),
            ("a/../..", ".."),
            ("/..", "/"),
            ("/a/../..", "/"),
            ("/a/b/../c", "/a/c"),
        ] {
            assert_eq!(shown(clean(Path::new(path))), cleaned, "{path:?}");
        }
        for (path, dir) in [
            ("/s/p/x/session.jsonl", "/s/p/x"),
            ("x/session.jsonl", "x"),
            ("session.jsonl", "."),
            ("./session.jsonl", "."),
            ("..", "."),
            (".", "."),
            ("/a", "/"),
            ("/", "/"),
        ] {
            assert_eq!(shown(dir_of(Path::new(path))), dir, "{path:?}");
        }
        for (path, name) in [("/a/b", "b"), ("b", "b"), (".", "."), ("..", "..")] {
            assert_eq!(base(Path::new(path)), name, "{path:?}");
        }
        assert_eq!(base(Path::new("/")).replace('\\', "/"), "/");
    }

    #[test]
    fn a_log_is_known_by_its_name_and_its_generation() {
        for (name, version) in [
            ("session.jsonl", Some(0)),
            ("session.jsonl.zstd", Some(0)),
            ("session.v4.jsonl", Some(4)),
            ("session.v4.jsonl.zstd", Some(4)),
            ("session.v10.jsonl", Some(10)),
            ("session.v99999999999999999999.jsonl", Some(i64::MAX)),
            ("session.v04.jsonl", None),
            ("session.v0.jsonl", None),
            ("session.v.jsonl", None),
            ("session.v4a.jsonl", None),
            ("session.v+4.jsonl", None),
            ("session.v4.jsonl.bak", None),
            ("session.v4.jsonl.zstd.zstd", None),
            ("session.v4.json", None),
            ("session.zstd", None),
            ("session", None),
            ("sessions.v4.jsonl", None),
            ("my-session.v4.jsonl", None),
            ("session.v4.jsonl\n", None),
        ] {
            assert_eq!(generation(name), version, "{name:?}");
        }
    }

    #[test]
    fn a_time_is_written_as_go_writes_it() {
        for (ms, written) in [
            (0, "1970-01-01T00:00:00.000Z"),
            (1_791_451_208_304, "2026-10-08T09:20:08.304Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            (4_102_444_800_000, "2100-01-01T00:00:00.000Z"),
            (-11_644_473_600_000, "1601-01-01T00:00:00.000Z"),
            // Past the years JavaScript writes with four digits, Go goes on
            // counting; these are what it writes.
            (253_402_300_800_000, "10000-01-01T00:00:00.000Z"),
            (8_640_000_000_000_000, "275760-09-13T00:00:00.000Z"),
            (-62_167_219_200_000, "0000-01-01T00:00:00.000Z"),
            (-62_167_219_200_001, "-0001-12-31T23:59:59.999Z"),
            (i64::MAX, "292278994-08-17T07:12:55.807Z"),
            (i64::MIN, "-292275055-05-16T16:47:04.192Z"),
        ] {
            assert_eq!(iso(ms), written, "{ms}");
        }
        // A time is cut to its millisecond, and one that is no number is
        // none at all.
        assert_eq!(
            millis(&json!(1_709_164_800_000.999_f64)),
            Some(1_709_164_800_000)
        );
        assert_eq!(millis(&json!(-1.9)), Some(-1));
        assert_eq!(millis(&json!(1e300)), Some(i64::MAX));
        assert_eq!(millis(&json!("1791451208304")), None);
        assert_eq!(millis(&Value::Null), None);
    }

    #[test]
    fn stored_bytes_are_written_as_base64() {
        for (raw, written) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"foobar", "Zm9vYmFy"),
            (b"\xfb\xff\xbe", "+/++"),
            (b"\xff\xff", "//8="),
        ] {
            assert_eq!(base64(raw), written, "{raw:?}");
        }
    }

    #[test]
    fn an_image_is_read_only_from_the_store_and_by_its_hash() {
        let objects = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/import/dsh/v4-images/attachments/v1/objects");
        let hash = "3fd6e6be528c182d768563a63b65ac5a70d022149a01eeeaaa30396d75f426e0";
        let stored = image(&format!("sha256:{hash}"), "", &objects).unwrap();
        // Its kind is told from its bytes when dsh did not say.
        assert_eq!(stored["mime"], "image/png");
        assert!(text(&stored, "url").starts_with("data:image/png;base64,iVBOR"));
        assert_eq!(
            image(&format!("sha256:{hash}"), "text/plain", &objects),
            Some(stored)
        );
        for id in [
            hash.to_owned(),
            format!("sha1:{hash}"),
            format!("sha256:{}", hash.to_uppercase()),
            format!("sha256:{hash}0"),
            format!("sha256:{}", &hash[1..]),
            format!("sha256:{}", "ab".repeat(32)),
            format!("sha256:../{}", &hash[3..]),
        ] {
            assert_eq!(image(&id, "image/png", &objects), None, "{id}");
        }
        assert_eq!(
            image(&format!("sha256:{hash}"), "image/png", Path::new("")),
            None
        );
    }

    #[test]
    fn the_newest_session_of_a_project_and_a_log_by_its_id() {
        let root = scratch("found");
        let project = "/work/upload";
        let now = SystemTime::now();
        let (minute, hour) = (Duration::from_secs(60), Duration::from_secs(3600));
        let top = write_log(&root, "session-top", project, "", &[TYPED, TITLED], now);
        let sub_agent = r#","parentSession":"session-top","origin":"subagent""#;
        let child = write_log(&root, "child-1", project, sub_agent, &[TYPED], now + minute);
        let untitled = write_log(&root, "session-untitled", project, "", &[TYPED], now - hour);
        // "/work:upload" is filed in the same directory; its header says it
        // is another project's.
        write_log(
            &root,
            "session-other",
            "/work:upload",
            "",
            &[TYPED],
            now + hour,
        );
        write_log(
            &root,
            "session-far",
            "/work/elsewhere",
            "",
            &[TYPED],
            now - hour,
        );

        // A sub-agent's session is not the project's newest, though it was
        // written last.
        let newest = latest_in(&root, Path::new(project)).unwrap();
        assert_eq!(
            (newest.harness, id_of(&newest)),
            ("dsh", "session-top".to_owned())
        );
        assert!(latest_in(&root, Path::new("/work")).is_none());
        assert!(latest_in(&root, Path::new("/work/upload/deeper")).is_none());
        // A log by its session id, a sub-agent's included.
        let by_id = |id: &str| transcript_in(&root, Path::new(project), id);
        assert_eq!(by_id("session-untitled"), Some(untitled));
        assert_eq!(by_id("child-1"), Some(child));
        assert_eq!(by_id("session-far"), None);
        assert_eq!(by_id("session-other"), None);

        // The newest anywhere is whichever project's, and is no sub-agent's.
        let elsewhere = anywhere_in(&root).unwrap();
        assert_eq!(elsewhere.dir, "/work:upload");
        assert_eq!(elsewhere.title, "Find the race in the upload test");
        fs::remove_dir_all(
            root.join(project_key(Path::new("/work:upload")))
                .join("session-other"),
        )
        .unwrap();
        let elsewhere = anywhere_in(&root).unwrap();
        assert_eq!(elsewhere.dir, project);
        assert_eq!(elsewhere.title, "Upload test race");
        assert_eq!(elsewhere.file, Some(top));
        assert!(anywhere_in(&root.join("absent")).is_none());
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_project_is_looked_up_with_its_links_resolved() {
        let root = scratch("linked");
        let home = root.parent().unwrap();
        let (real, link) = (home.join("real"), home.join("link"));
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // dsh records the directory with links resolved, and files the
        // session under that name.
        let real = fs::canonicalize(&real).unwrap();
        let cwd = real.to_string_lossy();
        let log = write_log(&root, "session-real", &cwd, "", &[TYPED], SystemTime::now());
        assert_eq!(sessions(&root, &link, false), std::slice::from_ref(&log));
        assert_eq!(sessions(&root, &real, false), std::slice::from_ref(&log));
        assert_eq!(transcript_in(&root, &link, "session-real"), Some(log));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_command_dsh_runs_is_about_the_session_running_it() {
        let root = scratch("running");
        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        write_log(
            &root,
            "session-running",
            "/work/upload",
            "",
            &[TYPED],
            now - hour,
        );
        write_log(&root, "session-newer", "/work/upload", "", &[TYPED], now);
        write_log(&root, "a/b", "/work/other", "", &[TYPED], now - hour);
        // Outside dsh, the newest session of the project is the one.
        let newest = latest_in(&root, Path::new("/work/upload")).unwrap();
        assert_eq!(id_of(&newest), "session-newer");
        // From dsh's shell tool it is the session running the command,
        // whichever was written to last and wherever the tool is working.
        let running = active_in(&root, "session-running").unwrap();
        assert_eq!(
            (running.harness, id_of(&running)),
            ("dsh", "session-running".to_owned())
        );
        assert_eq!(id_of(&active_in(&root, "a/b").unwrap()), "a/b");
        // An id that names no session leaves the newest to be found.
        assert!(active_in(&root, "session-gone").is_none());
        assert!(active_in(&root, "").is_none());
        assert!(active_in(&root, "..").is_none());
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }
}
