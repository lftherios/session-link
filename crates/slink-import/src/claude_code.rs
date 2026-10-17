//! Claude Code. A transcript under `~/.claude/projects` is one JSON entry on
//! each line. User and tool messages gather as what the next model call was
//! given; each model response becomes an `llm_call` with those messages as
//! its input, and each `tool_use` block a `tool_call` span that the matching
//! `tool_result` closes.
//!
//! Only what the person typed keeps the user role: tool results become tool
//! messages, and injected notices and compaction summaries become system
//! messages. An API error the harness wrote in the model's name, such as a
//! rate limit, becomes a failed custom span and not an agent response.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::discover::{
    Candidate, Elsewhere, Found, blobs_beside, newest_file, newest_of, peek, read_lines,
    sorted_entries, stem, transcripts_in, user_home,
};
use crate::normalize::{anthropic_content, anthropic_usage};
use crate::value::{field, or_null, put, text, truthy};
use crate::{Agent, Input, LABEL};

pub(crate) fn import(input: &Input) -> Option<Value> {
    let (mut run, calls) = to_run(&input.lines, &input.fallback, input.sub_agent)?;
    adopt_agents(&mut run, &input.agents, &calls, |agent: &Agent| {
        let mut input = agent.input.clone();
        input.sub_agent = true;
        import(&input)
    });
    Some(run)
}

/// The first thing the person asked, cut to the length of a title: the name
/// of a session its harness gave none.
pub(crate) fn first_user_text<'a>(entries: impl IntoIterator<Item = &'a Value>) -> String {
    for entry in entries {
        // Injected caveats, context and compaction summaries are not the ask.
        if text(entry, "type") != "user"
            || truthy(entry.get("isMeta"))
            || truthy(entry.get("isCompactSummary"))
        {
            continue;
        }
        let said = match &entry["message"]["content"] {
            Value::String(said) => said.as_str(),
            Value::Array(parts) => parts
                .iter()
                .find(|part| text(part, "type") == "text")
                .map_or("", |part| text(part, "text")),
            _ => "",
        };
        let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
        // Tool results and injected caveats are not a title either.
        if said.is_empty() || said.starts_with('<') || said.starts_with("Caveat:") {
            continue;
        }
        // Cut only against a pasted wall of text: a listing applies its own
        // tighter budget, and the viewer gets the whole thing.
        return if said.chars().count() > 200 {
            said.chars().take(199).chain(['…']).collect()
        } else {
            said
        };
    }
    String::new()
}

/// The role of a user entry's message.
fn user_role(entry: &Value, content: &[Value]) -> &'static str {
    let results = !content.is_empty()
        && content
            .iter()
            .all(|part| text(part, "type") == "tool_result");
    if results {
        "tool"
    } else if truthy(entry.get("isMeta")) || truthy(entry.get("isCompactSummary")) {
        "system"
    } else {
        "user"
    }
}

/// The text parts of some content, a blank line between them.
fn joined_text(content: &[Value], fallback: &str) -> String {
    let texts: Vec<&str> = content
        .iter()
        .filter(|part| text(part, "type") == "text" && !text(part, "text").is_empty())
        .map(|part| text(part, "text"))
        .collect();
    if texts.is_empty() {
        fallback.to_owned()
    } else {
        texts.join("\n\n")
    }
}

/// A model id as text, whatever JSON type the transcript gave it.
fn model_id(model: &Value) -> String {
    match model {
        Value::String(id) => id.clone(),
        other => slink_format::encode(other),
    }
}

