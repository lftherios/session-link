//! Codex. A rollout under `~/.codex/sessions` is one JSON line for each
//! thing that happened, `{timestamp, type, payload}`: a `session_meta`
//! header (cwd, model_provider, base_instructions), `turn_context` lines
//! (the model of each turn), `token_count` events (usage) and
//! `response_item` lines carrying Responses API items. Each turn collapses to
//! one `llm_call` span whose reply keeps reasoning and tool calls in the
//! order they were recorded. Function calls, custom tool calls such as
//! `apply_patch`, local shell calls, tool searches and web searches become
//! `tool_call` children. Any other item is kept as it was recorded, in a
//! custom span.
//!
//! The importer was first written in JavaScript, and a rollout is read here
//! the way that code read it: `js_string` and `same_key` are what `String(x)`
//! and a `Map`'s keys do there.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf, is_separator};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::discover::{
    Candidate, Elsewhere, Found, Loader, blobs_beside, first_json_line, peek, read_lines, stem,
    user_home,
};
use crate::images::image_part;
use crate::value::{data_part, field, js_string, or_null, text, truthy};
use crate::{Agent, Input, LABEL};

pub(crate) fn import(input: &Input) -> Result<Option<Value>, String> {
    Ok(read(input))
}

/// The session in `input`, with its sub-agents' sessions nested in it.
fn read(input: &Input) -> Option<Value> {
    let (mut run, calls) = to_run(&input.lines, &input.fallback)?;
    adopt_agents(&mut run, &input.agents, &calls, |agent: &Agent| {
        read(&agent.input)
    });
    Some(run)
}

/// A message's content as parts: text as text, an image as an image, and
/// anything else as it was recorded.
fn map_content(content: Option<&Value>) -> Vec<Value> {
    let items = match content {
        Some(Value::String(said)) if said.is_empty() => return Vec::new(),
        Some(Value::String(said)) => return vec![json!({"type": "text", "text": said})],
        Some(Value::Array(items)) => items,
        _ => return Vec::new(),
    };
    items
        .iter()
        .map(|item| {
            let Some(kind) = item.get("type").and_then(Value::as_str) else {
                return data_part(item);
            };
            match kind {
                "input_text" | "output_text" | "text" => {
                    let said = field(item, "text").cloned().unwrap_or_else(|| json!(""));
                    json!({"type": "text", "text": said})
                }
                // An image part has to say where its image is.
                "input_image" => match field(item, "image_url").or_else(|| item.get("url")) {
                    Some(Value::String(url)) => json!({"type": "image", "url": url}),
                    _ => data_part(item),
                },
                // The format is open: a part it does not know passes through.
                _ => item.clone(),
            }
        })
        .collect()
}

/// A readable summary, else the recorded content. Encrypted data says that a
/// reasoning event exists; it is not reasoning text anyone can read.
fn reasoning_text(payload: &Value) -> String {
    for name in ["summary", "content"] {
        let pieces: Vec<String> = payload[name]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|piece| match piece {
                Value::String(said) => Some(said.clone()),
                piece => field(piece, "text").map(js_string),
            })
            .collect();
        let joined = pieces.join("\n");
        if !joined.trim().is_empty() {
            return joined.trim().to_owned();
        }
    }
    String::new()
}

fn reasoning_part(payload: &Value) -> Value {
    let said = reasoning_text(payload);
    if !said.is_empty() {
        return json!({"type": "thinking", "text": said});
    }
    let reason = if truthy(payload.get("encrypted_content")) {
        "encrypted"
    } else {
        "not_recorded"
    };
    json!({"type": "thinking", "text": "", "unavailable": true, "reason": reason})
}

/// The token counts of a model call under the format's names, or `None`
/// when the event recorded none of them.
fn usage(recorded: &Value) -> Option<Value> {
    let mut counts = Map::new();
    for (theirs, ours) in [
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cached_input_tokens", "cache_read_tokens"),
        ("reasoning_output_tokens", "reasoning_tokens"),
    ] {
        if let Some(count) = field(recorded, theirs) {
            counts.insert(ours.to_owned(), count.clone());
        }
    }
    (!counts.is_empty()).then_some(Value::Object(counts))
}

/// The system prompt of a header: text, or an object holding it.
fn system_prompt(base: &Value) -> Option<&str> {
    base.as_str().or_else(|| base["text"].as_str())
}

/// The first text part of some content that is more than white space.
fn first_text(content: Option<&Value>) -> Option<String> {
    map_content(content)
        .iter()
        .filter(|part| text(part, "type") == "text")
        .filter_map(|part| part["text"].as_str())
        .map(str::trim)
        .find(|said| !said.is_empty())
        .map(str::to_owned)
}

/// A name cut to 80 UTF-16 code units, which is what JavaScript counts and
/// slices. A character cut in half reads as U+FFFD.
fn truncate(name: &str) -> String {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() <= 80 {
        return name.to_owned();
    }
    char::decode_utf16(units[..77].iter().copied())
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .chain(['…'])
        .collect()
}

