//! pi and omp. A pi transcript is one JSON entry on each line. pi stores a
//! session as a tree, and the import is the branch that ends at the file's
//! last entry, the one pi itself resumes; turns on branches the person went
//! back from are counted, not replayed. User and tool-result messages gather
//! as what the next model call was given, each assistant message becomes an
//! `llm_call`, and each of its `toolCall` blocks a `tool_call` span that the
//! matching tool result closes.
//!
//! omp (oh-my-pi) grew out of pi and keeps its transcript: the same header,
//! tree of entries and message shapes. What it adds is noted where it
//! matters.
//!
//! The importer was first written in JavaScript, where `JSON.stringify`
//! drops an object's key whose value is undefined and keeps one that is
//! null. An `Option<&Value>` holds that distinction here: `None` for a key
//! that is absent, `Some(Null)` for a null.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, MAIN_SEPARATOR_STR, Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::discover::{
    Candidate, Elsewhere, Found, blobs_beside, first_json_line, newest_file, newest_of, peek,
    read_lines, sorted_entries, stem, tail_lines, transcripts_in, user_home,
};
use crate::images::image_part;
use crate::value::{data_part, field, put, text, truthy};
use crate::{Agent, Input};

pub(crate) const PORTED: bool = true;

/// `harness` is `pi` or `omp`: omp's transcript is pi's with a title line of
/// its own, and it moves images into a store beside the transcripts.
pub(crate) fn import(input: &Input, harness: &str) -> Result<Option<Value>, String> {
    Ok(to_run(input, harness))
}

/* ------------------------------------------- JavaScript's values, in JSON */

/// A key of a JavaScript object literal on its way to `JSON.stringify`: an
/// undefined value is left out, and every other, null included, is kept.
fn set(target: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        target.insert(key.to_owned(), value.clone());
    }
}

/// JavaScript's `x == null`, the test behind `??`: null or undefined.
fn nullish(value: Option<&Value>) -> bool {
    value.is_none_or(Value::is_null)
}

/// JavaScript's `String(x)`, for a model or a provider that is not text.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(set) => set.to_string(),
        // Every JSON number is a float64 to Go, and is written as one.
        Value::Number(number) => {
            slink_format::encode(&Value::from(number.as_f64().unwrap_or_default()))
        }
        Value::Null => "null".to_owned(),
        // JavaScript says `[object Object]` here. The Go importer prints the
        // value the way Go prints one, and its output is the one to match;
        // no real transcript has an object for a model.
        other => go_print(other),
    }
}

/// JavaScript's `String(x ?? "unknown")`.
fn model_text(value: Option<&Value>) -> String {
    match value {
        Some(value) if !value.is_null() => js_string(value),
        _ => "unknown".to_owned(),
    }
}

/// A value as Go's `%v` prints what it decoded from JSON: an array as its
/// items between brackets, an object as `map[key:value ...]` with its keys
/// in order, and nothing quoted.
fn go_print(value: &Value) -> String {
    match value {
        Value::Null => "<nil>".to_owned(),
        Value::Bool(set) => set.to_string(),
        Value::Number(number) => go_float(number.as_f64().unwrap_or_default()),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(go_print).collect();
            format!("[{}]", items.join(" "))
        }
        Value::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(key, field)| format!("{key}:{}", go_print(field)))
                .collect();
            format!("map[{}]", fields.join(" "))
        }
    }
}

/// A float64 as Go's `%v` prints it: the shortest digits that read back as
/// the same number, with an exponent of at least two digits below 1e-4 and
/// from 1e6.
fn go_float(float: f64) -> String {
    let scientific = format!("{float:e}");
    let (digits, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or_default();
    if (-4..6).contains(&exponent) {
        format!("{float}")
    } else {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{digits}e{sign}{:02}", exponent.abs())
    }
}

/* ------------------------------------------------------------ the mapping */

/// JavaScript's `x ?? ""`: the field, or empty text for one that is missing
/// or null.
fn or_empty(value: &Value, name: &str) -> Value {
    field(value, name).cloned().unwrap_or_else(|| json!(""))
}

/// One block of a message's content as a part.
fn part(block: &Value) -> Value {
    let Some(kind) = block.get("type").and_then(Value::as_str) else {
        return data_part(block);
    };
    match kind {
        "text" => json!({"type": "text", "text": or_empty(block, "text")}),
        "thinking" => json!({"type": "thinking", "text": or_empty(block, "thinking")}),
        "toolCall" => {
            let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
                return data_part(block);
            };
            let mut call = Map::new();
            call.insert("type".into(), json!("tool_call"));
            call.insert("id".into(), json!(id));
            call.insert("name".into(), json!(name));
            put(&mut call, "arguments", block, "arguments");
            Value::Object(call)
        }
        // The format is open to parts it does not know: any other block is
        // kept as it is.
        _ => block.clone(),
    }
}

/// A message's content as parts.
fn content(recorded: Option<&Value>) -> Vec<Value> {
    match recorded {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(said)) => vec![json!({"type": "text", "text": said})],
        Some(Value::Array(blocks)) => blocks.iter().map(part).collect(),
        Some(other) => vec![data_part(other)],
    }
}

fn usage(recorded: &Value) -> Option<Value> {
    let mut usage = Map::new();
    for (theirs, ours) in [
        ("input", "input_tokens"),
        ("output", "output_tokens"),
        ("cacheRead", "cache_read_tokens"),
    ] {
        if let Some(count) = field(recorded, theirs) {
            usage.insert(ours.to_owned(), count.clone());
        }
    }
    (!usage.is_empty()).then_some(Value::Object(usage))
}

