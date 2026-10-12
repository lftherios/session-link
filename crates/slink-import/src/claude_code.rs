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

use serde_json::{Map, Value, json};

use crate::agents::adopt_agents;
use crate::normalize::{anthropic_content, anthropic_usage};
use crate::value::{field, or_null, put, text, truthy};
use crate::{Agent, Input};

const LABEL: &str = "session-import@0.1.0";

pub(crate) fn import(input: &Input) -> Option<Value> {
    let (mut run, calls) = transcript(&input.lines, &input.fallback, input.sub_agent)?;
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
fn transcript(
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
        return transcript(lines, session_name, true);
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
