//! opencode (sst/opencode). A session is a row in its SQLite database, each
//! message of the session a row, and each part of a message a row of its
//! own. User messages gather as what the next model call was given; each
//! assistant message becomes an `llm_call` with those messages as its input,
//! and each tool part of it a `tool_call` span under that call.
//!
//! The importer was written in JavaScript first, and this keeps to that one
//! as the Go one does: the same fields picked, `??` told from `||`, the same
//! sequence of span ids, the same handling of times, and a value of a kind
//! nobody expected read as missing and never refused.
//! `testdata/import/opencode` pins the result.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
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
    Ok(Some(to_run(input)))
}

/* ------------------------------------------------ what JavaScript would do */

/// A time opencode wrote, in whole milliseconds since 1970. Anything that
/// is not a number is zero, and what a number has below a millisecond is
/// dropped. A number no i64 holds becomes the nearest one that does, where
/// Go leaves the answer to the processor it runs on.
fn millis(time: &Value) -> i64 {
    time.as_f64().unwrap_or(0.0) as i64
}

/// `new Date(ms).toISOString()` for a time opencode wrote.
fn iso(time: &Value) -> String {
    iso_millis(millis(time))
}

/* ---------------------------------------------------------------- content */

/// A file part: an image opencode recorded as a data URL becomes an image,
/// and any other file is kept as recorded.
fn file_part(part: &Value) -> Value {
    if text(part, "mime").starts_with("image/")
        && let Some(image) = image_url_part(text(part, "url"))
    {
        return image;
    }
    data_part(part)
}

/// The text of a text or reasoning part, empty when it recorded none.
fn text_of(part: &Value) -> Value {
    field(part, "text").cloned().unwrap_or_else(|| json!(""))
}

/// The parts of a user message. opencode marks text it wrote itself as
/// synthetic, such as its account of reading an attached file; that is
/// returned apart, as context, from what the person typed and attached.
fn user_content(parts: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let (mut typed, mut injected) = (Vec::new(), Vec::new());
    for part in parts {
        match text(part, "type") {
            "text" => {
                let said = json!({"type": "text", "text": text_of(part)});
                if part["synthetic"] == true {
                    injected.push(said);
                } else {
                    typed.push(said);
                }
            }
            "file" => typed.push(file_part(part)),
            // A part of any other kind is kept as it is, not dropped.
            _ if truthy(part.get("type")) => typed.push(data_part(part)),
            _ => {}
        }
    }
    (typed, injected)
}

/// The parts of an assistant message: text, reasoning as thinking, a tool
/// part as the call the model made, and a file the model produced.
/// `step-start`, `step-finish` and `patch` parts are structure and are
/// passed over.
fn assistant_content(parts: &[Value]) -> Vec<Value> {
    let mut content = Vec::new();
    for part in parts {
        match text(part, "type") {
            kind @ ("text" | "reasoning") => content.push(json!({
                "type": if kind == "text" { "text" } else { "thinking" },
                "text": text_of(part),
            })),
            "file" => content.push(file_part(part)),
            "tool" => {
                let Some(call) = part["callID"].as_str() else {
                    continue;
                };
                let mut tool = Map::new();
                tool.insert("type".into(), json!("tool_call"));
                tool.insert("id".into(), json!(call));
                // A name or arguments the part never had are left out, as
                // JSON leaves out what was never assigned.
                put(&mut tool, "name", part, "tool");
                put(&mut tool, "arguments", &part["state"], "input");
                content.push(Value::Object(tool));
            }
            _ => {}
        }
    }
    content
}

/// A message's token counts under the format's names, or `None` when it
/// recorded none of them.
fn usage(tokens: &Value) -> Option<Value> {
    if !truthy(Some(tokens)) {
        return None;
    }
    let mut counts = Map::new();
    for (name, recorded) in [
        ("input_tokens", "input"),
        ("output_tokens", "output"),
        ("reasoning_tokens", "reasoning"),
    ] {
        if let Some(count) = field(tokens, recorded) {
            counts.insert(name.into(), count.clone());
        }
    }
    if let Some(count) = field(&tokens["cache"], "read") {
        counts.insert("cache_read_tokens".into(), count.clone());
    }
    (!counts.is_empty()).then_some(Value::Object(counts))
}

/* ------------------------------------------------------------ the builder */