/// The document for a transcript, and for each sub-agent the session started
/// the tool call that started it. A sub-agent's own transcript is all
/// sidechain entries, which a session's transcript does not replay;
/// `sub_agent` reads them.
fn to_run(
    lines: &[String],
    session_name: &str,
    sub_agent: bool,
) -> Option<(Value, HashMap<String, String>)> {
    let mut agent_calls: HashMap<String, String> = HashMap::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut skipped_sidechain = 0_u64;
    let mut name = Value::String(session_name.to_owned());
    for line in lines {
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        let kind = text(&entry, "type");
        if kind == "summary" && truthy(entry.get("summary")) {
            name = entry["summary"].clone();
        }
        // Claude Code records its own session title; the latest one names it.
        if kind == "ai-title" && truthy(entry.get("aiTitle")) {
            name = entry["aiTitle"].clone();
        }
        if kind != "user" && kind != "assistant" {
            continue;
        }
        if truthy(entry.get("isSidechain")) && !sub_agent {
            skipped_sidechain += 1;
            continue;
        }
        if !truthy(entry.get("message")) {
            continue;
        }
        entries.push(entry);
    }
    if entries.is_empty() && skipped_sidechain > 0 {
        // Nothing but sidechain entries: this is a sub-agent's own transcript.
        return to_run(lines, session_name, true);
    }
    let (first, last) = (entries.first()?, entries.last()?);
    // No title in the transcript? The first thing the person asked beats the
    // session's UUID everywhere a person sees the name.
    if name.as_str().unwrap_or_default() == session_name {
        let asked = first_user_text(&entries);
        if !asked.is_empty() {
            name = Value::String(asked);
        }
    }

    let mut root = Map::new();
    root.insert("id".into(), json!("root"));
    root.insert("parent_id".into(), Value::Null);
    root.insert("type".into(), json!("agent"));
    root.insert("name".into(), name.clone());
    root.insert("status".into(), json!("ok"));
    put(&mut root, "started_at", first, "timestamp");
    put(&mut root, "ended_at", last, "timestamp");
    let mut spans: Vec<Value> = vec![Value::Object(root)];

    let mut pending: Vec<Value> = Vec::new();
    // When the first message of what is pending was written.
    let mut pending_start: Option<Value> = None;
    let mut previous = or_null(first, "timestamp");
    // tool_use id -> where its span is in `spans`.
    let mut open_tools: HashMap<String, usize> = HashMap::new();
    let mut sequence = 0_u64;
    // The latest model call: where its span is, and its message id.
    let mut call: Option<(usize, String)> = None;

    for entry in &entries {
        let message = &entry["message"];
        let stamp = or_null(entry, "timestamp");
        if text(entry, "type") == "user" {
            let content = anthropic_content(message.get("content"));
            for part in &content {
                if text(part, "type") != "tool_result" {
                    continue;
                }
                let id = text(part, "tool_call_id");
                let Some(at) = open_tools.remove(id) else {
                    continue;
                };
                let result = field(entry, "toolUseResult").or_else(|| field(part, "content"));
                let mut output = Map::new();
                output.insert("result".into(), result.cloned().unwrap_or(Value::Null));
                let tool = &mut spans[at];
                tool["ended_at"] = stamp.clone();
                if truthy(part.get("is_error")) {
                    output.insert("is_error".into(), Value::Bool(true));
                    tool["status"] = json!("error");
                }
                tool["output"] = Value::Object(output);
                let agent = text(&entry["toolUseResult"], "agentId");
                if !agent.is_empty() {
                    agent_calls.insert(agent.to_owned(), id.to_owned());
                }
            }
            if pending_start.is_none() && !stamp.is_null() {
                pending_start = Some(stamp.clone());
            }
            pending.push(json!({"role": user_role(entry, &content), "content": content}));
        } else if truthy(entry.get("isApiErrorMessage")) {
            // No model produced this, so the text is kept as a failed span
            // and not as an llm_call with an agent response.
            sequence += 1;
            let content = anthropic_content(message.get("content"));
            let mut failure = Map::new();
            failure.insert(
                "message".into(),
                json!(joined_text(&content, "Claude Code reported an API error")),
            );
            let kind = text(entry, "error");
            if !kind.is_empty() {
                failure.insert("type".into(), json!(kind));
            }
            spans.push(json!({
                "id": format!("s{sequence}"),
                "parent_id": "root",
                "type": "custom",
                "name": "harness error",
                "started_at": previous,
                "ended_at": stamp,
                "status": "error",
                "error": failure,
                "input": {"messages": std::mem::take(&mut pending)},
            }));
            pending_start = None;
            call = None;
        } else {
            // Claude Code writes one entry for each content block of a
            // response, all with the response's message id and usage. They
            // are one model call: later blocks join the reply, and the last
            // usage stands.
            let id = text(message, "id");
            let content = anthropic_content(message.get("content"));
            let at = match &call {
                Some((at, current)) if !id.is_empty() && id == current => {
                    let span = &mut spans[*at];
                    if let Some(reply) = span["output"]["messages"][0]["content"].as_array_mut() {
                        reply.extend(content);
                    }
                    span["ended_at"] = stamp.clone();
                    *at
                }
                _ => {
                    sequence += 1;
                    let model =
                        field(message, "model").map_or_else(|| "unknown".to_owned(), model_id);
                    spans.push(json!({
                        "id": format!("s{sequence}"),
                        "parent_id": "root",
                        "type": "llm_call",
                        "name": "turn",
                        "started_at": previous,
                        "ended_at": stamp,
                        "status": "ok",
                        "model": {"id": model, "provider": "anthropic"},
                        "input": {"messages": std::mem::take(&mut pending)},
                        "output": {"messages": [{"role": "assistant", "content": content}]},
                    }));
                    pending_start = None;
                    call = Some((spans.len() - 1, id.to_owned()));
                    spans.len() - 1
                }
            };
            if let Some(usage) = anthropic_usage(message.get("usage")) {
                spans[at]["usage"] = usage;
            }
            let parent = spans[at]["id"].clone();
            for block in message["content"].as_array().into_iter().flatten() {
                if text(block, "type") != "tool_use" {
                    continue;
                }
                let Some(id) = block["id"].as_str() else {
                    continue;
                };
                sequence += 1;
                let mut input = Map::new();
                input.insert("tool_call_id".into(), json!(id));
                put(&mut input, "name", block, "name");
                put(&mut input, "arguments", block, "input");
                let mut tool = Map::new();
                tool.insert("id".into(), json!(format!("s{sequence}")));
                tool.insert("parent_id".into(), parent.clone());
                tool.insert("type".into(), json!("tool_call"));
                tool.insert("started_at".into(), stamp.clone());
                tool.insert("status".into(), json!("ok"));
                tool.insert("input".into(), Value::Object(input));
                put(&mut tool, "name", block, "name");
                spans.push(Value::Object(tool));
                open_tools.insert(id.to_owned(), spans.len() - 1);
            }
        }
        previous = stamp;
    }

    // A transcript can end on a user turn. Those messages are kept as a
    // custom span: an llm_call with no response would be an invention.
    if !pending.is_empty() {
        sequence += 1;
        spans.push(json!({
            "id": format!("s{sequence}"),
            "parent_id": "root",
            "type": "custom",
            "name": "trailing user message(s) — transcript ends before a reply",
            "started_at": pending_start.unwrap_or_else(|| previous.clone()),
            "ended_at": previous,
            "input": {"messages": pending},
        }));
    }

    let mut metadata = Map::new();
    put(&mut metadata, "session_id", first, "sessionId");
    put(&mut metadata, "cwd", first, "cwd");
    if truthy(first.get("version")) {
        metadata.insert("harness_version".into(), first["version"].clone());
    }
    if skipped_sidechain != 0 {
        metadata.insert("sidechain_entries_skipped".into(), json!(skipped_sidechain));
    }

    let mut run = Map::new();
    run.insert("schema".into(), json!("session/v0"));
    run.insert("name".into(), name);
    run.insert(
        "source".into(),
        json!({"kind": "import", "harness": "claude-code", "label": LABEL, "fidelity": "reconstructed"}),
    );
    run.insert("metadata".into(), Value::Object(metadata));
    put(&mut run, "created_at", first, "timestamp");
    run.insert("spans".into(), Value::Array(spans));
    Some((Value::Object(run), agent_calls))
}