/// Standard base64, padded.
fn base64(raw: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(raw.len().div_ceil(3) * 4);
    for chunk in raw.chunks(3) {
        let bits = chunk
            .iter()
            .fold(0_u32, |bits, byte| (bits << 8) | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for sextet in 0..4 {
            encoded.push(if sextet <= chunk.len() {
                ALPHABET[(bits >> (18 - 6 * sextet)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    encoded
}

/// An image omp moved to its blob store, as base64. omp names a blob by the
/// SHA-256 of its bytes, and nothing else is looked for there.
fn blob(blobs: &Path, hash: &str) -> Option<String> {
    let named = hash.len() == 64
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'f' | b'0'..=b'9'));
    if blobs.as_os_str().is_empty() || !named {
        return None;
    }
    let file = blobs.join(hash);
    if fs::metadata(&file).ok()?.len() > 32 << 20 {
        return None;
    }
    fs::read(&file).ok().map(|raw| base64(&raw))
}

/// Turns the images in mapped content into image parts. pi records an image
/// as base64 with its type. omp moves the bytes to its blob store and leaves
/// a reference, which resolves while that store is beside the transcript. An
/// image that cannot be shown is kept as recorded, as data: an image part
/// has to say where its image is.
fn images(mut parts: Vec<Value>, blobs: &Path) -> Vec<Value> {
    for part in &mut parts {
        if text(part, "type") != "image" {
            continue;
        }
        let recorded = text(part, "data");
        let stored;
        let data = match recorded.strip_prefix("blob:sha256:") {
            Some(hash) => {
                stored = blob(blobs, hash).unwrap_or_default();
                stored.as_str()
            }
            None => recorded,
        };
        let image = image_part(data, text(part, "mimeType")).unwrap_or_else(|| data_part(part));
        *part = image;
    }
    parts
}

/// What a tool gave back. It is usually a single text part, and that is
/// given plainly; anything else keeps its parts. `None` is a text part with
/// no text at all, which JavaScript leaves out of the span.
fn tool_result(recorded: Option<&Value>) -> Option<Value> {
    if let Some([only]) = recorded.and_then(Value::as_array).map(Vec::as_slice)
        && text(only, "type") == "text"
    {
        return only.get("text").cloned();
    }
    Some(recorded.cloned().unwrap_or(Value::Null))
}

/// The first text in some content that is more than white space.
fn first_text(recorded: Option<&Value>) -> Option<String> {
    content(recorded)
        .iter()
        .filter(|part| text(part, "type") == "text")
        .map(|part| text(part, "text").trim())
        .find(|said| !said.is_empty())
        .map(str::to_owned)
}

/// The entries on the path from a session's last entry back to its first,
/// oldest first. Going back to an earlier point and carrying on from there
/// leaves the turns that were given up in the file, as siblings. A session
/// from before pi recorded the tree, or one whose links do not resolve, is a
/// single branch in file order.
fn active_branch(entries: &[Value]) -> Vec<&Value> {
    let Some(mut entry) = entries.last() else {
        return Vec::new();
    };
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for entry in entries {
        if let Some(id) = entry["id"].as_str() {
            by_id.insert(id, entry);
        }
    }
    let mut branch = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    loop {
        // An entry with no id, or one the path has already passed.
        if !entry["id"].as_str().is_some_and(|id| seen.insert(id)) {
            return entries.iter().collect();
        }
        branch.push(entry);
        let Some(parent) = entry["parentId"].as_str() else {
            break;
        };
        match by_id.get(parent) {
            Some(parent) => entry = parent,
            None => return entries.iter().collect(),
        }
    }
    branch.reverse();
    branch
}

fn is_message(entry: &Value) -> bool {
    text(entry, "type") == "message" && truthy(entry.get("message"))
}

fn to_run(input: &Input, harness: &str) -> Option<Value> {
    let mut session = Value::Null;
    // Every entry after the header, in file order.
    let mut recorded: Vec<Value> = Vec::new();
    let mut session_name = String::new();
    let mut messages = 0_usize;
    // omp: the title line it rewrites in place, and its latest title change.
    let (mut slot_title, mut changed_title) = (String::new(), String::new());
    for line in &input.lines {
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        match text(&entry, "type") {
            "" => continue,
            "session" => {
                session = entry;
                continue;
            }
            "title" => {
                slot_title = text(&entry, "title").trim().to_owned();
                continue;
            }
            "title_change" => changed_title = text(&entry, "title").trim().to_owned(),
            // The latest one names the session; an empty name clears it.
            "session_info" => session_name = text(&entry, "name").trim().to_owned(),
            _ => {}
        }
        if is_message(&entry) {
            messages += 1;
        }
        recorded.push(entry);
    }
    let branch = active_branch(&recorded);
    let entries: Vec<&Value> = branch
        .iter()
        .copied()
        .filter(|entry| is_message(entry))
        .collect();
    let (first, last) = (*entries.first()?, *entries.last()?);

    // Named after the opening user turn, cut to a title: pi has no summary
    // line. Only the first user entry is asked, with text or without.
    let mut name = input.fallback.clone();
    if let Some(opener) = entries
        .iter()
        .find(|entry| text(&entry["message"], "role") == "user")
        && let Some(asked) = first_text(opener["message"].get("content"))
    {
        // JavaScript measures and cuts a string in UTF-16 code units.
        let units: Vec<u16> = asked.encode_utf16().collect();
        name = if units.len() > 80 {
            String::from_utf16_lossy(&units[..77]) + "…"
        } else {
            asked
        };
    }
    // A recorded name beats the opening prompt: pi's session name, or omp's
    // current title, its latest title change, then the title in its header.
    let header_title = text(&session, "title").trim().to_owned();
    if let Some(recorded) = [session_name, slot_title, changed_title, header_title]
        .into_iter()
        .find(|name| !name.is_empty())
    {
        name = recorded;
    }

    let started = field(&session, "timestamp").or_else(|| first.get("timestamp"));
    let mut root = Map::new();
    root.insert("id".into(), json!("root"));
    root.insert("parent_id".into(), Value::Null);
    root.insert("type".into(), json!("agent"));
    root.insert("name".into(), json!(name));
    root.insert("status".into(), json!("ok"));
    set(&mut root, "started_at", started);
    put(&mut root, "ended_at", last, "timestamp");
    let mut spans: Vec<Value> = vec![Value::Object(root)];

    let mut pending: Vec<Value> = Vec::new();
    // omp: sub-agent id -> the task call that started it.
    let mut agent_calls: HashMap<String, String> = HashMap::new();
    // When the first message of what is pending was written.
    let mut pending_start: Option<&Value> = None;
    let mut previous = first.get("timestamp");
    // toolCallId -> where its span is in `spans`.
    let mut open_tools: HashMap<String, usize> = HashMap::new();
    let mut sequence = 0_u64;

    for entry in branch.iter().copied() {
        let stamp = entry.get("timestamp");
        let kind = text(entry, "type");
        // A summary pi wrote in place of turns it compacted away, or of a
        // branch the person went back from, is context the model was given.
        if kind == "compaction" || kind == "branch_summary" {
            let summary = text(entry, "summary");
            if !summary.is_empty() {
                if nullish(pending_start) {
                    pending_start = stamp;
                }
                pending.push(
                    json!({"role": "system", "content": [{"type": "text", "text": summary}]}),
                );
            }
            continue;
        }
        // omp records when a tool actually started, after the reply that
        // asked for it; the tool's span starts then.
        if kind == "custom" && text(entry, "customType") == "tool_execution_start" {
            let data = &entry["data"];
            if let Some(at) = open_tools.get(text(data, "toolCallId"))
                && !text(data, "startedAt").is_empty()
            {
                spans[*at]["started_at"] = data["startedAt"].clone();
            }
            continue;
        }
        if !is_message(entry) {
            continue;
        }
        let message = &entry["message"];
        match text(message, "role") {
            // A developer message is the harness speaking to the model
            // between turns, as omp does to a sub-agent; it is context, not
            // a prompt.
            role @ ("user" | "developer") => {
                if nullish(pending_start) {
                    pending_start = stamp;
                }
                let content = images(content(message.get("content")), &input.blobs);
                pending.push(json!({"role": role, "content": content}));
            }
            "toolResult" => {
                let yes = Some(&Value::Bool(true));
                let failed = message.get("isError") == yes || message.get("is_error") == yes;
                // omp's task tool reports the sub-agents it started, by id.
                for list in ["progress", "results"] {
                    for started in message["details"][list].as_array().into_iter().flatten() {
                        let id = text(started, "id");
                        if !id.is_empty() {
                            agent_calls
                                .insert(id.to_owned(), text(message, "toolCallId").to_owned());
                        }
                    }
                }
                if let Some(call) = message["toolCallId"].as_str()
                    && let Some(at) = open_tools.remove(call)
                {
                    let mut output = Map::new();
                    let result = tool_result(message.get("content")).map(|result| match result {
                        Value::Array(_) => {
                            Value::Array(images(content(Some(&result)), &input.blobs))
                        }
                        plain => plain,
                    });
                    set(&mut output, "result", result.as_ref());
                    let tool = &mut spans[at];
                    if let Some(stamp) = stamp {
                        tool["ended_at"] = stamp.clone();
                    }
                    if failed {
                        output.insert("is_error".into(), Value::Bool(true));
                        tool["status"] = json!("error");
                    }
                    tool["output"] = Value::Object(output);
                }
                // The model sees the result on its next call, so it stays in
                // what is pending.
                if nullish(pending_start) {
                    pending_start = stamp;
                }
                let mut result = Map::new();
                result.insert("type".into(), json!("tool_result"));
                put(&mut result, "tool_call_id", message, "toolCallId");
                if failed {
                    result.insert("is_error".into(), Value::Bool(true));
                }
                let content = images(content(message.get("content")), &input.blobs);
                result.insert("content".into(), Value::Array(content));
                pending.push(json!({"role": "tool", "content": [result]}));
            }
            "assistant" => {
                sequence += 1;
                let id = format!("s{sequence}");
                let failed = text(message, "stopReason") == "error";
                let reply = images(content(message.get("content")), &input.blobs);
                let mut span = Map::new();
                span.insert("id".into(), json!(id));
                span.insert("parent_id".into(), json!("root"));
                span.insert("type".into(), json!("llm_call"));
                span.insert("name".into(), json!("turn"));
                span.insert("status".into(), json!(if failed { "error" } else { "ok" }));
                span.insert(
                    "model".into(),
                    json!({
                        "id": model_text(message.get("model")),
                        "provider": model_text(message.get("provider")),
                    }),
                );
                span.insert(
                    "input".into(),
                    json!({"messages": std::mem::take(&mut pending)}),
                );
                span.insert(
                    "output".into(),
                    json!({"messages": [{"role": "assistant", "content": reply}]}),
                );
                set(&mut span, "started_at", previous);
                set(&mut span, "ended_at", stamp);
                if let Some(usage) = usage(&message["usage"]) {
                    span.insert("usage".into(), usage);
                }
                let failure = text(message, "errorMessage");
                if !failure.is_empty() {
                    span.insert("error".into(), json!({"message": failure}));
                }
                spans.push(Value::Object(span));
                pending_start = None;
                for block in message["content"].as_array().into_iter().flatten() {
                    if text(block, "type") != "toolCall" {
                        continue;
                    }
                    let Some(call) = block["id"].as_str() else {
                        continue;
                    };
                    sequence += 1;
                    let mut given = Map::new();
                    given.insert("tool_call_id".into(), json!(call));
                    put(&mut given, "name", block, "name");
                    put(&mut given, "arguments", block, "arguments");
                    let mut tool = Map::new();
                    tool.insert("id".into(), json!(format!("s{sequence}")));
                    tool.insert("parent_id".into(), json!(id));
                    tool.insert("type".into(), json!("tool_call"));
                    tool.insert("status".into(), json!("ok"));
                    put(&mut tool, "name", block, "name");
                    set(&mut tool, "started_at", stamp);
                    tool.insert("input".into(), Value::Object(given));
                    spans.push(Value::Object(tool));
                    open_tools.insert(call.to_owned(), spans.len() - 1);
                }
            }
            _ => {}
        }
        previous = stamp;
    }

    // What was recorded stays as it was: a transcript that ends on a user or
    // a tool turn keeps those messages as a custom span, and no reply is
    // made up for an `llm_call` to hold.
    if !pending.is_empty() {
        sequence += 1;
        let mut trailing = Map::new();
        trailing.insert("id".into(), json!(format!("s{sequence}")));
        trailing.insert("parent_id".into(), json!("root"));
        trailing.insert("type".into(), json!("custom"));
        trailing.insert(
            "name".into(),
            json!("trailing message(s) — transcript ends before a reply"),
        );
        trailing.insert("input".into(), json!({"messages": pending}));
        let start = if nullish(pending_start) {
            previous
        } else {
            pending_start
        };
        set(&mut trailing, "started_at", start);
        set(&mut trailing, "ended_at", previous);
        spans.push(Value::Object(trailing));
    }

    let mut metadata = Map::new();
    if truthy(session.get("id")) {
        metadata.insert("session_id".into(), session["id"].clone());
    }
    if truthy(session.get("cwd")) {
        metadata.insert("cwd".into(), session["cwd"].clone());
    }
    // Messages on branches the person went back from stay out of the import.
    let skipped = messages - entries.len();
    if skipped != 0 {
        metadata.insert("branch_entries_skipped".into(), json!(skipped));
    }

    let mut run = Map::new();
    run.insert("schema".into(), json!("session/v0"));
    run.insert("name".into(), json!(name));
    run.insert(
        "source".into(),
        json!({
            "kind": "import",
            "harness": harness,
            "label": format!("{harness}-import@0.1.0"),
            "fidelity": "reconstructed",
        }),
    );
    run.insert("metadata".into(), Value::Object(metadata));
    run.insert("spans".into(), Value::Array(spans));
    set(&mut run, "created_at", started);
    let mut run = Value::Object(run);
    adopt_agents(&mut run, &input.agents, &agent_calls, |agent: &Agent| {
        let mut input = agent.input.clone();
        input.sub_agent = true;
        to_run(&input, harness)
    });
    Some(run)
}

/* ---------------------------------------------- where their sessions are */

/// What the environment says of where pi and omp keep their sessions. A
/// test gives its own, and so reads nothing of the machine it runs on.
struct Environment {
    home: PathBuf,
    /// The directory for temporary files.
    temp: PathBuf,
    /// The variables that are set, by name.
    variables: HashMap<&'static str, String>,
}

impl Environment {
    fn of_process() -> Environment {
        let variables = [
            "PI_CODING_AGENT_DIR",
            "PI_CODING_AGENT_SESSION_DIR",
            "PI_CONFIG_DIR",
            "OMP_PROFILE",
            "PI_PROFILE",
            "XDG_DATA_HOME",
        ]
        .into_iter()
        .filter_map(|name| Some((name, env::var_os(name)?.to_string_lossy().into_owned())))
        .collect();
        // Where Go looks for it, which is not where Rust does on a Mac:
        // `TMPDIR`, else `/tmp`.
        let temp = if cfg!(windows) {
            env::temp_dir()
        } else {
            env::var_os("TMPDIR")
                .filter(|dir| !dir.is_empty())
                .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
        };
        Environment {
            home: user_home(),
            temp,
            variables,
        }
    }

    /// A variable's value, `""` when it is not set.
    fn get(&self, name: &str) -> &str {
        self.variables.get(name).map_or("", String::as_str)
    }
}

/// A path with its `.` and `..` worked out as text, following no link: Go's
/// `filepath.Clean`, which every path the Go importer joins goes through.
fn clean(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match cleaned.components().next_back() {
                Some(Component::Normal(_)) => {
                    cleaned.pop();
                }
                // Nothing is above the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => cleaned.push(".."),
            },
            other => cleaned.push(other),
        }
    }
    if cleaned.as_os_str().is_empty() {
        cleaned.push(".");
    }
    cleaned
}

/// `base` with each of `names` under it. A name is never taken for a path of
/// its own, as `Path::join` takes one that starts at the root.
fn join(base: &Path, names: &[&str]) -> PathBuf {
    let mut joined = base.as_os_str().to_owned();
    for name in names.iter().filter(|name| !name.is_empty()) {
        if !joined.is_empty() {
            joined.push(MAIN_SEPARATOR_STR);
        }
        joined.push(name);
    }
    clean(Path::new(&joined))
}

/// A path from a harness's own configuration, with a leading `~` as the home
/// directory.
fn expand_home(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        join(home, &[rest])
    } else {
        PathBuf::from(path)
    }
}

/// Where pi keeps one directory of transcripts for each project.
/// `PI_CODING_AGENT_DIR` moves its whole agent directory.
fn pi_sessions(env: &Environment) -> PathBuf {
    match env.get("PI_CODING_AGENT_DIR") {
        "" => join(&env.home, &[".pi", "agent", "sessions"]),
        dir => join(&expand_home(dir, &env.home), &["sessions"]),
    }
}

/// A project's directory under `sessions`, named the way pi names it.
fn pi_project_dir(sessions: &Path, cwd: &Path) -> PathBuf {
    let cwd = cwd.to_string_lossy();
    let name = cwd
        .trim_start_matches(['/', '\\'])
        .replace(['/', '\\', ':'], "-");
    join(sessions, &[&format!("--{name}--")])
}

/// The single directory pi keeps sessions in when it is given one:
/// `PI_CODING_AGENT_SESSION_DIR`, else `sessionDir` in the `settings.json`
/// of the project or of the agent directory. `None` means the default
/// layout.
fn pi_session_dir(env: &Environment, agent_dir: &Path, cwd: &Path) -> Option<PathBuf> {
    let mut dir = env.get("PI_CODING_AGENT_SESSION_DIR").to_owned();
    for settings in [
        join(cwd, &[".pi", "settings.json"]),
        join(agent_dir, &["settings.json"]),
    ] {
        if !dir.is_empty() {
            break;
        }
        if let Ok(raw) = fs::read(&settings)
            && let Ok(parsed) = slink_format::parse(&String::from_utf8_lossy(&raw))
        {
            dir = text(&parsed, "sessionDir").to_owned();
        }
    }
    if dir.is_empty() {
        return None;
    }
    let dir = expand_home(&dir, &env.home);
    Some(if dir.is_absolute() {
        dir
    } else {
        // pi resolves it from the working directory.
        join(cwd, &[&dir.to_string_lossy()])
    })
}

/// pi's transcripts for one working directory: those in the project's own
/// directory under `sessions`, and those a configured session directory
/// holds for it. That directory is shared by every project using it, so each
/// transcript's header says which one it belongs to.
fn pi_transcripts(env: &Environment, sessions: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut files = transcripts_in(&pi_project_dir(sessions, cwd));
    let agent_dir = sessions.parent().unwrap_or(sessions);
    if let Some(shared) = pi_session_dir(env, agent_dir, cwd) {
        for file in transcripts_in(&shared) {
            // The directory is read as it was written, and its files are
            // named by the path cleaned, as Go names them.
            let file = clean(&file);
            if text(&header(&file), "cwd") == cwd.to_string_lossy() {
                files.push(file);
            }
        }
    }
    files
}

/// Whether omp accepts `name` for a profile.
fn profile_name(name: &str) -> bool {
    let plain = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    name.len() <= 64
        && name.bytes().next().is_some_and(plain)
        && name
            .bytes()
            .all(|byte| plain(byte) || matches!(byte, b'.' | b'_' | b'-'))
}

/// Where omp keeps one directory of transcripts for each project, resolved
/// the way omp resolves it: `PI_CONFIG_DIR` names its directory under home
/// (`.omp`), `OMP_PROFILE` (or `PI_PROFILE` when that is unset) selects a
/// profile inside it, and `PI_CODING_AGENT_DIR` moves the default profile's
/// agent directory. Data that omp has moved under `$XDG_DATA_HOME/omp` is
/// read there.
fn omp_sessions(env: &Environment) -> PathBuf {
    let root = match env.get("PI_CONFIG_DIR") {
        "" => ".omp",
        root => root,
    };
    let mut root = join(&env.home, &[root]);
    let profile = match env.variables.get("OMP_PROFILE") {
        Some(profile) => profile.trim(),
        None => env.get("PI_PROFILE").trim(),
    };
    let named = profile != "default" && profile_name(profile) && !profile.ends_with('.');
    let mut data = match env.get("XDG_DATA_HOME") {
        "" => None,
        data => Some(join(Path::new(data), &["omp"])),
    };
    if named {
        root = join(&root, &["profiles", profile]);
        data = data.map(|data| join(&data, &["profiles", profile]));
    } else if !env.get("PI_CODING_AGENT_DIR").is_empty() {
        return join(Path::new(env.get("PI_CODING_AGENT_DIR")), &["sessions"]);
    }
    match data.filter(|data| data.is_dir()) {
        Some(data) => join(&data, &["sessions"]),
        None => join(&root, &["agent", "sessions"]),
    }
}

/// A path with its links followed; one that is not there, as its text says.
fn resolved(path: &Path) -> PathBuf {
    let Ok(real) = fs::canonicalize(path) else {
        return clean(path);
    };
    // Windows answers with a verbatim path, `\\?\C:\...`, and a project is
    // named by the plain one.
    match real.to_str().and_then(|real| real.strip_prefix(r"\\?\")) {
        Some(plain) if !plain.starts_with(r"UNC\") => PathBuf::from(plain),
        _ => real,
    }
}

/// omp's transcripts for one working directory. omp names a project by its
/// path relative to the home directory (`-code-app`) or to the temp
/// directory (`-tmp-build`), after resolving symlinks, and only otherwise by
/// its whole path, the way pi does. A project last used before that naming
/// still has the whole-path name, until omp runs there again.
fn omp_transcripts(env: &Environment, sessions: &Path, cwd: &Path) -> Vec<PathBuf> {
    let canonical = resolved(cwd);
    let relative = |prefix: &str, rest: &Path| {
        let rest = rest.to_string_lossy();
        if rest.is_empty() {
            return join(sessions, &[prefix]);
        }
        let prefix = prefix.strip_suffix('-').unwrap_or(prefix);
        let rest = rest.replace(['/', '\\', ':'], "-");
        join(sessions, &[&format!("{prefix}-{rest}")])
    };
    let whole = pi_project_dir(sessions, &canonical);
    let named = if let Ok(rest) = canonical.strip_prefix(resolved(&env.home)) {
        relative("-", rest)
    } else if let Ok(rest) = canonical.strip_prefix(resolved(&env.temp)) {
        relative("-tmp", rest)
    } else {
        whole.clone()
    };
    let mut dirs = vec![named];
    for older in [whole, pi_project_dir(sessions, &clean(cwd))] {
        if !dirs.contains(&older) {
            dirs.push(older);
        }
    }
    dirs.iter().flat_map(|dir| transcripts_in(dir)).collect()
}

/// The longest line read from the head of a transcript: where Go's scanner
/// gives a line up, and with it the rest of the file.
const LINE_LIMIT: u64 = 16 * 1024 * 1024;

/// The session header among the first lines of a pi or omp transcript. omp
/// writes its title line ahead of it. `Null` when there is none.
fn header(file: &Path) -> Value {
    let Ok(opened) = fs::File::open(file) else {
        return Value::Null;
    };
    let mut reader = BufReader::new(opened);
    for _ in 0..5 {
        let mut line = Vec::new();
        let read = (&mut reader).take(LINE_LIMIT).read_until(b'\n', &mut line);
        let cut = line.len() as u64 == LINE_LIMIT && !line.ends_with(b"\n");
        if !matches!(read, Ok(1..)) || cut {
            break;
        }
        if let Ok(entry) = slink_format::parse(&String::from_utf8_lossy(&line))
            && text(&entry, "type") == "session"
        {
            return entry;
        }
    }
    Value::Null
}

/// The title omp gave a session. omp keeps the current one in the
/// transcript's first line, which it rewrites in place; the header and the
/// latest title change are where an older transcript has it.
fn omp_title(file: &Path) -> String {
    let current = first_json_line(file);
    if !text(&current, "title").trim().is_empty() {
        return text(&current, "title").trim().to_owned();
    }
    for line in tail_lines(file).iter().rev() {
        if !line.contains("\"title_change\"") {
            continue;
        }
        if let Ok(entry) = slink_format::parse(line)
            && text(&entry, "type") == "title_change"
            && !text(&entry, "title").trim().is_empty()
        {
            return text(&entry, "title").trim().to_owned();
        }
    }
    text(&header(file), "title").trim().to_owned()
}

/// The name the person gave a pi session. The latest `session_info` entry
/// decides, and an empty name in it clears the name.
fn session_name(file: &Path) -> String {
    for line in tail_lines(file).iter().rev() {
        if !line.contains("\"session_info\"") {
            continue;
        }
        if let Ok(entry) = slink_format::parse(line)
            && text(&entry, "type") == "session_info"
        {
            return text(&entry, "name").trim().to_owned();
        }
    }
    String::new()
}

/// The transcripts omp keeps for a session's sub-agents: in a directory
/// named after the session's own file.
fn agent_transcripts(transcript: &Path) -> Vec<PathBuf> {
    transcript
        .to_str()
        .and_then(|path| path.strip_suffix(".jsonl"))
        .map(|dir| transcripts_in(Path::new(dir)))
        .unwrap_or_default()
}

/// The transcripts of a session's sub-agents, and of theirs, as far down as
/// `agents` reads them. A preview made from the session depends on every one
/// of them, and on no file the import never opens. Go's listing goes on
/// down as far as there are directories.
fn agent_files(transcript: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if depth > 8 {
        return files;
    }
    for file in agent_transcripts(transcript) {
        let theirs = agent_files(&file, depth + 1);
        files.push(file);
        files.extend(theirs);
    }
    files
}

/// An omp session's sub-agents. omp writes each one's transcript, named
/// after the task, into a directory named after the session's own file; a
/// sub-agent's sub-agents are beside it the same way.
fn agents(transcript: &Path, blobs: &Path, depth: usize) -> Vec<Agent> {
    let mut found = Vec::new();
    for file in agent_transcripts(transcript) {
        let Ok(lines) = read_lines(&file) else {
            continue;
        };
        if depth > 8 {
            continue;
        }
        let id = stem(&file);
        found.push(Agent {
            call: String::new(),
            name: id.clone(),
            input: Input {
                lines,
                fallback: id.clone(),
                blobs: blobs.to_owned(),
                agents: agents(&file, blobs, depth + 1),
                ..Input::default()
            },
            id,
        });
    }
    found
}

/// The harness as a name that outlives the caller's.
fn named(harness: &str) -> &'static str {
    if harness == "omp" { "omp" } else { "pi" }
}

fn sessions(env: &Environment, harness: &str) -> PathBuf {
    if harness == "omp" {
        omp_sessions(env)
    } else {
        pi_sessions(env)
    }
}

fn transcripts(env: &Environment, store: &Path, cwd: &Path, harness: &str) -> Vec<PathBuf> {
    if harness == "omp" {
        omp_transcripts(env, store, cwd)
    } else {
        pi_transcripts(env, store, cwd)
    }
}

/// Where the harness keeps its sessions.
pub(crate) fn store(harness: &str) -> PathBuf {
    sessions(&Environment::of_process(), harness)
}

pub(crate) fn latest(cwd: &Path, harness: &str) -> Option<Found> {
    let env = Environment::of_process();
    let store = sessions(&env, harness);
    newest_of(transcripts(&env, &store, cwd, harness), named(harness))
}

pub(crate) fn anywhere(harness: &str) -> Option<Elsewhere> {
    newest_under(&store(harness), named(harness))
}

/// The newest transcript across every project directory under `store`.
fn newest_under(store: &Path, harness: &'static str) -> Option<Elsewhere> {
    let mut best: Option<(PathBuf, i64)> = None;
    for project in sorted_entries(store) {
        // A link to a directory is not one of the store's own.
        if fs::symlink_metadata(&project).is_ok_and(|entry| entry.is_dir())
            && let Some(found) = newest_file(&project)
            && best.as_ref().is_none_or(|best| found.1 > best.1)
        {
            best = Some(found);
        }
    }
    let (file, recency) = best?;
    let head = peek(&file, 40);
    let mut title = if head.name.is_empty() {
        head.prompt
    } else {
        head.name
    };
    if harness == "omp" {
        let recorded = omp_title(&file);
        if !recorded.is_empty() {
            title = recorded;
        }
    }
    Some(Elsewhere {
        harness,
        dir: head.cwd,
        title,
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
    harness: &'static str,
) -> Result<Vec<Candidate>, String> {
    Ok(candidates_in(
        &Environment::of_process(),
        store,
        dirs,
        id,
        harness,
    ))
}

fn candidates_in(
    env: &Environment,
    store: &Path,
    dirs: &[PathBuf],
    id: &str,
    harness: &'static str,
) -> Vec<Candidate> {
    let mut found = Vec::new();
    for dir in dirs {
        for file in transcripts(env, store, dir, harness) {
            // A session is known by the id in its header, not by its file.
            let session = match header(&file).get("id").and_then(Value::as_str) {
                Some(id) => id.to_owned(),
                None => stem(&file),
            };
            if id.is_empty() || session == id {
                found.extend(Candidate::file(harness, &file, dir, session));
            }
        }
    }
    found
}

/// Fills in a candidate's title, first prompt, start and sub-agent files.
pub(crate) fn describe(candidate: &mut Candidate) {
    let Some(file) = candidate.file.clone() else {
        return;
    };
    let omp = candidate.found.harness == "omp";
    candidate.describe_file(if omp {
        agent_files(&file, 0)
    } else {
        Vec::new()
    });
    // Its name is the one its harness recorded, and no other.
    candidate.name = if omp {
        omp_title(&file)
    } else {
        session_name(&file)
    };
    candidate.title = if candidate.name.is_empty() {
        candidate.prompt.clone()
    } else {
        candidate.name.clone()
    };
}

/// A transcript's lines with its image store and its sub-agents' sessions.
pub(crate) fn input(file: &Path, lines: Vec<String>, harness: &str) -> Input {
    let blobs = blobs_beside(file);
    Input {
        lines,
        fallback: stem(file),
        agents: if harness == "omp" {
            agents(file, &blobs, 0)
        } else {
            Vec::new()
        },
        blobs,
        ..Input::default()
    }
}

/// Whether a transcript's first entries are this harness's. omp is asked
/// first: its transcript has a title line of its own ahead of pi's header.
pub(crate) fn recognizes(head: &[Value], harness: &str) -> bool {
    if harness == "omp" {
        head.iter()
            .any(|entry| entry.get("pad").is_some() && entry["type"] == "title")
    } else {
        head.iter()
            .any(|entry| entry["type"] == "session" || entry["type"] == "message")
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(3600);

    /// An empty directory of the test's own, with its links followed: omp
    /// names a project by where it really is.
    fn scratch(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("slink-import-pi-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        resolved(&dir)
    }

    fn write(file: &Path, body: &str, at: SystemTime) {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, body).unwrap();
        let written = fs::File::options().write(true).open(file).unwrap();
        written.set_modified(at).unwrap();
    }

    /// A machine with this home and these variables, and nothing else set.
    fn machine(home: &Path, temp: &Path, variables: &[(&'static str, &str)]) -> Environment {
        Environment {
            home: home.to_owned(),
            temp: temp.to_owned(),
            variables: variables
                .iter()
                .map(|(name, value)| (*name, (*value).to_owned()))
                .collect(),
        }
    }

    /// What a listing shows for these directories: newest first, described.
    fn listed(
        env: &Environment,
        store: &Path,
        dirs: &[&Path],
        harness: &'static str,
    ) -> Vec<Candidate> {
        let dirs: Vec<PathBuf> = dirs.iter().map(|dir| dir.to_path_buf()).collect();
        let mut found = candidates_in(env, store, &dirs, "", harness);
        found.sort_by_key(|candidate| std::cmp::Reverse(candidate.found.recency));
        for candidate in &mut found {
            describe(candidate);
        }
        found
    }

    fn ids(candidates: &[Candidate]) -> String {
        let ids: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        ids.join(",")
    }

    #[test]
    fn a_model_that_is_not_text_is_named_as_the_go_importer_names_it() {
        for (model, named) in [
            (json!("claude"), "claude"),
            (json!(""), ""),
            (json!(5), "5"),
            (json!(5.5), "5.5"),
            (json!(1e21), "1e+21"),
            (json!(1e20), "100000000000000000000"),
            (json!(1e-7), "1e-7"),
            // Past what a float64 holds exactly.
            (json!(9_007_199_254_740_993_u64), "9007199254740992"),
            (json!(true), "true"),
            (Value::Null, "unknown"),
            (json!({}), "map[]"),
            (
                json!({"a": 1, "b": [1, 2.5, "x", null, true, {"k": 1_000_000}]}),
                "map[a:1 b:[1 2.5 x <nil> true map[k:1e+06]]]",
            ),
            (
                json!({"m": -1_234_567, "n": -1e-7, "o": 123_456.7}),
                "map[m:-1.234567e+06 n:-1e-07 o:123456.7]",
            ),
            (json!([[], {}, ""]), "[[] map[] ]"),
        ] {
            assert_eq!(model_text(Some(&model)), named, "{model}");
        }
        assert_eq!(model_text(None), "unknown");
        assert_eq!(js_string(&Value::Null), "null");
    }

    #[test]
    fn a_blob_is_written_as_padded_base64() {
        for (raw, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }

    #[test]
    fn a_path_is_joined_and_cleaned_as_go_does_it() {
        assert_eq!(
            join(Path::new("/home/u"), &["/abs/x"]),
            Path::new("/home/u/abs/x")
        );
        assert_eq!(
            join(Path::new("/a/b/"), &["", "sessions"]),
            Path::new("/a/b/sessions")
        );
        assert_eq!(join(Path::new(""), &["a"]), Path::new("a"));
        assert_eq!(clean(Path::new("/a/./b/../c/")), Path::new("/a/c"));
        assert_eq!(clean(Path::new("/../a")), Path::new("/a"));
        assert_eq!(clean(Path::new("a/../../b")), Path::new("../b"));
        assert_eq!(clean(Path::new("")), Path::new("."));
    }

    // Each harness can be told to keep its store somewhere else, and each is
    // followed there by the rule that harness itself applies.
    #[test]
    fn a_store_is_followed_to_where_its_harness_was_told_to_keep_it() {
        let home = scratch("stores");
        let none = home.join("none");
        let at = |variables: &[(&'static str, &str)]| machine(&home, &none, variables);

        assert_eq!(
            pi_sessions(&at(&[])),
            home.join(".pi").join("agent").join("sessions")
        );
        assert_eq!(
            pi_sessions(&at(&[("PI_CODING_AGENT_DIR", "~/agents/pi")])),
            home.join("agents").join("pi").join("sessions")
        );
        assert_eq!(
            pi_project_dir(Path::new("/s"), Path::new("/work/my project")),
            Path::new("/s").join("--work-my project--")
        );

        let omp = home.join(".omp");
        let sessions = omp.join("agent").join("sessions");
        assert_eq!(omp_sessions(&at(&[])), sessions);
        // A profile: PI_PROFILE counts only while OMP_PROFILE is not set at
        // all.
        let profile = |name: &str| {
            omp.join("profiles")
                .join(name)
                .join("agent")
                .join("sessions")
        };
        assert_eq!(
            omp_sessions(&at(&[("PI_PROFILE", "work")])),
            profile("work")
        );
        assert_eq!(
            omp_sessions(&at(&[("OMP_PROFILE", ""), ("PI_PROFILE", "work")])),
            sessions,
            "an empty OMP_PROFILE selects the default profile over PI_PROFILE"
        );
        assert_eq!(
            omp_sessions(&at(&[("OMP_PROFILE", "review"), ("PI_PROFILE", "work")])),
            profile("review")
        );
        for refused in ["Review", "review.", ".review", "a/b", &"x".repeat(65)] {
            assert_eq!(omp_sessions(&at(&[("OMP_PROFILE", refused)])), sessions);
        }
        // The default profile's agent directory can be moved, and its data
        // can live under XDG_DATA_HOME once omp has moved it there.
        let moved = home.join("agents").join("omp");
        let moved_to = moved.to_string_lossy();
        assert_eq!(
            omp_sessions(&at(&[
                ("OMP_PROFILE", "default"),
                ("PI_CODING_AGENT_DIR", &moved_to)
            ])),
            moved.join("sessions")
        );
        assert_eq!(
            omp_sessions(&at(&[
                ("OMP_PROFILE", "review"),
                ("PI_CODING_AGENT_DIR", &moved_to)
            ])),
            profile("review"),
            "only the default profile's agent directory moves"
        );
        let data = home.join("xdg");
        let data_home = data.to_string_lossy();
        assert_eq!(
            omp_sessions(&at(&[("XDG_DATA_HOME", &data_home)])),
            sessions,
            "nothing has been moved there yet"
        );
        fs::create_dir_all(data.join("omp")).unwrap();
        assert_eq!(
            omp_sessions(&at(&[("XDG_DATA_HOME", &data_home)])),
            data.join("omp").join("sessions")
        );
        // omp's directory under home has another name when it is given one.
        assert_eq!(
            omp_sessions(&at(&[("PI_CONFIG_DIR", ".config/omp")])),
            home.join(".config")
                .join("omp")
                .join("agent")
                .join("sessions")
        );
        fs::remove_dir_all(&home).unwrap();
    }

    // Given a session directory, pi keeps every project's sessions in it, so
    // the header of each transcript says which project it belongs to.
    #[test]
    fn pi_sessions_in_a_configured_session_directory_are_told_apart_by_their_headers() {
        let home = scratch("session-dir");
        let project = home.join("code").join("proj");
        let other = home.join("code").join("other");
        let session = |id: &str, cwd: &Path| {
            format!("{}\n", json!({"type": "session", "id": id, "cwd": cwd}))
        };
        let shared = home.join("pi-sessions");
        let now = SystemTime::now();
        write(&shared.join("mine.jsonl"), &session("mine", &project), now);
        write(
            &shared.join("theirs.jsonl"),
            &session("theirs", &other),
            now,
        );
        let plain = machine(&home, &home.join("none"), &[]);
        let store = pi_sessions(&plain);
        write(
            &pi_project_dir(&store, &project).join("default.jsonl"),
            &session("default", &project),
            now - HOUR,
        );

        let code = home.join("code");
        let list = |env: &Environment| ids(&listed(env, &store, &[&project, &code], "pi"));
        assert_eq!(list(&plain), "default", "without a session directory");
        let told = machine(
            &home,
            &home.join("none"),
            &[("PI_CODING_AGENT_SESSION_DIR", "~/pi-sessions")],
        );
        assert_eq!(list(&told), "mine,default");
        let latest = newest_of(transcripts(&told, &store, &project, "pi"), "pi").unwrap();
        assert_eq!(latest.harness, "pi");
        assert_eq!(
            latest.recency,
            crate::discover::modified(&shared.join("mine.jsonl")).unwrap()
        );

        // The same through settings.json: the agent directory's, then the
        // project's, whose relative path starts at the project.
        write(
            &home.join(".pi").join("agent").join("settings.json"),
            &json!({"sessionDir": shared}).to_string(),
            now,
        );
        assert_eq!(list(&plain), "mine,default");
        write(
            &project.join(".pi").join("settings.json"),
            r#"{"sessionDir":".sessions"}"#,
            now,
        );
        write(
            &project.join(".sessions").join("local.jsonl"),
            &session("local", &project),
            now + MINUTE,
        );
        assert_eq!(list(&plain), "local,default");
        fs::remove_dir_all(&home).unwrap();
    }

    // omp names a project's session directory by where the project is:
    // relative to home, relative to the temp directory, or by its whole
    // path.
    #[test]
    fn omp_sessions_are_found_where_omp_keeps_them() {
        let root = scratch("omp");
        let (home, temp) = (root.join("home"), root.join("tmp"));
        let env = machine(&home, &temp, &[]);
        let sessions = omp_sessions(&env);
        assert_eq!(sessions, home.join(".omp").join("agent").join("sessions"));
        let transcript = |id: &str, cwd: &Path, title: &str| {
            let lines = [
                json!({"type": "title", "v": 1, "title": title, "updatedAt": "2026-10-07T10:00:00.000Z", "pad": "    "}),
                json!({"type": "session", "version": 3, "id": id, "timestamp": "2026-10-07T10:00:00.000Z", "cwd": cwd}),
                json!({"type": "message", "id": "m1", "parentId": null, "timestamp": "2026-10-07T10:00:01.000Z", "message": {"role": "user", "content": [{"type": "text", "text": "Find the race"}]}}),
            ];
            lines.map(|line| format!("{line}\n")).concat()
        };
        let project = home.join("code").join("my.app");
        let scratch = temp.join("build").join("one");
        let elsewhere = Path::new("/work/elsewhere");
        for dir in [&project, &scratch] {
            fs::create_dir_all(dir).unwrap();
        }
        let now = SystemTime::now();
        let session = sessions.join("-code-my.app").join("a.jsonl");
        write(
            &session,
            &transcript("in-home", &project, "Upload test race"),
            now,
        );
        // A sub-agent's transcript sits in a directory beside its session's
        // file.
        let scout = sessions.join("-code-my.app").join("a").join("Scout.jsonl");
        write(&scout, &transcript("sub-agent", &project, ""), now + MINUTE);
        let deeper = sessions
            .join("-code-my.app")
            .join("a")
            .join("Scout")
            .join("Probe.jsonl");
        write(&deeper, &transcript("deeper", &project, ""), now + MINUTE);
        // Last used before omp named directories relative to home.
        write(
            &pi_project_dir(&sessions, &project).join("b.jsonl"),
            &transcript("older-naming", &project, ""),
            now - HOUR,
        );
        write(
            &sessions.join("-").join("c.jsonl"),
            &transcript("at-home", &home, ""),
            now,
        );
        write(
            &sessions.join("-tmp-build-one").join("d.jsonl"),
            &transcript("in-tmp", &scratch, ""),
            now,
        );
        write(
            &sessions.join("--work-elsewhere--").join("e.jsonl"),
            &transcript("elsewhere", elsewhere, ""),
            now,
        );

        // From inside the project: its own directory, then each parent short
        // of home.
        let (inside, code) = (project.join("src"), home.join("code"));
        let in_project = listed(&env, &sessions, &[&inside, &project, &code], "omp");
        assert_eq!(ids(&in_project), "in-home,older-naming");
        let first = &in_project[0];
        assert_eq!(first.found.harness, "omp");
        assert_eq!(first.title, "Upload test race");
        assert_eq!(first.prompt, "Find the race");
        assert_eq!(first.dir, project);
        assert_eq!(first.related, [scout.clone(), deeper]);
        let untitled = &in_project[1];
        assert_eq!(
            (untitled.name.as_str(), untitled.title.as_str()),
            ("", "Find the race"),
            "an untitled session falls back to its prompt"
        );
        for (cwd, id) in [
            (home.as_path(), "at-home"),
            (scratch.as_path(), "in-tmp"),
            (elsewhere, "elsewhere"),
        ] {
            assert_eq!(
                ids(&listed(&env, &sessions, &[cwd], "omp")),
                id,
                "{}",
                cwd.display()
            );
        }
        let latest = newest_of(transcripts(&env, &sessions, &project, "omp"), "omp").unwrap();
        assert_eq!(latest.harness, "omp");
        assert!(
            matches!(&latest.loader, crate::discover::Loader::File(file) if *file == session),
            "the newest session of the project, not its sub-agent's"
        );
        // Read with what omp keeps beside it: the sub-agent, and its own.
        let input = latest.load().unwrap();
        assert_eq!(input.agents.len(), 1);
        assert_eq!(input.agents[0].id, "Scout");
        assert_eq!(input.agents[0].input.agents[0].id, "Probe");
        fs::remove_dir_all(&root).unwrap();
    }

    // A session started in the home directory belongs to work done there,
    // and to no project beneath it.
    #[test]
    fn a_session_started_at_home_belongs_to_home_only() {
        let home = scratch("at-home");
        let env = machine(&home, &home.join("none"), &[]);
        let store = pi_sessions(&env);
        let header = json!({"type": "session", "id": "pi", "cwd": home});
        write(
            &pi_project_dir(&store, &home).join("pi.jsonl"),
            &format!("{header}\n"),
            SystemTime::now(),
        );
        assert_eq!(ids(&listed(&env, &store, &[&home], "pi")), "pi");
        // What a project beneath it is asked for: itself and its parents
        // short of home.
        let (project, code) = (home.join("code").join("proj"), home.join("code"));
        assert_eq!(ids(&listed(&env, &store, &[&project, &code], "pi")), "");
        assert!(newest_of(transcripts(&env, &store, &home, "pi"), "pi").is_some());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn the_newest_session_anywhere_is_found_with_its_directory_and_title() {
        let root = scratch("anywhere");
        let store = root.join("sessions");
        assert!(newest_under(&store, "pi").is_none(), "no store at all");
        fs::create_dir_all(&store).unwrap();
        assert!(newest_under(&store, "pi").is_none(), "an empty store");

        let session = |cwd: &str, title: &str, prompt: &str, more: &str| {
            let lines = [
                json!({"type": "title", "v": 1, "title": title, "pad": "  "}),
                json!({"type": "session", "id": "s", "cwd": cwd}),
                json!({"type": "message", "id": "m1", "parentId": null, "message": {"role": "user", "content": [{"type": "text", "text": prompt}]}}),
            ];
            lines.map(|line| format!("{line}\n")).concat() + more
        };
        let now = SystemTime::now();
        write(
            &store.join("--proj-a--").join("old.jsonl"),
            &session("/proj/a", "", "Asked first", ""),
            now - HOUR,
        );
        let newest = store.join("--proj-b--").join("new.jsonl");
        write(
            &newest,
            &session(
                "/proj/b",
                "",
                "Ship the empty state",
                "{\"type\":\"title_change\",\"id\":\"t\",\"parentId\":\"m1\",\"title\":\" Empty state \"}\n",
            ),
            now,
        );
        // Not in a project's directory, so not a session of any project.
        write(
            &store.join("loose.jsonl"),
            &session("/proj/c", "", "Loose", ""),
            now + HOUR,
        );

        let found = newest_under(&store, "pi").unwrap();
        assert_eq!((found.harness, found.dir.as_str()), ("pi", "/proj/b"));
        assert_eq!(found.file.as_deref(), Some(newest.as_path()));
        assert_eq!(found.title, "Ship the empty state");
        // omp gave it a title of its own.
        assert_eq!(newest_under(&store, "omp").unwrap().title, "Empty state");
        fs::remove_dir_all(&root).unwrap();
    }
}
