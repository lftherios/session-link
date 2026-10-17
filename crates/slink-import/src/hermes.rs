//! Hermes Agent. Its database holds an OpenAI-style chat log, a row for each
//! message: role, content, tool_calls, tool_call_id, reasoning, token_count.
//! The walk mirrors the OpenAI normalizer. User and tool rows gather as what
//! the next model call was given; each assistant row becomes an `llm_call`
//! with those messages as its input, and each of its tool calls a
//! `tool_call` span that the tool row with the same id closes.
//!
//! The importer was first written in JavaScript, and a column can hold a
//! value of a type nobody expected. Where the JavaScript decided by what a
//! value is, what counts as set, or how it reads as text, the helpers below
//! decide the same way, so the same rows give the same document.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, ToSql};
use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::db;
use crate::discover::{Candidate, Elsewhere, Found, Loader, user_home};
use crate::images::image_url_part;
use crate::value::{data_part, field, iso_millis, js_string, or_null, put, text, truthy};
use crate::{Agent, Input, LABEL};

pub(crate) fn import(input: &Input) -> Result<Option<Value>, String> {
    let mut run = to_run(&input.session, &input.messages);
    // Hermes links a delegate's session to its parent's, not to the call that
    // delegated, so each goes under the call that was running when it started.
    adopt_agents(&mut run, &input.agents, &HashMap::new(), |agent: &Agent| {
        import(&agent.input).ok().flatten()
    });
    Ok(Some(run))
}

/* ---------------------------------------------- what JavaScript would do */

/// The field when it is set, which is what JavaScript's `a.name || b` asks.
fn set<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| truthy(Some(found)))
}

/// `isoSec`: the seconds since 1970 that Hermes records, fraction included,
/// as an ISO 8601 time with milliseconds. Anything that is not a number
/// counts as 0, and what is finer than a millisecond is cut off toward zero,
/// as JavaScript's TimeClip cuts it.
fn iso_sec(seconds: &Value) -> String {
    iso_millis((seconds.as_f64().unwrap_or(0.0) * 1000.0) as i64)
}

/* ----------------------------------------------------------------- content */

fn text_part(said: &str) -> Value {
    json!({"type": "text", "text": said})
}

/// One part of structured content.
fn to_part(stored: &Value) -> Value {
    if let Some(said) = stored.as_str() {
        return text_part(said);
    }
    let Some(kind) = stored.get("type").and_then(Value::as_str) else {
        return match stored.get("text").and_then(Value::as_str) {
            Some(said) => text_part(said),
            None => data_part(stored),
        };
    };
    if kind == "text" {
        // JavaScript's `p.text ?? ""`: whatever is there, of any type.
        let said = field(stored, "text").cloned().unwrap_or_else(|| json!(""));
        return json!({"type": "text", "text": said});
    }
    // An image is an OpenAI-style part: image_url.url, or image_url itself.
    if kind == "image_url" || kind == "input_image" {
        let url = match stored["image_url"].get("url").and_then(Value::as_str) {
            Some(url) => url,
            None => text(stored, "image_url"),
        };
        if let Some(image) = image_url_part(url) {
            return image;
        }
    }
    // An image part has to say where its image is; one that does not is data.
    if kind == "image" && !stored["url"].is_string() && field(stored, "attachment").is_none() {
        return data_part(stored);
    }
    // Any other part passes through as it is: the format is open-world.
    stored.clone()
}

/// Marks content Hermes stored as JSON. Its store holds text, so a list or
/// dict of parts is written as this prefix followed by JSON.
const STRUCTURED: &str = "\0json:";

/// One stored content value as parts. Text stays text even when it reads as
/// JSON: a pasted object or a tool's JSON output is what was said. Only
/// Hermes's own marker makes content structured.
fn content(raw: Option<&Value>) -> Vec<Value> {
    let Some(raw) = raw.filter(|raw| !raw.is_null()) else {
        return Vec::new();
    };
    let decoded;
    let value = match raw {
        Value::String(said) => {
            let marked = said
                .strip_prefix(STRUCTURED)
                .and_then(|encoded| slink_format::parse(encoded).ok());
            match marked {
                Some(found) => {
                    decoded = found;
                    &decoded
                }
                None => return vec![text_part(said)],
            }
        }
        other => other,
    };
    match value {
        Value::String(said) => vec![text_part(said)],
        Value::Array(parts) => parts.iter().map(to_part).collect(),
        Value::Object(_) => match value["text"].as_str() {
            Some(said) => vec![text_part(said)],
            None => vec![data_part(value)],
        },
        other => vec![text_part(&js_string(other))],
    }
}