/* ------------------------------------------------ where its sessions are */

/// Where Claude Code keeps one directory of transcripts for each project.
/// `CLAUDE_CONFIG_DIR` moves its whole configuration directory.
pub(crate) fn store() -> PathBuf {
    match env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("projects"),
        None => user_home().join(".claude").join("projects"),
    }
}

/// The longest directory name Claude Code derives from a path alone. A longer
/// one is cut here and ends in a hash of the whole path.
const SLUG_LIMIT: usize = 200;

/// A project directory's name as Claude Code writes it: every UTF-16 code
/// unit other than an ASCII letter or digit becomes `-`.
fn slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => byte as char,
            _ => '-',
        })
        .collect()
}

/// The transcripts Claude Code keeps under `base` for one working directory.
/// A long path is found by its cut name, since the hash that follows depends
/// on the runtime Claude Code was built for; projects can share that name,
/// so each transcript must also record `cwd` as its directory.
fn transcripts(base: &Path, cwd: &Path) -> Vec<PathBuf> {
    let slug = slug(cwd);
    if slug.len() <= SLUG_LIMIT {
        return transcripts_in(&base.join(slug));
    }
    let prefix = format!("{}-", &slug[..SLUG_LIMIT]);
    let mut files = Vec::new();
    for dir in sorted_entries(base) {
        let named = dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(&prefix));
        if !dir.is_dir() || !named {
            continue;
        }
        for file in transcripts_in(&dir) {
            if Path::new(&peek(&file, 40).cwd) == cwd {
                files.push(file);
            }
        }
    }
    files
}