/// Whether two turn ids are the same key of a JavaScript `Map`: text, a
/// number or a boolean, by value. Objects and arrays are distinct for each
/// parsed line, so they never match.
fn same_key(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

/// The id a call's result names: `call_id`, else the item's own id. A call
/// recorded with neither gets `fallback`, so calls stay distinct.
fn call_id(payload: &Value, fallback: String) -> String {
    field(payload, "call_id")
        .or_else(|| field(payload, "id"))
        .map_or(fallback, js_string)
}

/// One tool call, from the item that made it to the item that answers it.
struct Tool {
    id: String,
    name: Value,
    arguments: Value,
    started: Value,
    /// When its result came; until then the call ends with its turn.
    ended: Option<Value>,
    output: Value,
    /// Where its span is, once the turn is written, so a late result still
    /// lands.
    span: Option<usize>,
}

/// A rollout while it is read: the spans written so far, and what has
/// gathered since the last reply.
struct Rollout {
    provider: String,
    system: Option<String>,
    /// The model of a reply that names no turn, or a turn with no model.
    default_model: Option<Value>,
    spans: Vec<Value>,
    /// The messages the next model call was given.
    pending: Vec<Value>,
    /// Reasoning and tool calls since the last reply, in recorded order.
    activity: Vec<Value>,
    tools: Vec<Tool>,
    /// The calls of the turn being read, by where they are in `tools`.
    turn_tools: Vec<usize>,
    /// Call id -> the call with no result yet.
    awaiting: HashMap<String, usize>,
    calls: u64,
    last_usage: Value,
    turn_start: Value,
    sequence: u64,
    /// For each sub-agent the thread spawned, the call that spawned it, by
    /// the task path Codex gave the agent.
    agent_calls: HashMap<String, String>,
}

impl Rollout {
    fn next_id(&mut self) -> String {
        self.sequence += 1;
        format!("s{}", self.sequence)
    }

    /// Keeps an item this importer has no mapping for. It stays in the
    /// document as recorded instead of disappearing from the session.
    fn keep(&mut self, payload: &Value, stamp: &Value) {
        let id = self.next_id();
        self.spans.push(json!({
            "id": id,
            "parent_id": "root",
            "type": "custom",
            "name": format!("codex {}", payload["type"].as_str().unwrap_or("item")),
            "started_at": stamp,
            "ended_at": stamp,
            "raw": {"item": payload},
        }));
    }

    /// A message that is not the model's opens a turn when nothing has
    /// gathered for one yet.
    fn begin(&mut self, stamp: &Value) {
        if self.pending.is_empty() && self.activity.is_empty() {
            self.turn_start = stamp.clone();
        }
    }

    /// Writes the turn that `reply` closes: the model call, then each tool
    /// call it made.
    fn flush(&mut self, reply: Vec<Value>, model: Option<&Value>, ended: &Value) {
        let id = self.next_id();
        let mut content = std::mem::take(&mut self.activity);
        content.extend(reply);
        let model = model
            .or(self.default_model.as_ref())
            .map_or_else(|| "unknown".to_owned(), js_string);
        let mut input = Map::new();
        input.insert(
            "messages".to_owned(),
            Value::Array(std::mem::take(&mut self.pending)),
        );
        if let Some(system) = &self.system {
            input.insert("system".to_owned(), json!(system));
        }
        let mut span = json!({
            "id": id,
            "parent_id": "root",
            "type": "llm_call",
            "name": "turn",
            "started_at": self.turn_start,
            "ended_at": ended,
            "status": "ok",
            "model": {"id": model, "provider": self.provider},
            "input": input,
            "output": {"messages": [{"role": "assistant", "content": content}]},
        });
        if let Some(usage) = usage(&self.last_usage) {
            span["usage"] = usage;
        }
        self.spans.push(span);
        for at in std::mem::take(&mut self.turn_tools) {
            let tool_id = self.next_id();
            let tool = &mut self.tools[at];
            tool.span = Some(self.spans.len());
            self.spans.push(json!({
                "id": tool_id,
                "parent_id": id,
                "type": "tool_call",
                "name": tool.name,
                "started_at": tool.started,
                "ended_at": tool.ended.as_ref().unwrap_or(ended),
                "status": "ok",
                "input": {
                    "name": tool.name,
                    "arguments": tool.arguments,
                    "tool_call_id": tool.id,
                },
                "output": {"result": tool.output},
            }));
        }
        self.last_usage = Value::Null;
        self.turn_start = ended.clone();
    }

    /// Records one tool call: a part in the reply that made it, and a span
    /// its result closes.
    fn call(&mut self, payload: &Value, name: Value, arguments: Value, stamp: &Value) -> usize {
        self.calls += 1;
        let kind = payload["type"].as_str().unwrap_or("call");
        let id = call_id(payload, format!("{kind}_{}", self.calls));
        self.activity
            .push(json!({"type": "tool_call", "id": id, "name": name, "arguments": arguments}));
        let at = self.tools.len();
        self.turn_tools.push(at);
        self.awaiting.insert(id.clone(), at);
        self.tools.push(Tool {
            id,
            name,
            arguments,
            started: stamp.clone(),
            ended: None,
            output: Value::Null,
            span: None,
        });
        at
    }

    /// Closes the call an output item names. One that names no known call is
    /// kept as recorded.
    fn result(&mut self, payload: &Value, output: Value, stamp: &Value) {
        let named = field(payload, "call_id").map(js_string);
        let Some(at) = named.and_then(|id| self.awaiting.remove(&id)) else {
            self.keep(payload, stamp);
            return;
        };
        let tool = &mut self.tools[at];
        // spawn_agent answers with the task path of the agent it started.
        if tool.name == "spawn_agent"
            && let Some(answer) = output.as_str()
            && let Ok(spawned) = slink_format::parse(answer)
            && !text(&spawned, "task_name").is_empty()
        {
            self.agent_calls
                .insert(text(&spawned, "task_name").to_owned(), tool.id.clone());
        }
        if let Some(span) = tool.span {
            self.spans[span]["output"] = json!({"result": output});
            self.spans[span]["ended_at"] = stamp.clone();
        }
        tool.output = output;
        tool.ended = Some(stamp.clone());
    }
}

/// The document for a rollout, and for each sub-agent the thread spawned the
/// call that spawned it, by the task path Codex gave the agent.
fn to_run(lines: &[String], fallback: &str) -> Option<(Value, HashMap<String, String>)> {
    let mut inherited = 0_u64;
    let mut meta = Value::Null;
    // Each turn's model, in the order the turns were first named.
    let mut turn_models: Vec<(Value, Value)> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    for line in lines {
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        let kind = text(&entry, "type");
        // A thread that was forked, as a sub-agent's is, opens with its own
        // header and then replays its parent's, header included. It says
        // where its own history starts; what comes before was the parent's.
        if kind != "session_meta"
            && let Some(start) = meta["subagent_history_start_ordinal"].as_f64()
            && entry["ordinal"]
                .as_f64()
                .is_some_and(|ordinal| ordinal < start)
        {
            if kind == "response_item" {
                inherited += 1;
            }
            continue;
        }
        let payload = &entry["payload"];
        match kind {
            "session_meta" if truthy(Some(payload)) => {
                if meta.is_null() {
                    meta = payload.clone();
                }
            }
            "turn_context" if truthy(payload.get("turn_id")) => {
                let (turn, model) = (&payload["turn_id"], or_null(payload, "model"));
                match turn_models
                    .iter_mut()
                    .find(|(known, _)| same_key(known, turn))
                {
                    Some(known) => known.1 = model,
                    None => turn_models.push((turn.clone(), model)),
                }
            }
            "response_item" | "event_msg" => items.push(entry),
            _ => {}
        }
    }
    let responses = || {
        items
            .iter()
            .filter(|item| text(item, "type") == "response_item")
    };
    if meta.is_null() && responses().next().is_none() {
        return None;
    }

    let created = field(&meta, "timestamp")
        .or_else(|| items.first().and_then(|item| field(item, "timestamp")))
        .cloned()
        .unwrap_or_else(|| json!("1970-01-01T00:00:00.000Z"));

    // Named from the first real prompt: a user_message event carries what
    // was typed as it was typed, and injected user items do not.
    let typed = items
        .iter()
        .find(|item| {
            text(item, "type") == "event_msg" && text(&item["payload"], "type") == "user_message"
        })
        .map(|item| or_null(&item["payload"], "message"))
        .filter(|message| truthy(Some(message)));
    let opener = typed.or_else(|| {
        for item in responses() {
            let payload = &item["payload"];
            if text(payload, "type") != "message" || text(payload, "role") != "user" {
                continue;
            }
            match first_text(payload.get("content")) {
                // A block Codex wrote itself, such as <environment_context>.
                Some(said) if said.starts_with('<') => {}
                said => return said.map(Value::String),
            }
        }
        None
    });
    let name = match opener {
        Some(Value::String(said)) => Value::String(truncate(&said)),
        Some(other) => other,
        None => Value::String(fallback.to_owned()),
    };

    let mut rollout = Rollout {
        provider: field(&meta, "model_provider").map_or_else(|| "openai".to_owned(), js_string),
        system: system_prompt(&meta["base_instructions"])
            .filter(|system| !system.is_empty())
            .map(str::to_owned),
        // The last turn that named a model.
        default_model: turn_models
            .iter()
            .rev()
            .map(|(_, model)| model)
            .find(|model| truthy(Some(model)))
            .cloned(),
        spans: vec![json!({
            "id": "root",
            "parent_id": null,
            "type": "agent",
            "name": name,
            "started_at": created,
        })],
        pending: Vec::new(),
        activity: Vec::new(),
        tools: Vec::new(),
        turn_tools: Vec::new(),
        awaiting: HashMap::new(),
        calls: 0,
        last_usage: Value::Null,
        turn_start: created.clone(),
        sequence: 0,
        agent_calls: HashMap::new(),
    };
    let agent_path = text(&meta, "agent_path");
    let mut last = created.clone();

    for item in &items {
        let stamp = field(item, "timestamp")
            .cloned()
            .unwrap_or_else(|| last.clone());
        let payload = &item["payload"];
        if text(item, "type") == "event_msg" {
            if text(payload, "type") == "token_count" {
                let info = &payload["info"];
                if let Some(counts) =
                    field(info, "last_token_usage").or_else(|| field(info, "total_token_usage"))
                {
                    rollout.last_usage = counts.clone();
                }
            }
            // An event never moves the time the next item falls back on.
            continue;
        }
        match text(payload, "type") {
            "message" if text(payload, "role") == "assistant" => {
                let turn = &payload["internal_chat_message_metadata_passthrough"]["turn_id"];
                let model = turn_models
                    .iter()
                    .find(|(known, _)| same_key(known, turn))
                    .map(|(_, model)| model)
                    .filter(|model| !model.is_null());
                rollout.flush(map_content(payload.get("content")), model, &stamp);
            }
            "message" => {
                rollout.begin(&stamp);
                let role = field(payload, "role")
                    .cloned()
                    .unwrap_or_else(|| json!("user"));
                rollout
                    .pending
                    .push(json!({"role": role, "content": map_content(payload.get("content"))}));
            }
            kind @ ("function_call" | "custom_tool_call") => {
                // A function call carries JSON arguments as a string; a
                // custom tool such as apply_patch carries free-form text,
                // kept as written.
                let arguments = if kind == "custom_tool_call" {
                    or_null(payload, "input")
                } else if let Some(raw) = payload["arguments"].as_str() {
                    slink_format::parse(raw).unwrap_or_else(|_| json!(raw))
                } else {
                    or_null(payload, "arguments")
                };
                let name = payload
                    .get("name")
                    .filter(|name| truthy(Some(name)))
                    .cloned()
                    .unwrap_or_else(|| json!("tool"));
                rollout.call(payload, name, arguments, &stamp);
            }
            "local_shell_call" => {
                let action = or_null(payload, "action");
                rollout.call(payload, json!("local_shell"), action, &stamp);
            }
            "tool_search_call" => {
                let arguments = or_null(payload, "arguments");
                rollout.call(payload, json!("tool_search"), arguments, &stamp);
            }
            "web_search_call" => {
                // The search runs on the provider's side; no result is
                // recorded.
                let action = or_null(payload, "action");
                rollout.call(payload, json!("web_search"), action, &stamp);
            }
            // The provider draws the image and returns it with the item,
            // along with the prompt it drew from.
            "image_generation_call" => match image_part(text(payload, "result"), "") {
                Some(image) => {
                    let arguments = match text(payload, "revised_prompt") {
                        "" => Value::Null,
                        prompt => json!({"revised_prompt": prompt}),
                    };
                    let at = rollout.call(payload, json!("image_generation"), arguments, &stamp);
                    let tool = &mut rollout.tools[at];
                    rollout.awaiting.remove(&tool.id);
                    tool.output = json!([image]);
                    tool.ended = Some(stamp.clone());
                }
                None => rollout.keep(payload, &stamp),
            },
            "function_call_output" | "custom_tool_call_output" => {
                // A result is text, or a list of content items that can
                // hold images.
                let output = match payload.get("output") {
                    Some(Value::Array(_)) => Value::Array(map_content(payload.get("output"))),
                    other => other.cloned().unwrap_or(Value::Null),
                };
                rollout.result(payload, output, &stamp);
            }
            "tool_search_output" => {
                rollout.result(payload, or_null(payload, "tools"), &stamp);
            }
            "agent_message" => {
                // Agents in a team write to each other. What is addressed to
                // this thread's own agent is its task, as a prompt is;
                // anything else a thread receives is context. The payload of
                // a task is recorded as encrypted content, kept as it is.
                let content: Vec<Value> = payload["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|part| match text(part, "type") {
                        "input_text" => json!({"type": "text", "text": text(part, "text")}),
                        _ => data_part(part),
                    })
                    .collect();
                let own = !agent_path.is_empty() && text(payload, "recipient") == agent_path;
                let role = if own { "user" } else { "system" };
                rollout.begin(&stamp);
                rollout
                    .pending
                    .push(json!({"role": role, "content": content}));
            }
            "reasoning" => rollout.activity.push(reasoning_part(payload)),
            _ if payload.is_object() => rollout.keep(payload, &stamp),
            _ => {}
        }
        last = stamp;
    }

    // What the model did with no closing message still becomes a turn;
    // otherwise leftover user input survives as a custom span.
    if !rollout.activity.is_empty() {
        rollout.flush(Vec::new(), None, &last);
    } else if !rollout.pending.is_empty() {
        let id = rollout.next_id();
        rollout.spans.push(json!({
            "id": id,
            "parent_id": "root",
            "type": "custom",
            "name": "trailing message(s) — no assistant reply",
            "started_at": rollout.turn_start,
            "ended_at": last,
            "input": {"messages": rollout.pending},
        }));
    }
    rollout.spans[0]["ended_at"] = last;
    rollout.spans[0]["status"] = json!("ok");

    let mut metadata = Map::new();
    for (theirs, ours) in [
        ("session_id", "session_id"),
        ("cwd", "cwd"),
        ("cli_version", "harness_version"),
    ] {
        if truthy(meta.get(theirs)) {
            metadata.insert(ours.to_owned(), meta[theirs].clone());
        }
    }
    if inherited != 0 {
        metadata.insert("inherited_items_skipped".to_owned(), json!(inherited));
    }

    let mut run = Map::new();
    run.insert("schema".to_owned(), json!("session/v0"));
    run.insert("name".to_owned(), name);
    run.insert("created_at".to_owned(), created);
    run.insert(
        "source".to_owned(),
        json!({"kind": "import", "harness": "codex", "label": LABEL, "fidelity": "reconstructed"}),
    );
    run.insert("metadata".to_owned(), Value::Object(metadata));
    run.insert("spans".to_owned(), Value::Array(rollout.spans));
    Some((Value::Object(run), rollout.agent_calls))
}