/// A tool span's result: stored text as it is, or the parts of a structured
/// result.
fn result(raw: Option<&Value>) -> Value {
    match raw.and_then(Value::as_str) {
        Some(said) if !said.starts_with(STRUCTURED) => json!(said),
        _ => Value::Array(content(raw)),
    }
}

/* -------------------------------------------------------------- tool calls */

/// A tool call the model asked for.
struct ToolCall {
    id: String,
    name: String,
    /// `None` where JavaScript had `undefined`, which JSON leaves out.
    arguments: Option<Value>,
}

impl ToolCall {
    /// `fields` with the call's arguments, when it has any.
    fn with_arguments(&self, mut fields: Value) -> Value {
        if let Some(arguments) = &self.arguments {
            fields["arguments"] = arguments.clone();
        }
        fields
    }
}

/// The `tool_calls` column: OpenAI-shaped JSON, `[{id, function: {name,
/// arguments}}]`, read tolerantly. `sequence` numbers the model calls so
/// far, and names a tool call that has no id of its own.
fn tool_calls(raw: Option<&Value>, sequence: u64) -> Vec<ToolCall> {
    let Some(raw) = raw.filter(|raw| truthy(Some(raw))) else {
        return Vec::new();
    };
    let decoded;
    let list = match raw {
        Value::String(encoded) => match slink_format::parse(encoded) {
            Ok(found) => {
                decoded = found;
                &decoded
            }
            Err(_) => return Vec::new(),
        },
        other => other,
    };
    let mut calls = Vec::new();
    for (index, call) in list.as_array().into_iter().flatten().enumerate() {
        // JavaScript's `tc?.function ?? tc ?? {}`.
        let function = field(call, "function").unwrap_or(call);
        // `fn.arguments ?? fn.input ?? fn.args`: an `args` that is there and
        // null stays a null.
        let arguments = field(function, "arguments")
            .or_else(|| field(function, "input"))
            .or_else(|| function.get("args"))
            .map(|found| match found.as_str().map(slink_format::parse) {
                Some(Ok(parsed)) => parsed,
                // Not JSON, or not text: kept as it is.
                _ => found.clone(),
            });
        // `fn.name ?? tc?.name`, and a call without a name is no call.
        let name = field(function, "name").or_else(|| field(call, "name"));
        let Some(name) = name.filter(|name| truthy(Some(name))) else {
            continue;
        };
        // `tc?.id ?? tc?.tool_call_id`.
        let id = match field(call, "id").or_else(|| field(call, "tool_call_id")) {
            Some(id) => js_string(id),
            None => format!("call_{sequence}_{index}"),
        };
        calls.push(ToolCall {
            id,
            name: js_string(name),
            arguments,
        });
    }
    calls
}

/* ------------------------------------------------------------- the builder */