/// The transcript of a pasted session id, for one working directory. Claude
/// Code names a transcript `<uuid>.jsonl`.
pub(crate) fn transcript(cwd: &Path, id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    transcripts(&store(), cwd).into_iter().find(|file| {
        file.file_name()
            .is_some_and(|found| found.to_string_lossy() == name)
    })
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    newest_of(transcripts(&store(), cwd), "claude-code")
}

pub(crate) fn anywhere() -> Option<Elsewhere> {
    let mut best: Option<(PathBuf, i64)> = None;
    for project in sorted_entries(&store()) {
        if project.is_dir()
            && let Some(found) = newest_file(&project)
            && best.as_ref().is_none_or(|best| found.1 > best.1)
        {
            best = Some(found);
        }
    }
    let (file, recency) = best?;
    let head = peek(&file, 40);
    Some(Elsewhere {
        harness: "claude-code",
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
    let mut found = Vec::new();
    for dir in dirs {
        for file in transcripts(store, dir) {
            let session = stem(&file);
            if id.is_empty() || session == id {
                found.extend(Candidate::file("claude-code", &file, dir, session));
            }
        }
    }
    Ok(found)
}

pub(crate) fn describe(candidate: &mut Candidate) {
    let related = candidate
        .file
        .as_deref()
        .map(agent_files)
        .unwrap_or_default();
    candidate.describe_file(related);
    candidate.title = if candidate.name.is_empty() {
        candidate.prompt.clone()
    } else {
        candidate.name.clone()
    };
}

/// A transcript's lines with the sessions of its sub-agents.
pub(crate) fn input(file: &Path, lines: Vec<String>) -> Input {
    Input {
        lines,
        fallback: stem(file),
        blobs: blobs_beside(file),
        agents: agents(file),
        ..Input::default()
    }
}

pub(crate) fn recognizes(head: &[Value]) -> bool {
    head.iter()
        .any(|entry| matches!(text(entry, "type"), "user" | "assistant"))
}

/// The directory Claude Code keeps a session's sub-agents in.
fn agents_dir(transcript: &Path) -> Option<PathBuf> {
    let session = transcript.to_str()?.strip_suffix(".jsonl")?;
    Some(Path::new(session).join("subagents"))
}

/// The transcripts of a session's sub-agents.
fn agent_files(transcript: &Path) -> Vec<PathBuf> {
    agents_dir(transcript)
        .map(|dir| transcripts_in(&dir))
        .unwrap_or_default()
}

/// A session's sub-agents. Claude Code writes each one's transcript to
/// `subagents/agent-<id>.jsonl`, in a directory named after the session,
/// with a `.meta.json` that names the tool call that started it. A
/// sub-agent's own sub-agents are in the same directory, one spawn deeper,
/// so the shallower ones come first.
fn agents(transcript: &Path) -> Vec<Agent> {
    let mut agents: Vec<(f64, Agent)> = Vec::new();
    for file in agent_files(transcript) {
        let Ok(lines) = read_lines(&file) else {
            continue;
        };
        let name = stem(&file);
        let id = name.strip_prefix("agent-").unwrap_or(&name).to_owned();
        let meta = file
            .to_str()
            .and_then(|path| path.strip_suffix(".jsonl"))
            .and_then(|path| fs::read_to_string(format!("{path}.meta.json")).ok())
            .and_then(|raw| slink_format::parse(&raw).ok())
            .unwrap_or(Value::Null);
        let described = match text(&meta, "description") {
            "" => text(&meta, "agentType"),
            description => description,
        };
        let agent = Agent {
            call: text(&meta, "toolUseId").to_owned(),
            name: described.to_owned(),
            input: Input {
                lines,
                fallback: id.clone(),
                ..Input::default()
            },
            id,
        };
        agents.push((meta["spawnDepth"].as_f64().unwrap_or(1.0), agent));
    }
    agents.sort_by(|a, b| a.0.total_cmp(&b.0));
    agents.into_iter().map(|(_, agent)| agent).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_named_as_claude_code_names_it() {
        assert_eq!(
            slug(Path::new("/work/dev/my project_1")),
            "-work-dev-my-project-1"
        );
        // One code unit each for a letter outside ASCII, two for an emoji.
        assert_eq!(slug(Path::new("/a/\u{e9}/\u{1f600}")), "-a-----");
    }
}