/* ------------------------------------------------ where its sessions are */

/// Where Codex keeps its rollouts, in a directory for each day. `CODEX_HOME`
/// moves its whole home.
pub(crate) fn store() -> PathBuf {
    match env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        Some(home) => PathBuf::from(home).join("sessions"),
        None => user_home().join(".codex").join("sessions"),
    }
}

/// Every `.jsonl` file under `root`, directory by directory in the order of
/// their names. A link is not followed, so a directory that is one is not
/// searched, `root` included.
fn rollouts(root: &Path) -> Vec<PathBuf> {
    fn walk(path: PathBuf, is_dir: bool, found: &mut Vec<PathBuf>) {
        if !is_dir {
            if path.to_string_lossy().ends_with(".jsonl") {
                found.push(path);
            }
            return;
        }
        let mut entries: Vec<(OsString, bool)> = fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| {
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                (entry.file_name(), is_dir)
            })
            .collect();
        entries.sort();
        for (name, is_dir) in entries {
            walk(path.join(name), is_dir, found);
        }
    }
    let mut found = Vec::new();
    if let Ok(kind) = fs::symlink_metadata(root) {
        walk(root.to_owned(), kind.is_dir(), &mut found);
    }
    found
}

/// When a rollout was last written, in nanoseconds since 1970: the time the
/// walk that found it reads, which for a link is the link's own.
fn written(file: &Path) -> Option<i64> {
    let modified = fs::symlink_metadata(file).ok()?.modified().ok()?;
    i64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

/// Whether a rollout is for a thread the person did not start: a sub-agent,
/// a review or compaction helper, or internal work such as memory
/// consolidation. Codex writes each one beside the session that spawned it,
/// with the same working directory.
fn auxiliary(meta: &Value) -> bool {
    let source = &meta["source"];
    field(source, "subagent").is_some() || field(source, "internal").is_some()
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    latest_in(&store(), cwd)
}

/// The rollout under `store` last written for a session the person started
/// in `cwd`. Only the 500 newest are opened.
fn latest_in(store: &Path, cwd: &Path) -> Option<Found> {
    let mut rolls: Vec<(PathBuf, i64)> = rollouts(store)
        .into_iter()
        .filter_map(|file| {
            let recency = written(&file)?;
            Some((file, recency))
        })
        .collect();
    rolls.sort_by_key(|roll| std::cmp::Reverse(roll.1));
    rolls.truncate(500);
    rolls.into_iter().find_map(|(file, recency)| {
        let head = first_json_line(&file);
        let meta = &head["payload"];
        let own = text(&head, "type") == "session_meta"
            && cwd.to_str() == Some(text(meta, "cwd"))
            && !auxiliary(meta);
        own.then_some(Found {
            harness: "codex",
            recency,
            loader: Loader::File(file),
        })
    })
}

pub(crate) fn anywhere() -> Option<Elsewhere> {
    anywhere_in(&store())
}

/// The rollout under `store` that was written last, whichever thread and
/// project it is for.
fn anywhere_in(store: &Path) -> Option<Elsewhere> {
    let mut best: Option<(PathBuf, i64)> = None;
    for file in rollouts(store) {
        if let Some(recency) = written(&file)
            && best.as_ref().is_none_or(|best| recency > best.1)
        {
            best = Some((file, recency));
        }
    }
    let (file, recency) = best?;
    let head = peek(&file, 40);
    Some(Elsewhere {
        harness: "codex",
        dir: head.cwd,
        title: if head.name.is_empty() {
            head.prompt
        } else {
            head.name
        },
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
    let mut found: Vec<Candidate> = Vec::new();
    // The walk that finds the sessions reads every rollout's header, so it
    // also learns which rollouts are sub-agents' and whose. A listing reads
    // the store once.
    let mut threads = Threads::new();
    for file in rollouts(store) {
        let head = first_json_line(&file);
        let meta = &head["payload"];
        index(&mut threads, meta, &file);
        let dir = Path::new(text(meta, "cwd"));
        // Only an explicit id reaches a sub-agent's own rollout.
        if text(&head, "type") != "session_meta"
            || !dirs.iter().any(|own| own == dir)
            || (id.is_empty() && auxiliary(meta))
        {
            continue;
        }
        let session = meta["id"]
            .as_str()
            .or_else(|| meta["session_id"].as_str())
            .map_or_else(|| stem(&file), str::to_owned);
        if id.is_empty() || session == id {
            found.extend(Candidate::file("codex", &file, dir, session));
        }
    }
    for candidate in &mut found {
        agent_files(&threads, &candidate.id, 0, &mut candidate.related);
    }
    Ok(found)
}

/// Fills in a candidate's title, first prompt and start. The files of its
/// sub-agents came with it from the walk in `candidates`, so the store is
/// not walked again for each session a listing shows.
pub(crate) fn describe(candidate: &mut Candidate) {
    let related = std::mem::take(&mut candidate.related);
    candidate.describe_file(related);
    candidate.title = if candidate.name.is_empty() {
        candidate.prompt.clone()
    } else {
        candidate.name.clone()
    };
}

/// A rollout's lines with the sessions of its sub-agents.
pub(crate) fn input(file: &Path, lines: Vec<String>) -> Input {
    Input {
        lines,
        fallback: stem(file),
        blobs: blobs_beside(file),
        agents: agents(file, &store()),
        ..Input::default()
    }
}

/// Whether a transcript's first entries are a Codex rollout's.
pub(crate) fn recognizes(head: &[Value]) -> bool {
    head.iter().any(|entry| {
        entry["type"] == "session_meta"
            || (entry["type"] == "response_item" && !entry["payload"].is_null())
    })
}

/// A sub-agent's thread, as the header of its rollout describes it.
struct Thread {
    id: String,
    file: PathBuf,
    /// The task path it was given.
    path: String,
    nickname: String,
}

/// Sub-agent threads, by the thread that spawned each.
type Threads = HashMap<String, Vec<Thread>>;

/// Notes a rollout whose header says a thread spawned it. Codex writes a
/// sub-agent's rollout beside every other one; its header names its parent
/// thread and the task path it was given.
fn index(threads: &mut Threads, meta: &Value, file: &Path) {
    let spawner = text(meta, "parent_thread_id");
    if !spawner.is_empty() && auxiliary(meta) {
        threads.entry(spawner.to_owned()).or_default().push(Thread {
            id: text(meta, "id").to_owned(),
            file: file.to_owned(),
            path: text(meta, "agent_path").to_owned(),
            nickname: text(meta, "agent_nickname").to_owned(),
        });
    }
}

/// The directory a path is in, with `.` for a bare name.
fn parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if dir.as_os_str().is_empty() => PathBuf::from("."),
        Some(dir) => dir.to_owned(),
        None => path.to_owned(),
    }
}

/// The directory to look in for a rollout's sub-agent threads: Codex's
/// sessions directory when the rollout is in it, else the top of the
/// year/month/day directories the rollout sits in, else its own directory.
fn root(rollout: &Path, sessions: &Path) -> PathBuf {
    let inside = format!("{}{MAIN_SEPARATOR}", sessions.display());
    if rollout.to_string_lossy().starts_with(&inside) {
        return sessions.to_owned();
    }
    let dir = parent(rollout);
    let mut dated = dir.clone();
    for _ in 0..3 {
        let numbered = dated
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.parse::<i64>().is_ok());
        if !numbered {
            return dir;
        }
        dated = parent(&dated);
    }
    dated
}

