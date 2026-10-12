//! Sub-agents. A harness keeps a sub-agent's work apart from the session
//! that started it; this nests each one back in, as a child agent under the
//! tool call that started it, so the session reads in the order it ran.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::Agent;
use crate::value::text;

/// Nests each agent's session in `run`. The harness names the call that
/// started an agent itself, or the importer learned it while reading the
/// parent (`calls`, by the agent's id). An agent with neither goes under the
/// tool call that was running when it started, and failing that under the
/// session.
pub(crate) fn adopt_agents(
    run: &mut Value,
    agents: &[Agent],
    calls: &HashMap<String, String>,
    import: impl Fn(&Agent) -> Option<Value>,
) {
    let own: Vec<Value> = run["spans"].as_array().cloned().unwrap_or_default();
    // tool call id -> the span of that call
    let mut call_span: HashMap<String, String> = HashMap::new();
    let note = |call_span: &mut HashMap<String, String>, spans: &[Value]| {
        for span in spans {
            let id = text(&span["input"], "tool_call_id");
            if text(span, "type") == "tool_call" && !id.is_empty() {
                call_span
                    .entry(id.to_owned())
                    .or_insert_with(|| text(span, "id").to_owned());
            }
        }
    };
    note(&mut call_span, &own);
    // span id -> the agents that span started
    let mut under: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
    let mut adopted = 0_u64;
    for agent in agents {
        let Some(child) = import(agent) else {
            continue;
        };
        let child_spans = child["spans"].as_array().cloned().unwrap_or_default();
        // Nothing was recorded for it beyond its start.
        let Some((child_root, rest)) = child_spans
            .split_first()
            .filter(|(_, rest)| !rest.is_empty())
        else {
            continue;
        };
        adopted += 1;
        let prefix = format!("a{adopted}");
        let call = if agent.call.is_empty() {
            calls.get(&agent.id).map_or("", String::as_str)
        } else {
            &agent.call
        };
        let mut anchor = call_span.get(call).cloned().unwrap_or_default();
        if anchor.is_empty()
            && let Some(started) = slink_format::instant(text(child_root, "started_at"))
        {
            for span in &own {
                let from = slink_format::instant(text(span, "started_at"));
                let to = slink_format::instant(text(span, "ended_at"));
                if text(span, "type") == "tool_call"
                    && from.is_some_and(|from| started >= from)
                    && to.is_some_and(|to| started <= to)
                {
                    anchor = text(span, "id").to_owned();
                }
            }
        }
        if anchor.is_empty() {
            anchor = "root".into();
        }
        // Named by its harness, else after the call that started it.
        let mut name = agent.name.clone();
        if name.is_empty()
            && anchor != "root"
            && let Some(span) = own.iter().find(|span| text(span, "id") == anchor)
        {
            name = text(span, "name").to_owned();
        }
        if name.is_empty() {
            name = child_root["name"].as_str().unwrap_or(&agent.id).to_owned();
        }
        let mut head = Map::new();
        head.insert("id".into(), json!(prefix));
        head.insert("parent_id".into(), json!(anchor));
        head.insert("type".into(), json!("agent"));
        head.insert("name".into(), json!(name));
        for key in ["started_at", "ended_at", "status"] {
            if let Some(value) = child_root.get(key) {
                head.insert(key.into(), value.clone());
            }
        }
        let mut block = vec![Value::Object(head)];
        for span in rest {
            let mut span = span.clone();
            let parent = span["parent_id"].as_str().unwrap_or("root").to_owned();
            span["id"] = json!(format!("{prefix}.{}", text(&span, "id")));
            span["parent_id"] = if parent == "root" {
                json!(prefix)
            } else {
                json!(format!("{prefix}.{parent}"))
            };
            block.push(span);
        }
        note(&mut call_span, &block);
        under.entry(anchor).or_default().push(block);
    }
    if adopted == 0 {
        return;
    }
    let mut ordered: Vec<Value> = Vec::new();
    place(&mut ordered, &own, &under);
    for block in under.get("root").into_iter().flatten() {
        place(&mut ordered, block, &under);
    }
    run["spans"] = Value::Array(ordered);
    run["metadata"]["sub_agents"] = json!(adopted);
}

/// Each span, followed by the agents it started.
fn place(ordered: &mut Vec<Value>, spans: &[Value], under: &HashMap<String, Vec<Vec<Value>>>) {
    for span in spans {
        ordered.push(span.clone());
        let id = text(span, "id");
        if id != "root" {
            for block in under.get(id).into_iter().flatten() {
                place(ordered, block, under);
            }
        }
    }
}