/// The document for a session's row and its messages' rows.
fn to_run(session: &Value, messages: &[Value]) -> Value {
    // JavaScript's `title || display_name || id || "hermes session"`.
    let name = ["title", "display_name", "id"]
        .into_iter()
        .find_map(|key| set(session, key))
        .cloned()
        .unwrap_or_else(|| json!("hermes session"));
    let started = or_null(session, "started_at");
    let started_at = iso_sec(&started);
    let system = set(session, "system_prompt");
    let provider = set(session, "billing_provider").map_or_else(|| "unknown".to_owned(), js_string);
    let model = field(session, "model").map_or_else(|| "unknown".to_owned(), js_string);

    let mut spans: Vec<Value> = vec![json!({
        "id": "root",
        "parent_id": null,
        "type": "agent",
        "name": name,
        "started_at": started_at,
    })];
    let mut pending: Vec<Value> = Vec::new();
    let mut sequence = 0_u64;
    let mut previous = started.clone();
    let mut last = started;
    // tool call id -> where its span is in `spans`.
    let mut open_tools: HashMap<String, usize> = HashMap::new();

    for message in messages {
        let stamp = or_null(message, "timestamp");
        match text(message, "role") {
            "user" => {
                let said = content(message.get("content"));
                pending.push(json!({"role": "user", "content": said}));
                last = stamp.clone();
            }
            "tool" => {
                let call = message["tool_call_id"].as_str();
                if let Some(at) = call.and_then(|id| open_tools.remove(id)) {
                    let tool = &mut spans[at];
                    tool["ended_at"] = json!(iso_sec(&stamp));
                    tool["output"] = json!({"result": result(message.get("content"))});
                }
                let mut returned = Map::new();
                returned.insert("type".into(), json!("tool_result"));
                returned.insert(
                    "content".into(),
                    Value::Array(content(message.get("content"))),
                );
                put(&mut returned, "tool_call_id", message, "tool_call_id");
                pending.push(json!({"role": "tool", "content": [returned]}));
                last = stamp.clone();
            }
            "assistant" => {
                let calls = tool_calls(message.get("tool_calls"), sequence);
                let mut reply: Vec<Value> = Vec::new();
                // JavaScript's `m.reasoning || m.reasoning_content`.
                let reasoning =
                    set(message, "reasoning").or_else(|| set(message, "reasoning_content"));
                if let Some(reasoning) = reasoning {
                    reply.push(json!({"type": "thinking", "text": js_string(reasoning)}));
                }
                reply.extend(content(message.get("content")));
                for call in &calls {
                    reply.push(call.with_arguments(
                        json!({"type": "tool_call", "id": call.id, "name": call.name}),
                    ));
                }
                let status = if text(message, "finish_reason") == "error" {
                    "error"
                } else {
                    "ok"
                };
                let mut given = Map::new();
                given.insert(
                    "messages".into(),
                    Value::Array(std::mem::take(&mut pending)),
                );
                if let Some(system) = system {
                    given.insert("system".into(), system.clone());
                }
                sequence += 1;
                let turn = format!("s{sequence}");
                let mut span = json!({
                    "id": turn,
                    "parent_id": "root",
                    "type": "llm_call",
                    "name": "turn",
                    "started_at": iso_sec(&previous),
                    "ended_at": iso_sec(&stamp),
                    "status": status,
                    "model": {"id": model, "provider": provider},
                    "input": given,
                    "output": {"messages": [{"role": "assistant", "content": reply}]},
                });
                if let Some(count) = field(message, "token_count") {
                    span["usage"] = json!({"output_tokens": count});
                }
                spans.push(span);
                for call in calls {
                    sequence += 1;
                    let given =
                        call.with_arguments(json!({"name": call.name, "tool_call_id": call.id}));
                    spans.push(json!({
                        "id": format!("s{sequence}"),
                        "parent_id": turn,
                        "type": "tool_call",
                        "name": call.name,
                        "started_at": iso_sec(&stamp),
                        "status": "ok",
                        "input": given,
                    }));
                    open_tools.insert(call.id, spans.len() - 1);
                }
                last = stamp.clone();
            }
            // A system row is not a message: the system prompt is taken from
            // the session's row. A row with any other role is not known.
            _ => {}
        }
        previous = stamp;
    }

    // JavaScript's `session.ended_at ?? lastTs`.
    let ended = iso_sec(field(session, "ended_at").unwrap_or(&last));
    spans[0]["ended_at"] = json!(ended);
    spans[0]["status"] = json!("ok");
    // A session can end on a message nothing answered. Those are kept as a
    // custom span: an llm_call with no response would be an invention.
    if !pending.is_empty() {
        sequence += 1;
        spans.push(json!({
            "id": format!("s{sequence}"),
            "parent_id": "root",
            "type": "custom",
            "name": "trailing message(s) — no assistant reply",
            "started_at": iso_sec(&last),
            "ended_at": iso_sec(&last),
            "input": {"messages": pending},
        }));
    }

    let mut metadata = Map::new();
    put(&mut metadata, "session_id", session, "id");
    // Hermes ends a session when it compresses its context and carries on in
    // a new one that names it as parent. Each is imported on its own, because
    // the new one repeats part of the old; the last two fields link them.
    for (key, column) in [
        ("cwd", "cwd"),
        ("origin", "source"),
        ("parent_session_id", "parent_session_id"),
        ("end_reason", "end_reason"),
    ] {
        if let Some(found) = set(session, column) {
            metadata.insert(key.to_owned(), found.clone());
        }
    }

    json!({
        "schema": "session/v0",
        "name": name,
        "created_at": started_at,
        "source": {"kind": "import", "harness": "hermes", "label": LABEL, "fidelity": "reconstructed"},
        "metadata": metadata,
        "spans": spans,
    })
}