/// The sub-agent threads under `root`.
fn threads(root: &Path) -> Threads {
    let mut threads = Threads::new();
    for file in rollouts(root) {
        index(&mut threads, &first_json_line(&file)["payload"], &file);
    }
    threads
}

/// The last element of a task path, as Go's `filepath.Base` names it.
fn base(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_separator);
    match trimmed.rfind(is_separator) {
        _ if trimmed.is_empty() => MAIN_SEPARATOR_STR,
        Some(at) => &trimmed[at + 1..],
        None => trimmed,
    }
}

/// The sub-agents of one thread, and theirs.
fn agents_of(threads: &Threads, id: &str, depth: usize) -> Vec<Agent> {
    let mut agents = Vec::new();
    if depth > 8 {
        return agents;
    }
    for thread in threads.get(id).into_iter().flatten() {
        let Ok(lines) = read_lines(&thread.file) else {
            continue;
        };
        let name = if thread.path.is_empty() {
            &thread.nickname
        } else {
            base(&thread.path)
        };
        agents.push(Agent {
            id: thread.path.clone(),
            call: String::new(),
            name: name.to_owned(),
            input: Input {
                lines,
                fallback: stem(&thread.file),
                agents: agents_of(threads, &thread.id, depth + 1),
                ..Input::default()
            },
        });
    }
    agents
}