/// The document for a session's row and its messages' rows, with the
/// sessions of its sub-agents nested in.
fn to_run(input: &Input) -> Value {
    let session = &input.session;
    // The model is JSON kept as text. An object is taken as it is, and a
    // lookup in anything else finds nothing.
    let model = db::parsed(&session["model"]);
    // The title, else the id, else a name of our own. This is `||`, so an
    // empty title is no title.
    let name = [session.get("title"), session.get("id")]
        .into_iter()
        .find(|named| truthy(*named))
        .flatten()
        .cloned()
        .unwrap_or_else(|| json!("opencode session"));
    let started_at = iso(&session["time_created"]);

    let mut spans: Vec<Value> = vec![json!({
        "id": "root",
        "parent_id": null,
        "type": "agent",
        "name": name,
        "started_at": started_at,
    })];

    let mut pending: Vec<Value> = Vec::new();
    // child session id -> the task call that started it
    let mut agent_calls: HashMap<String, String> = HashMap::new();
    let mut sequence = 0_u64;
    let mut last = or_null(session, "time_created");

    for message in &input.messages {
        let data = &message["data"];
        let parts = message["parts"].as_array().map_or(&[][..], Vec::as_slice);
        match text(data, "role") {
            "user" => {
                let (typed, injected) = user_content(parts);
                if !injected.is_empty() {
                    pending.push(json!({"role": "system", "content": injected}));
                }
                pending.push(json!({"role": "user", "content": typed}));
                last = or_null(message, "time_created");
            }
            "assistant" => {
                // It started when the message says it was created, else when
                // its row was written, and finished when the message says it
                // completed, else when it started.
                let created = field(&data["time"], "created")
                    .or_else(|| field(message, "time_created"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let completed = field(&data["time"], "completed")
                    .cloned()
                    .unwrap_or_else(|| created.clone());
                let errored = truthy(data.get("error")) || data["finish"] == "error";
                // The message's own model, else the session's.
                let named = |own: &str, of_session: &str| {
                    field(data, own)
                        .or_else(|| field(&model, of_session))
                        .map_or_else(|| "unknown".to_owned(), js_string)
                };

                sequence += 1;
                let id = format!("s{sequence}");
                let mut span = json!({
                    "id": id,
                    "parent_id": "root",
                    "type": "llm_call",
                    "name": "turn",
                    "started_at": iso(&created),
                    "ended_at": iso(&completed),
                    "status": if errored { "error" } else { "ok" },
                    "model": {
                        "id": named("modelID", "id"),
                        "provider": named("providerID", "providerID"),
                    },
                    "input": {"messages": std::mem::take(&mut pending)},
                    "output": {"messages": [{
                        "role": "assistant",
                        "content": assistant_content(parts),
                    }]},
                });
                if let Some(usage) = usage(&data["tokens"]) {
                    span["usage"] = usage;
                }
                if errored {
                    // The error's message, else the error itself, else a
                    // word for it.
                    let why = field(&data["error"], "message")
                        .or_else(|| field(data, "error"))
                        .map_or_else(|| "error".to_owned(), js_string);
                    span["error"] = json!({"message": why});
                }
                spans.push(span);

                for part in parts {
                    if text(part, "type") != "tool" {
                        continue;
                    }
                    let Some(call) = part["callID"].as_str() else {
                        continue;
                    };
                    let state = &part["state"];
                    let failed = state["status"] == "error";
                    // The task tool records the session of the sub-agent it ran.
                    let child = text(&state["metadata"], "sessionId");
                    if !child.is_empty() {
                        agent_calls.insert(child.to_owned(), call.to_owned());
                    }

                    let mut tool_input = Map::new();
                    tool_input.insert("tool_call_id".into(), json!(call));
                    put(&mut tool_input, "name", part, "tool");
                    put(&mut tool_input, "arguments", state, "input");

                    // What the tool gave back: its output, else its error,
                    // else the output its metadata holds.
                    let mut result = field(state, "output")
                        .or_else(|| field(state, "error"))
                        .or_else(|| field(&state["metadata"], "output"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    // A tool can return files beside its text, such as the
                    // image it read. They are part of what the model received.
                    if let Some(attachments) = state["attachments"]
                        .as_array()
                        .filter(|attachments| !attachments.is_empty())
                    {
                        let mut returned = Vec::new();
                        match &result {
                            Value::String(said) if !said.is_empty() => {
                                returned.push(json!({"type": "text", "text": said}));
                            }
                            Value::String(_) | Value::Null => {}
                            other => returned.push(data_part(other)),
                        }
                        returned.extend(attachments.iter().map(file_part));
                        result = Value::Array(returned);
                    }
                    let mut output = Map::new();
                    output.insert("result".into(), result);
                    if failed {
                        output.insert("is_error".into(), Value::Bool(true));
                    }

                    sequence += 1;
                    spans.push(json!({
                        "id": format!("s{sequence}"),
                        "parent_id": id,
                        "type": "tool_call",
                        // This is `||` too: a tool with an empty name is "tool".
                        "name": part.get("tool")
                            .filter(|tool| truthy(Some(*tool)))
                            .cloned()
                            .unwrap_or_else(|| json!("tool")),
                        "started_at": iso(field(&state["time"], "start").unwrap_or(&created)),
                        "ended_at": iso(field(&state["time"], "end").unwrap_or(&completed)),
                        "status": if failed { "error" } else { "ok" },
                        "input": tool_input,
                        "output": output,
                    }));
                }
                last = completed;
            }
            // Other roles, such as summaries, are passed over.
            _ => {}
        }
    }

    let ended_at = iso(&last);
    spans[0]["ended_at"] = json!(ended_at);
    spans[0]["status"] = json!("ok");
    if !pending.is_empty() {
        sequence += 1;
        spans.push(json!({
            "id": format!("s{sequence}"),
            "parent_id": "root",
            "type": "custom",
            "name": "trailing message(s) — no assistant reply",
            "started_at": ended_at,
            "ended_at": ended_at,
            "input": {"messages": pending},
        }));
    }

    let mut metadata = Map::new();
    // The id is kept whenever the row has one, whatever it is: JSON leaves
    // out only what was never there. The other two are kept when set.
    put(&mut metadata, "session_id", session, "id");
    if truthy(session.get("directory")) {
        metadata.insert("cwd".into(), session["directory"].clone());
    }
    if truthy(session.get("version")) {
        metadata.insert("harness_version".into(), session["version"].clone());
    }

    let mut run = json!({
        "schema": "session/v0",
        "name": name,
        "created_at": started_at,
        "source": {
            "kind": "import",
            "harness": "opencode",
            "label": LABEL,
            "fidelity": "reconstructed",
        },
        "metadata": metadata,
        "spans": spans,
    });
    adopt_agents(&mut run, &input.agents, &agent_calls, |agent: &Agent| {
        Some(to_run(&agent.input))
    });
    run
}

/* ------------------------------------------------ where its sessions are */

/// opencode's session store. `OPENCODE_DB` names another one, by absolute
/// path or by file name inside opencode's data directory.
pub(crate) fn store() -> PathBuf {
    store_from(
        &user_home(),
        env::var_os("XDG_DATA_HOME"),
        env::var_os("OPENCODE_DB"),
    )
}

/// The store for a home directory and what the two variables hold.
fn store_from(home: &Path, data_home: Option<OsString>, named: Option<OsString>) -> PathBuf {
    let data = data_home
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home.join(".local").join("share"), PathBuf::from)
        .join("opencode");
    // opencode can be told to keep its sessions in memory, which leaves
    // nothing to read; slink then looks where it would otherwise keep them.
    match named.filter(|name| !name.is_empty() && name.to_str() != Some(":memory:")) {
        Some(name) if Path::new(&name).is_absolute() => PathBuf::from(name),
        Some(name) => data.join(name),
        None => data.join("opencode.db"),
    }
}

/// A time opencode wrote, in nanoseconds: what the time a transcript file
/// was written at is compared in.
fn nanos(time: &Value) -> i64 {
    millis(time).wrapping_mul(1_000_000)
}

/// The sessions opencode recorded for one directory, most recently active
/// first: `active_at` is when the session last changed, which is how
/// opencode orders its own list. A session with a parent is a sub-agent's
/// task, which opencode leaves out; only an explicit id reaches one.
fn sessions(db: &Connection, dir: &Path, id: &str, limit: usize) -> Result<Vec<Value>, String> {
    // A store that predates a column is asked for what it has.
    let columns = db::columns(db, "session");
    let has = |name: &str| columns.iter().any(|column| column == name);
    let active = if has("time_updated") {
        "COALESCE(time_updated, time_created)"
    } else {
        "time_created"
    };
    let mut query = format!(
        "SELECT id, title, time_created, {active} AS active_at FROM session WHERE directory = ?"
    );
    let dir = dir.to_string_lossy();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut args: Vec<&dyn ToSql> = vec![&dir];
    if !id.is_empty() {
        query.push_str(" AND id = ?");
        args.push(&id);
    } else if has("parent_id") {
        query.push_str(" AND parent_id IS NULL");
    }
    query.push_str(" ORDER BY active_at DESC, id LIMIT ?");
    args.push(&limit);
    db::rows(db, &query, &args)
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    latest_in(&store(), cwd)
}

/// The session for `cwd` that the database at `path` saw work on last.
fn latest_in(path: &Path, cwd: &Path) -> Option<Found> {
    let db = db::open(path).ok()?;
    let rows = sessions(&db, cwd, "", 1).ok()?;
    let row = rows.first()?;
    Some(Found {
        harness: "opencode",
        recency: nanos(&row["active_at"]),
        loader: Loader::Database {
            path: path.to_owned(),
            id: text(row, "id").to_owned(),
        },
    })
}

pub(crate) fn anywhere() -> Option<Elsewhere> {
    anywhere_in(&store())
}

/// The session the database at `path` holds that started last, whatever
/// directory it was for.
fn anywhere_in(path: &Path) -> Option<Elsewhere> {
    let db = db::open(path).ok()?;
    let rows = db::rows(
        &db,
        "SELECT directory, title, time_created FROM session ORDER BY time_created DESC LIMIT 1",
        &[],
    )
    .ok()?;
    let row = rows.first()?;
    Some(Elsewhere {
        harness: "opencode",
        dir: text(row, "directory").to_owned(),
        file: None,
        title: text(row, "title").to_owned(),
        recency: nanos(&row["time_created"]),
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
    if fs::metadata(store).is_err() {
        return Ok(Vec::new());
    }
    let unread = |error: String| format!("cannot read opencode sessions: {error}");
    let db = db::open(store).map_err(unread)?;
    let mut found = Vec::new();
    for dir in dirs {
        for row in sessions(&db, dir, id, limit).map_err(unread)? {
            let id = text(&row, "id").to_owned();
            // A row with no start has none, which is not 1970.
            let started = (millis(&row["time_created"]) > 0).then(|| {
                let at = nanos(&row["time_created"]);
                (
                    at.div_euclid(1_000_000_000),
                    at.rem_euclid(1_000_000_000) as u32,
                )
            });
            found.push(Candidate {
                found: Found {
                    harness: "opencode",
                    recency: nanos(&row["active_at"]),
                    // The session is named by its id, so what is loaded is
                    // the one that was chosen and not whichever is newest
                    // by then.
                    loader: Loader::Database {
                        path: store.to_owned(),
                        id: id.clone(),
                    },
                },
                id,
                title: text(&row, "title").to_owned(),
                dir: dir.clone(),
                file: None,
                related: Vec::new(),
                name: String::new(),
                prompt: String::new(),
                started,
            });
        }
    }
    Ok(found)
}

/// One session of the database at `path`, with its messages and the
/// sessions of its sub-agents, as an importer's input.
pub(crate) fn load_session(path: &Path, id: &str) -> Result<Input, String> {
    let db = db::open(path)?;
    session_input(&db, path, id, 0)
}

/// A session's row, its messages each with their parts, and the same for
/// every session it started. `depth` counts the sessions between this one
/// and the one that was asked for, and sub-agents are followed eight deep.
fn session_input(db: &Connection, path: &Path, id: &str, depth: u8) -> Result<Input, String> {
    let session = db::rows(db, "SELECT * FROM session WHERE id = ?", &[&id])?
        .into_iter()
        .next()
        .ok_or_else(|| {
            format!(
                "opencode session {} not found in {}",
                quoted(id),
                path.display()
            )
        })?;
    // A store these queries do not fit must fail here, not import a session
    // with its evidence missing.
    let message_rows = db::rows(
        db,
        "SELECT id, time_created, data FROM message WHERE session_id = ? ORDER BY time_created, id",
        &[&id],
    )
    .map_err(|error| format!("cannot read opencode messages: {error}"))?;
    let part_rows = db::rows(
        db,
        "SELECT message_id, time_created, data FROM part WHERE session_id = ? ORDER BY time_created, id",
        &[&id],
    )
    .map_err(|error| format!("cannot read opencode message parts: {error}"))?;
    let mut parts: HashMap<&str, Vec<Value>> = HashMap::new();
    for row in &part_rows {
        parts
            .entry(text(row, "message_id"))
            .or_default()
            .push(db::parsed(&row["data"]));
    }
    let messages = message_rows
        .iter()
        .map(|row| {
            json!({
                "id": row["id"],
                "time_created": row["time_created"],
                "data": db::parsed(&row["data"]),
                // Null for a message with no parts, as Go writes a list
                // nothing was added to.
                "parts": parts.get(text(row, "id")),
            })
        })
        .collect();
    let mut input = Input {
        session,
        messages,
        ..Input::default()
    };
    // A session with this one as parent is a sub-agent's task.
    if depth < 8
        && db::columns(db, "session")
            .iter()
            .any(|column| column == "parent_id")
    {
        let children = db::rows(
            db,
            "SELECT id, title FROM session WHERE parent_id = ? ORDER BY time_created, id",
            &[&id],
        )
        .unwrap_or_default();
        for child in &children {
            let child_id = text(child, "id");
            if let Ok(agent) = session_input(db, path, child_id, depth + 1) {
                input.agents.push(Agent {
                    id: child_id.to_owned(),
                    call: String::new(),
                    name: text(child, "title").to_owned(),
                    input: agent,
                });
            }
        }
    }
    Ok(input)
}

/// Text in double quotes as Go's `%q` writes it, so an id someone typed
/// with a line break in it stays on the line of the message that names it.
fn quoted(text: &str) -> String {
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
            control if control.is_control() && control.is_ascii() => {
                out.push_str(&format!("\\x{:02x}", control as u32));
            }
            control if control.is_control() => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            shown => out.push(shown),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database of this test's own in the system's temporary directory,
    /// holding what `statements` put in it.
    fn database(name: &str, statements: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "slink-import-opencode-{name}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        let _ = fs::remove_file(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(statements).unwrap();
        path
    }

    fn session_id(found: &Found) -> &str {
        match &found.loader {
            Loader::Database { id, .. } => id,
            other => panic!("not a database session: {other:?}"),
        }
    }

    #[test]
    fn a_time_is_written_as_javascript_writes_a_date() {
        for (time, written) in [
            (json!(1_752_900_000_000_i64), "2025-07-19T04:40:00.000Z"),
            (json!(0), "1970-01-01T00:00:00.000Z"),
            (json!(-1), "1969-12-31T23:59:59.999Z"),
            // What a number has below a millisecond is dropped.
            (json!(1.9), "1970-01-01T00:00:00.001Z"),
            (json!(-1.9), "1969-12-31T23:59:59.999Z"),
            // A leap day, and the day after 28 February in a year without one.
            (json!(951_782_400_000_i64), "2000-02-29T00:00:00.000Z"),
            (json!(4_107_542_400_000_i64), "2100-03-01T00:00:00.000Z"),
            // A year of five digits, and one before year 0, as Go writes them.
            (json!(253_402_300_800_000_i64), "10000-01-01T00:00:00.000Z"),
            (json!(-62_167_219_200_001_i64), "-0001-12-31T23:59:59.999Z"),
            // Not a number, so zero.
            (json!("1752900000000"), "1970-01-01T00:00:00.000Z"),
            (Value::Null, "1970-01-01T00:00:00.000Z"),
        ] {
            assert_eq!(iso(&time), written, "{time}");
        }
    }

    #[test]
    fn an_id_is_quoted_as_go_quotes_it() {
        assert_eq!(quoted("ses_1"), "\"ses_1\"");
        assert_eq!(
            quoted("a\"b\\c\n\t\u{1}\u{7f}\u{85}\u{e9}"),
            "\"a\\\"b\\\\c\\n\\t\\x01\\x7f\\u0085\u{e9}\""
        );
    }

    /// The store is followed where opencode itself would keep it.
    #[test]
    fn the_store_follows_the_data_directory_and_opencode_db() {
        let home = env::temp_dir().join("someone");
        let data = home.join(".local").join("share").join("opencode");
        let set = |text: &str| Some(OsString::from(text));
        assert_eq!(store_from(&home, None, None), data.join("opencode.db"));
        assert_eq!(
            store_from(&home, set(""), set("")),
            data.join("opencode.db")
        );
        assert_eq!(
            store_from(&home, None, set("opencode-dev.db")),
            data.join("opencode-dev.db")
        );
        let elsewhere = home.join("elsewhere.db");
        assert_eq!(
            store_from(&home, None, Some(elsewhere.clone().into_os_string())),
            elsewhere
        );
        assert_eq!(
            store_from(&home, None, set(":memory:")),
            data.join("opencode.db")
        );
        let moved = home.join("data");
        assert_eq!(
            store_from(&home, Some(moved.clone().into_os_string()), set("dev.db")),
            moved.join("opencode").join("dev.db")
        );
    }

    /// A sub-agent's task is a session of its own, started after the one
    /// that started it. It is not the person's latest session.
    #[test]
    fn the_latest_session_is_the_persons_own() {
        let path = database(
            "latest",
            "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('main', NULL, 'Rename the handler', '/work/project', 1),
                 ('task', 'main', 'List the call sites', '/work/project', 2),
                 ('other', NULL, 'Another project', '/work/other', 3);",
        );
        let found = latest_in(&path, Path::new("/work/project")).expect("a latest session");
        assert_eq!(found.harness, "opencode");
        assert_eq!(session_id(&found), "main");
        assert_eq!(found.recency, 1_000_000);
        assert_eq!(found.load().unwrap().session["id"], "main");
        // It is the session of that directory, not of one inside it.
        assert!(latest_in(&path, Path::new("/work/project/src")).is_none());
        assert!(latest_in(&path.with_extension("gone"), Path::new("/work/project")).is_none());

        // The newest anywhere is whichever started last.
        let elsewhere = anywhere_in(&path).expect("a newest session");
        assert_eq!(elsewhere.harness, "opencode");
        assert_eq!(elsewhere.dir, "/work/other");
        assert_eq!(elsewhere.title, "Another project");
        assert_eq!(elsewhere.recency, 3_000_000);
        assert_eq!(elsewhere.file, None);
        assert!(anywhere_in(&path.with_extension("gone")).is_none());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// A long-running session is as recent as its last activity.
    #[test]
    fn the_latest_session_is_the_one_last_active() {
        let path = database(
            "active",
            "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER);
             INSERT INTO session VALUES ('long-running', NULL, 'Started first, still going', '/work/project', 1000, 9000),
                 ('quick', NULL, 'Started later, finished', '/work/project', 5000, 6000);",
        );
        let found = latest_in(&path, Path::new("/work/project")).expect("a latest session");
        assert_eq!(session_id(&found), "long-running");
        assert_eq!(found.recency, 9_000_000_000);
        // Elsewhere is told by when a session started.
        let elsewhere = anywhere_in(&path).expect("a newest session");
        assert_eq!(elsewhere.title, "Started later, finished");
        assert_eq!(elsewhere.recency, 5_000_000_000);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// A listing asks each directory for no more than it will show.
    #[test]
    fn each_directory_gives_at_most_the_limit() {
        let path = database(
            "limit",
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_created INTEGER);
             INSERT INTO session VALUES ('a1', 'A1', '/work/a', 1), ('a2', 'A2', '/work/a', 2),
                 ('a3', 'A3', '/work/a', 3), ('b1', 'B1', '/work', 4), ('b2', 'B2', '/work', 5);",
        );
        let dirs = [PathBuf::from("/work/a"), PathBuf::from("/work")];
        let ids = |id: &str, limit: usize| {
            let found = candidates(&path, &dirs, id, limit).unwrap();
            let ids: Vec<&str> = found
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect();
            ids.join(",")
        };
        assert_eq!(ids("", 30), "a3,a2,a1,b2,b1");
        assert_eq!(ids("", 2), "a3,a2,b2,b1");
        assert_eq!(ids("", 1), "a3,b2");
        assert_eq!(ids("b1", 1), "b1");
        assert_eq!(ids("nobody", 30), "");
        let named = candidates(&path, &dirs, "b1", 1).unwrap();
        assert_eq!(named[0].dir, Path::new("/work"));
        assert_eq!(named[0].title, "B1");
        assert_eq!(named[0].started, Some((0, 4_000_000)));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// A database that is not opencode's has no latest session and none
    /// anywhere: neither question has a place for an error.
    #[test]
    fn a_database_that_cannot_be_read_has_no_latest_session() {
        let path = database("unread", "CREATE TABLE incompatible (id TEXT)");
        assert!(latest_in(&path, Path::new("/work/project")).is_none());
        assert!(anywhere_in(&path).is_none());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