/* ------------------------------------------------ where its sessions are */

/// The session store a plain `hermes` would open, resolved the way Hermes
/// resolves its home: `HERMES_HOME` when it names a profile, else the profile
/// the `active_profile` file in the Hermes root selects, else the root.
/// `HERMES_STATE_DB`, slink's own override, names the file outright.
pub(crate) fn store() -> PathBuf {
    let named = |name: &str| {
        env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    store_from(named("HERMES_STATE_DB"), named("HERMES_HOME"), &user_home())
}

/// The store for what the environment says: the file `HERMES_STATE_DB`
/// names, the directory `HERMES_HOME` names, and the person's home.
fn store_from(state_db: Option<PathBuf>, hermes_home: Option<PathBuf>, home: &Path) -> PathBuf {
    if let Some(file) = state_db {
        return file;
    }
    let mut root = home.join(".hermes");
    if let Some(dir) = hermes_home {
        let in_profiles = dir.parent().and_then(Path::file_name);
        if in_profiles.is_some_and(|name| name == "profiles") {
            return dir.join("state.db");
        }
        root = dir;
    }
    if let Ok(name) = fs::read(root.join("active_profile")) {
        let name = String::from_utf8_lossy(&name);
        let profile = name.trim();
        let dir = root.join("profiles").join(profile);
        if !profile.is_empty()
            && profile != "default"
            && !profile.contains(['/', '\\'])
            && dir.is_dir()
        {
            return dir.join("state.db");
        }
    }
    root.join("state.db")
}

/// The columns of a message's row that an import reads.
const MESSAGE_COLUMNS: &str = "role, content, tool_call_id, tool_calls, tool_name, timestamp, \
     token_count, finish_reason, reasoning, reasoning_content";

fn has(columns: &[String], name: &str) -> bool {
    columns.iter().any(|column| column == name)
}

/// A time in a row, as whole seconds. Hermes records seconds since 1970 with
/// a fraction, and a column that holds no number counts as 0.
fn seconds(row: &Value, column: &str) -> i64 {
    row[column].as_f64().unwrap_or(0.0) as i64
}

/// The session a row of `sessions` lists, to be loaded from the database it
/// was found in. Recency is in nanoseconds, to compare with a file's.
fn found(path: &Path, row: &Value) -> Found {
    Found {
        harness: "hermes",
        recency: seconds(row, "active_at").wrapping_mul(1_000_000_000),
        loader: Loader::Database {
            path: path.to_owned(),
            id: text(row, "id").to_owned(),
        },
    }
}

/// The sessions Hermes recorded for one directory, most recently active
/// first: `active_at` is the time of a session's last message. Hermes links
/// a session to a parent in three cases, told apart by how the parent ended,
/// and this follows the rules of its own session list:
///
/// - A continuation starts after context compression ended its parent.
///   Hermes does not always record a directory for it, so it takes its
///   parent's.
/// - A branch starts after the parent ended as branched, or carries the
///   branch marker. Both are the person's own sessions and are listed.
/// - Anything else started while its parent was live: a sub-agent or a
///   background review. Only an explicit id reaches one.
fn sessions(db: &Connection, dir: &Path, id: &str, limit: usize) -> Result<Vec<Value>, String> {
    let columns = db::columns(db, "sessions");
    let messages = db::columns(db, "messages");
    let active = if has(&messages, "session_id") && has(&messages, "timestamp") {
        "COALESCE((SELECT MAX(m.timestamp) FROM messages m WHERE m.session_id = s.id), \
         s.started_at)"
    } else {
        "s.started_at"
    };
    let fields = format!("s.id, s.title, s.started_at, {active} AS active_at");
    let dir = dir.to_string_lossy().into_owned();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut args: Vec<&dyn ToSql> = vec![&dir];
    let mut query;
    let linked = ["parent_session_id", "end_reason", "ended_at"]
        .iter()
        .all(|name| has(&columns, name));
    if linked {
        let after = |reasons: &str| {
            format!(
                "EXISTS (SELECT 1 FROM sessions p WHERE p.id = s.parent_session_id \
                 AND p.end_reason IN ({reasons}) AND s.started_at >= p.ended_at)"
            )
        };
        query = format!(
            "WITH RECURSIVE here(id) AS (\
             SELECT id FROM sessions WHERE cwd = ? \
             UNION \
             SELECT s.id FROM sessions s JOIN here ON s.parent_session_id = here.id \
             WHERE (s.cwd IS NULL OR s.cwd = '') AND {}) \
             SELECT {fields} FROM sessions s WHERE s.id IN (SELECT id FROM here)",
            after("'compression'")
        );
        if id.is_empty() {
            let mut own = format!(
                "s.parent_session_id IS NULL OR {}",
                after("'compression', 'branched'")
            );
            if has(&columns, "model_config") {
                own.push_str(
                    " OR CASE WHEN json_valid(s.model_config) \
                     THEN json_extract(s.model_config, '$._branched_from') END IS NOT NULL",
                );
            }
            query.push_str(&format!(" AND ({own})"));
        } else {
            query.push_str(" AND s.id = ?");
            args.push(&id);
        }
    } else {
        // A store that does not record how sessions are linked has none to
        // tell apart.
        query = format!("SELECT {fields} FROM sessions s WHERE s.cwd = ?");
        if !id.is_empty() {
            query.push_str(" AND s.id = ?");
            args.push(&id);
        }
    }
    query.push_str(" ORDER BY active_at DESC, s.id LIMIT ?");
    args.push(&limit);
    db::rows(db, &query, &args)
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    latest_in(&store(), cwd)
}

/// The session the database at `path` last recorded a message of for `cwd`.
fn latest_in(path: &Path, cwd: &Path) -> Option<Found> {
    let db = db::open(path).ok()?;
    let rows = sessions(&db, cwd, "", 1).ok()?;
    Some(found(path, rows.first()?))
}

pub(crate) fn anywhere() -> Option<Elsewhere> {
    anywhere_in(&store())
}

/// The session the database at `path` saw start last, whatever its
/// directory.
fn anywhere_in(path: &Path) -> Option<Elsewhere> {
    let db = db::open(path).ok()?;
    let query = "SELECT cwd, title, started_at FROM sessions ORDER BY started_at DESC LIMIT 1";
    let rows = db::rows(&db, query, &[]).ok()?;
    let row = rows.first()?;
    Some(Elsewhere {
        harness: "hermes",
        dir: text(row, "cwd").to_owned(),
        file: None,
        title: text(row, "title").to_owned(),
        recency: seconds(row, "started_at").wrapping_mul(1_000_000_000),
    })
}

/// The sessions the database at `store` holds for each of `dirs`, at most
/// `limit` for each; one named by `id` only, when an id is given. A machine
/// without the harness has no database, which is no sessions and no error.
pub(crate) fn candidates(
    store: &Path,
    dirs: &[PathBuf],
    id: &str,
    limit: usize,
) -> Result<Vec<Candidate>, String> {
    if !store.exists() {
        return Ok(Vec::new());
    }
    let unreadable = |error: String| format!("cannot read hermes sessions: {error}");
    let db = db::open(store).map_err(unreadable)?;
    let mut listed = Vec::new();
    for dir in dirs {
        for row in sessions(&db, dir, id, limit).map_err(unreadable)? {
            let started = seconds(&row, "started_at");
            listed.push(Candidate {
                found: found(store, &row),
                id: text(&row, "id").to_owned(),
                title: text(&row, "title").to_owned(),
                dir: dir.clone(),
                file: None,
                related: Vec::new(),
                name: String::new(),
                prompt: String::new(),
                started: (started > 0).then_some((started, 0)),
            });
        }
    }
    Ok(listed)
}

/// One session of the database at `path`, with its messages and the
/// sessions of its sub-agents, as an importer's input.
pub(crate) fn load_session(path: &Path, id: &str) -> Result<Input, String> {
    let db = db::open(path)?;
    load(&db, path, id, 0)
}

fn load(db: &Connection, path: &Path, id: &str, depth: usize) -> Result<Input, String> {
    let session = db::rows(db, "SELECT * FROM sessions WHERE id = ?", &[&id])?
        .into_iter()
        .next()
        .ok_or_else(|| format!("hermes session {id:?} not found in {}", path.display()))?;
    let mut query = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE session_id = ?");
    if has(&db::columns(db, "messages"), "active") {
        // Rewinding keeps the undone messages with active = 0. Hermes leaves
        // them out of the conversation it shows and resumes, and so does this.
        query.push_str(" AND active = 1");
    }
    query.push_str(" ORDER BY id");
    let messages = db::rows(db, &query, &[&id])
        .map_err(|error| format!("cannot read hermes messages: {error}"))?;
    let mut input = Input {
        session,
        messages,
        ..Input::default()
    };
    // A session started while this one was live is a delegate's. One started
    // after this one ended is a continuation or a branch: a session of its own.
    let columns = db::columns(db, "sessions");
    if has(&columns, "parent_session_id") && has(&columns, "ended_at") && depth < 8 {
        let children = db::rows(
            db,
            "SELECT s.id, s.title FROM sessions s JOIN sessions p ON p.id = s.parent_session_id \
             WHERE p.id = ? AND (p.ended_at IS NULL OR s.started_at < p.ended_at) \
             ORDER BY s.started_at, s.id",
            &[&id],
        )
        .unwrap_or_default();
        for child in children {
            let child_id = text(&child, "id");
            if let Ok(agent) = load(db, path, child_id, depth + 1) {
                input.agents.push(Agent {
                    id: child_id.to_owned(),
                    call: String::new(),
                    name: text(&child, "title").to_owned(),
                    input: agent,
                });
            }
        }
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for one test.
    fn scratch(test: &str) -> PathBuf {
        let name = format!("slink-import-hermes-{test}-{}", std::process::id());
        let dir = env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn seconds_are_written_as_the_time_go_writes() {
        let epoch = "1970-01-01T00:00:00.000Z";
        for (seconds, written) in [
            (json!(0), epoch),
            (json!(1_752_900_003.25), "2025-07-19T04:40:03.250Z"),
            (json!(951_782_400), "2000-02-29T00:00:00.000Z"),
            (json!(4_107_542_400_i64), "2100-03-01T00:00:00.000Z"),
            // What is finer than a millisecond is cut off, toward zero.
            (json!(0.9999), "1970-01-01T00:00:00.999Z"),
            (json!(-0.9999), "1969-12-31T23:59:59.001Z"),
            (json!(-0.0005), epoch),
            (json!(-1.5), "1969-12-31T23:59:58.500Z"),
            // Years that are not four digits long.
            (json!(-62_167_219_200_i64), "0000-01-01T00:00:00.000Z"),
            (json!(-62_167_219_201_i64), "-0001-12-31T23:59:59.000Z"),
            (json!(253_402_300_800_i64), "10000-01-01T00:00:00.000Z"),
            // Anything that is not a number is no time at all.
            (json!("1752900003"), epoch),
            (json!(true), epoch),
            (json!(null), epoch),
        ] {
            assert_eq!(iso_sec(&seconds), written, "{seconds}");
        }
    }

    #[test]
    fn content_is_structured_only_by_its_marker() {
        let marked = |encoded: &str| json!(format!("{STRUCTURED}{encoded}"));
        assert_eq!(content(None), Vec::<Value>::new());
        assert_eq!(content(Some(&Value::Null)), Vec::<Value>::new());
        // Text that reads as JSON is still what was said.
        let pasted = "{\"text\": \"use this\"}";
        assert_eq!(content(Some(&json!(pasted))), [text_part(pasted)]);
        assert_eq!(content(Some(&marked(pasted))), [text_part("use this")]);
        assert_eq!(
            content(Some(&marked("{\"size\": 9}"))),
            [json!({"type": "data", "data": {"size": 9}})]
        );
        assert_eq!(content(Some(&marked("\"said\""))), [text_part("said")]);
        assert_eq!(content(Some(&marked("1.50"))), [text_part("1.5")]);
        assert_eq!(content(Some(&marked("null"))), [text_part("null")]);
        // A marker with no JSON after it is text, marker and all.
        let cut = marked("[\"cut");
        assert_eq!(content(Some(&cut)), [json!({"type": "text", "text": cut})]);
        // A value the store should not hold reads as JavaScript reads it.
        assert_eq!(content(Some(&json!(false))), [text_part("false")]);
        assert_eq!(content(Some(&json!({"text": "kept"}))), [text_part("kept")]);

        let parts = marked(
            r#"["bare", {"type": "text"}, {"type": "text", "text": 5}, {"text": "typeless"},
                {"type": "input_image", "image_url": "https://example.test/a.png"},
                {"type": "image_url", "image_url": {"url": "file:///a.png"}},
                {"type": "image"}, {"type": "image", "url": "https://example.test/b.png"},
                {"type": "audio", "data": "zz"}, {"size": 9}, 5]"#,
        );
        assert_eq!(
            content(Some(&parts)),
            [
                text_part("bare"),
                text_part(""),
                json!({"type": "text", "text": 5}),
                text_part("typeless"),
                json!({"type": "image", "url": "https://example.test/a.png"}),
                // Not an address the viewer may show: the part as it was.
                json!({"type": "image_url", "image_url": {"url": "file:///a.png"}}),
                json!({"type": "data", "data": {"type": "image"}}),
                json!({"type": "image", "url": "https://example.test/b.png"}),
                json!({"type": "audio", "data": "zz"}),
                json!({"type": "data", "data": {"size": 9}}),
                json!({"type": "data", "data": 5}),
            ]
        );

        // A tool's result is its text, or the parts of a structured one.
        assert_eq!(result(Some(&json!(pasted))), json!(pasted));
        assert_eq!(
            result(Some(&marked("[\"shown\"]"))),
            json!([text_part("shown")])
        );
        assert_eq!(result(None), json!([]));
    }

    #[test]
    fn tool_calls_are_read_tolerantly() {
        let stored = json!([
            {"id": "a", "type": "function", "function": {"name": "f", "arguments": "{\"k\": 1}"}},
            {"function": {"name": "g", "arguments": "not json"}},
            {"id": 7, "name": "direct", "input": {"k": 2}},
            {"tool_call_id": "b", "function": {"name": "f", "args": null}},
            {"id": "c", "function": {"name": ["f", "g"]}},
            {"id": "d", "function": {"name": ""}, "name": "never read"},
            {"id": "e", "function": "not an object", "name": "outer"},
            "not a call",
            {"function": {"name": "h", "arguments": "null"}},
        ]);
        let read = |raw: &Value| -> Vec<(String, String, Option<Value>)> {
            tool_calls(Some(raw), 3)
                .into_iter()
                .map(|call| (call.id, call.name, call.arguments))
                .collect()
        };
        let call = |id: &str, name: &str, arguments: Option<Value>| {
            (id.to_owned(), name.to_owned(), arguments)
        };
        let wanted = [
            call("a", "f", Some(json!({"k": 1}))),
            // No id of its own: named by the model call and its place.
            call("call_3_1", "g", Some(json!("not json"))),
            call("7", "direct", Some(json!({"k": 2}))),
            call("b", "f", Some(Value::Null)),
            call("c", "f,g", None),
            call("e", "outer", None),
            call("call_3_8", "h", Some(Value::Null)),
        ];
        // As Hermes stores them, and as a list already read.
        assert_eq!(read(&json!(stored.to_string())), wanted);
        assert_eq!(read(&stored), wanted);
        for nothing in [
            json!(null),
            json!(""),
            json!("not json"),
            json!("{}"),
            json!(0),
        ] {
            assert!(read(&nothing).is_empty(), "{nothing}");
        }
        assert!(tool_calls(None, 3).is_empty());
    }

    // Each harness can be told to keep its store somewhere else, and Hermes
    // is followed there by the rule it applies itself.
    #[test]
    fn the_store_is_the_one_hermes_would_open() {
        let home = scratch("store");
        let at = |state_db: Option<&Path>, hermes_home: Option<&Path>| {
            store_from(
                state_db.map(Path::to_owned),
                hermes_home.map(Path::to_owned),
                &home,
            )
        };
        let root = home.join(".hermes");
        assert_eq!(at(None, None), root.join("state.db"), "default");
        let work = root.join("profiles").join("work");
        fs::create_dir_all(&work).unwrap();
        let active = root.join("active_profile");
        fs::write(&active, "work\n").unwrap();
        assert_eq!(at(None, None), work.join("state.db"), "active profile");
        fs::write(&active, "gone\n").unwrap();
        let stays = "a profile that does not exist";
        assert_eq!(at(None, None), root.join("state.db"), "{stays}");
        fs::write(&active, "default\n").unwrap();
        assert_eq!(at(None, None), root.join("state.db"), "default profile");
        fs::write(&active, "../profiles/work\n").unwrap();
        assert_eq!(at(None, None), root.join("state.db"), "a path, not a name");

        let other = home.join("data");
        let coder = other.join("profiles").join("coder");
        assert_eq!(
            at(None, Some(&coder)),
            coder.join("state.db"),
            "HERMES_HOME naming a profile"
        );
        assert_eq!(
            at(None, Some(&other)),
            other.join("state.db"),
            "HERMES_HOME naming a root"
        );
        let ops = other.join("profiles").join("ops");
        fs::create_dir_all(&ops).unwrap();
        fs::write(other.join("active_profile"), "ops").unwrap();
        assert_eq!(
            at(None, Some(&other)),
            ops.join("state.db"),
            "HERMES_HOME root with an active profile"
        );
        let explicit = home.join("explicit.db");
        assert_eq!(
            at(Some(&explicit), Some(&other)),
            explicit,
            "HERMES_STATE_DB"
        );
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn the_newest_session_in_a_database_is_found() {
        let dir = scratch("newest");
        let path = dir.join("state.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT, started_at REAL,
                 ended_at REAL, end_reason TEXT, cwd TEXT, title TEXT);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT,
                 role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT,
                 timestamp REAL, token_count INTEGER, finish_reason TEXT, reasoning TEXT,
                 reasoning_content TEXT);
             INSERT INTO sessions VALUES
                 ('first', NULL, 100, 200, 'compression', '/work/project', 'Billing refactor'),
                 ('continued', 'first', 200, NULL, NULL, NULL, 'Billing refactor #2'),
                 ('delegate', 'continued', 250, 260, 'completed', '/work/project', NULL),
                 ('elsewhere', NULL, 500.75, NULL, NULL, '/other', 'Other project');
             INSERT INTO messages (session_id, role, content, timestamp)
                 VALUES ('continued', 'user', 'carry on', 300.9);",
        )
        .unwrap();
        drop(db);
        let cwd = Path::new("/work/project");
        const SECOND: i64 = 1_000_000_000;

        // The continuation has no directory of its own, and is the newest
        // of its conversation's: as recent as its last message, in whole
        // seconds. The delegate started later and is nobody's own session.
        let latest = latest_in(&path, cwd).unwrap();
        assert_eq!((latest.harness, latest.recency), ("hermes", 300 * SECOND));
        assert!(matches!(
            &latest.loader,
            Loader::Database { path: at, id } if *at == path && id == "continued"
        ));
        assert_eq!(
            latest.load().unwrap().session["title"],
            "Billing refactor #2"
        );
        // Only the directory asked about, not the ones above it.
        assert!(latest_in(&path, Path::new("/work")).is_none());

        let newest = anywhere_in(&path).unwrap();
        assert_eq!(
            (newest.harness, newest.dir.as_str(), newest.title.as_str()),
            ("hermes", "/other", "Other project")
        );
        assert_eq!((newest.recency, newest.file), (500 * SECOND, None));

        // The limit is for each directory, and an id is matched before it.
        let dirs = [cwd.to_owned(), PathBuf::from("/other")];
        let ids = |id: &str, limit: usize| -> Vec<String> {
            let listed = candidates(&path, &dirs, id, limit).unwrap();
            listed.into_iter().map(|candidate| candidate.id).collect()
        };
        assert_eq!(ids("", 30), ["continued", "first", "elsewhere"]);
        assert_eq!(ids("", 1), ["continued", "elsewhere"]);
        assert_eq!(ids("delegate", 1), ["delegate"]);
        assert_eq!(ids("first", 1), ["first"]);

        // A machine without Hermes has no database: nothing is found, and
        // only loading a session from it is an error.
        let absent = dir.join("absent.db");
        assert!(latest_in(&absent, cwd).is_none());
        assert!(anywhere_in(&absent).is_none());
        assert!(candidates(&absent, &dirs, "", 30).unwrap().is_empty());
        assert!(load_session(&absent, "first").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