/// The sub-agents of the session in `rollout`, found from its place in
/// `sessions` or beside it.
fn agents(rollout: &Path, sessions: &Path) -> Vec<Agent> {
    let head = first_json_line(rollout);
    let thread = text(&head["payload"], "id");
    // No rollout is indexed under a parent it does not name, so a thread
    // with no id has no sub-agents to look for.
    if thread.is_empty() {
        return Vec::new();
    }
    agents_of(&threads(&root(rollout, sessions)), thread, 0)
}

/// The rollouts of a thread's sub-agents, and of theirs. A preview made from
/// the session depends on every one of them.
fn agent_files(threads: &Threads, id: &str, depth: usize, files: &mut Vec<PathBuf>) {
    for child in threads.get(id).into_iter().flatten() {
        files.push(child.file.clone());
        if depth < 8 {
            agent_files(threads, &child.id, depth + 1, files);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    const CWD: &str = "/work/project";

    /// A directory of one test's own, not yet there.
    fn scratch(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("slink-import-codex-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Writes a rollout as if it was last written `age` seconds ago: a
    /// header, then what the person asked.
    fn rollout(file: &Path, header: &str, prompt: &str, age: u64) {
        let asked = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": prompt}],
            },
        });
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, format!("{header}\n{asked}\n")).unwrap();
        let at = SystemTime::now() - Duration::from_secs(age);
        let written = fs::File::options().write(true).open(file).unwrap();
        written.set_modified(at).unwrap();
    }

    fn header(id: &str, cwd: &str, rest: &str) -> String {
        format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}"{rest}}}}}"#)
    }

    #[test]
    fn a_long_name_is_cut_where_javascript_cuts_it() {
        let short = "a".repeat(80);
        assert_eq!(truncate(&short), short);
        assert_eq!(truncate(&"a".repeat(81)), format!("{}…", "a".repeat(77)));
        // Two code units each, so 80 of them fit in 40 characters.
        let faces = "\u{1f600}".repeat(40);
        assert_eq!(truncate(&faces), faces);
        // The cut falls inside the first face, and half a character is none.
        let cut = format!("{}{}", "a".repeat(76), "\u{1f600}".repeat(3));
        assert_eq!(truncate(&cut), format!("{}\u{fffd}…", "a".repeat(76)));
    }

    #[test]
    fn turn_ids_match_as_the_keys_of_a_map_do() {
        assert!(same_key(&json!("t1"), &json!("t1")));
        assert!(same_key(&json!(1), &json!(1.0)));
        assert!(same_key(&json!(true), &json!(true)));
        assert!(!same_key(&json!("1"), &json!(1)));
        assert!(!same_key(&json!([1]), &json!([1])));
        assert!(!same_key(&json!({}), &json!({})));
        assert!(!same_key(&json!(null), &json!(null)));
    }

    #[test]
    fn content_becomes_parts() {
        assert_eq!(map_content(None), Vec::<Value>::new());
        assert_eq!(map_content(Some(&json!(""))), Vec::<Value>::new());
        assert_eq!(
            map_content(Some(&json!({"type": "text"}))),
            Vec::<Value>::new()
        );
        assert_eq!(
            map_content(Some(&json!("Done."))),
            [json!({"type": "text", "text": "Done."})]
        );
        let content = json!([
            {"type": "output_text", "text": "a"},
            {"type": "text", "text": null},
            {"type": "input_text", "text": 7},
            {"type": "input_image", "image_url": "data:image/png;base64,AAAA"},
            {"type": "input_image", "image_url": null, "url": "https://example.test/a.png"},
            {"type": "input_image", "url": null},
            {"type": "input_image", "image_url": {"url": "x"}, "url": "https://example.test/b.png"},
            {"type": "input_file", "filename": "notes.txt"},
            {"filename": "untyped"},
            "bare",
        ]);
        assert_eq!(
            map_content(Some(&content)),
            [
                json!({"type": "text", "text": "a"}),
                json!({"type": "text", "text": ""}),
                json!({"type": "text", "text": 7}),
                json!({"type": "image", "url": "data:image/png;base64,AAAA"}),
                json!({"type": "image", "url": "https://example.test/a.png"}),
                // An image that does not say where it is stays as data.
                json!({"type": "data", "data": {"type": "input_image", "url": null}}),
                json!({"type": "data", "data": {
                    "type": "input_image",
                    "image_url": {"url": "x"},
                    "url": "https://example.test/b.png",
                }}),
                json!({"type": "input_file", "filename": "notes.txt"}),
                json!({"type": "data", "data": {"filename": "untyped"}}),
                json!({"type": "data", "data": "bare"}),
            ]
        );
    }

    #[test]
    fn sub_agents_are_looked_for_from_the_top_of_a_rollouts_days() {
        let sessions = Path::new("/home/me/.codex/sessions");
        for (rollout, top) in [
            // In Codex's own store: all of it.
            (
                "/home/me/.codex/sessions/2026/10/08/rollout-a.jsonl",
                "/home/me/.codex/sessions",
            ),
            (
                "/home/me/.codex/sessions/rollout-a.jsonl",
                "/home/me/.codex/sessions",
            ),
            // A copy of a store: the directory its years are in.
            ("/backup/2026/10/08/rollout-a.jsonl", "/backup"),
            // Anywhere else: the rollout's own directory.
            ("/backup/10/08/rollout-a.jsonl", "/backup/10/08"),
            (
                "/fixtures/sub-agent/input.session.jsonl",
                "/fixtures/sub-agent",
            ),
            (
                "/home/me/.codex/sessions-old/rollout-a.jsonl",
                "/home/me/.codex/sessions-old",
            ),
            ("rollout-a.jsonl", "."),
            ("2026/10/08/rollout-a.jsonl", "."),
        ] {
            assert_eq!(
                root(Path::new(rollout), sessions),
                Path::new(top),
                "{rollout}"
            );
        }
    }

    #[test]
    fn an_agent_is_named_by_the_end_of_its_task_path() {
        assert_eq!(base("/root/checker"), "checker");
        assert_eq!(base("/root/checker/"), "checker");
        assert_eq!(base("checker"), "checker");
        assert_eq!(base("/"), MAIN_SEPARATOR_STR);
    }

    // Codex records a sub-agent's work, a review and its own housekeeping as
    // rollouts beside the session, with the same working directory. None of
    // them is the session the person means.
    #[test]
    fn the_latest_session_is_the_persons_own() {
        let store = scratch("latest").join("sessions");
        let day = store.join("2026/10/03");
        assert!(latest_in(&store, Path::new(CWD)).is_none());
        rollout(
            &day.join("rollout-older.jsonl"),
            &header("older", CWD, r#","source":"cli""#),
            "Add the handler",
            7200,
        );
        rollout(
            &day.join("rollout-main.jsonl"),
            &header("main", CWD, r#","source":"cli""#),
            "Rename the handler",
            3600,
        );
        rollout(
            &day.join("rollout-scout.jsonl"),
            &header(
                "scout",
                CWD,
                r#","source":{"subagent":{"thread_spawn":{"parent_thread_id":"main","depth":1}}}"#,
            ),
            "List the call sites",
            60,
        );
        rollout(
            &day.join("rollout-review.jsonl"),
            &header("review", CWD, r#","source":{"subagent":"review"}"#),
            "Review the diff",
            30,
        );
        rollout(
            &day.join("rollout-memory.jsonl"),
            &header(
                "memory",
                CWD,
                r#","source":{"internal":"memory_consolidation"}"#,
            ),
            "Consolidate",
            10,
        );
        rollout(
            &day.join("rollout-other.jsonl"),
            &header("other", "/work/project/src", r#","source":"cli""#),
            "Another project's",
            0,
        );
        // Not a rollout: nothing in it says it is a session.
        rollout(
            &day.join("notes.jsonl"),
            r#"{"cwd":"/work/project"}"#,
            "x",
            0,
        );

        let found = latest_in(&store, Path::new(CWD)).unwrap();
        assert_eq!(found.harness, "codex");
        let main = day.join("rollout-main.jsonl");
        assert!(matches!(&found.loader, Loader::File(file) if *file == main));
        assert_eq!(Some(found.recency), written(&main));
        let lines = found.load().unwrap().lines;
        assert!(lines[1].contains("Rename the handler"), "{lines:?}");
        // The directory is matched as it was recorded, not its parents.
        assert!(latest_in(&store, Path::new("/work")).is_none());
        fs::remove_dir_all(store.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_newest_rollout_anywhere_says_where_and_what_it_is() {
        let store = scratch("anywhere").join("sessions");
        assert!(anywhere_in(&store).is_none());
        let older = store.join("2026/10/03/rollout-older.jsonl");
        let newer = store.join("2026/10/04/rollout-newer.jsonl");
        rollout(
            &older,
            &header("older", "/work/a", ""),
            "Fix the header",
            3600,
        );
        rollout(
            &newer,
            &header("newer", "/work/b", ""),
            "Ship the empty state",
            60,
        );
        let found = anywhere_in(&store).unwrap();
        assert_eq!(found.harness, "codex");
        assert_eq!(found.dir, "/work/b");
        assert_eq!(found.title, "Ship the empty state");
        assert_eq!(found.file.as_deref(), Some(newer.as_path()));
        assert_eq!(Some(found.recency), written(&newer));
        fs::remove_dir_all(store.parent().unwrap()).unwrap();
    }

    // In Codex's own store a sub-agent's rollout is found wherever it is; a
    // rollout somewhere else brings only what is beside it.
    #[test]
    fn sub_agents_are_found_across_the_whole_store() {
        let store = scratch("agents").join("sessions");
        let main = store.join("a/rollout-main.jsonl");
        let spawned = r#","parent_thread_id":"main","agent_nickname":"Scout","source":{"subagent":{"thread_spawn":{"parent_thread_id":"main","depth":1}}}"#;
        rollout(
            &main,
            &header("main", CWD, r#","source":"cli""#),
            "Rename it",
            60,
        );
        rollout(
            &store.join("b/rollout-child.jsonl"),
            &header("child", CWD, spawned),
            "List the call sites",
            30,
        );
        // Named after its parent, but not a sub-agent's thread.
        rollout(
            &store.join("b/rollout-fork.jsonl"),
            &header("fork", CWD, r#","parent_thread_id":"main","source":"cli""#),
            "Try it another way",
            20,
        );
        let found = agents(&main, &store);
        let [agent] = found.as_slice() else {
            panic!("{found:?}");
        };
        // With no task path, the agent goes by its nickname.
        assert_eq!((agent.id.as_str(), agent.name.as_str()), ("", "Scout"));
        assert_eq!(agent.input.fallback, "rollout-child");
        assert_eq!(agent.input.lines.len(), 2);
        assert!(agents(&main, Path::new("/no/such/store")).is_empty());
        let mut files = Vec::new();
        agent_files(&threads(&store), "main", 0, &mut files);
        assert_eq!(files, [store.join("b/rollout-child.jsonl")]);
        fs::remove_dir_all(store.parent().unwrap()).unwrap();
    }

    // A thread that names itself as its parent must not be read forever.
    #[test]
    fn a_thread_that_spawned_itself_ends() {
        let store = scratch("cycle").join("sessions");
        let spawned = r#","parent_thread_id":"loop","agent_path":"/root/loop","source":{"subagent":{"thread_spawn":{"depth":1}}}"#;
        let file = store.join("rollout-loop.jsonl");
        rollout(&file, &header("loop", CWD, spawned), "Again", 0);
        let mut depth = 0;
        let mut found = agents(&file, &store);
        while let Some(agent) = found.pop() {
            depth += 1;
            found = agent.input.agents;
        }
        assert_eq!(depth, 9);
        let mut files = Vec::new();
        agent_files(&threads(&store), "loop", 0, &mut files);
        assert_eq!(files.len(), 9);
        fs::remove_dir_all(store.parent().unwrap()).unwrap();
    }
}
