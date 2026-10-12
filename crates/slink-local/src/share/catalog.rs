//! What a session offers for an excerpt: each piece of text in it, in the
//! order it happened, with where it came from and which prompt it answers.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use slink_format::{encode, encode_pretty};

use super::export::definition_blocks;

/// One piece of a session that can be picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unit {
    pub id: String,
    pub span_id: String,
    pub role: String,
    pub kind: String,
    pub text: String,
    /// The prompt this answers, as the units it was made of.
    pub prompt_ids: Vec<String>,
    /// Shown so that a gap is visible, and never exported: an image, an
    /// attachment, reasoning the capture does not hold.
    pub unavailable: bool,
    /// Part of the prompt is one of those.
    pub prompt_incomplete: bool,
    /// Only used here, to notice a tool result picked without its call.
    /// Neither is ever copied to an export.
    pub(crate) call_id: String,
    pub(crate) scope: String,
}

impl Unit {
    /// The unit as the compose page is sent it.
    pub fn to_json(&self) -> Value {
        let mut unit = Map::new();
        unit.insert("id".into(), json!(self.id));
        unit.insert("span_id".into(), json!(self.span_id));
        unit.insert("role".into(), json!(self.role));
        unit.insert("kind".into(), json!(self.kind));
        unit.insert("text".into(), json!(self.text));
        if !self.prompt_ids.is_empty() {
            unit.insert("prompt_ids".into(), json!(self.prompt_ids));
        }
        if self.unavailable {
            unit.insert("unavailable".into(), json!(true));
        }
        if self.prompt_incomplete {
            unit.insert("prompt_incomplete".into(), json!(true));
        }
        Value::Object(unit)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub units: Vec<Unit>,
}

impl Catalog {
    pub fn to_json(&self) -> Value {
        json!({"units": self.units.iter().map(Unit::to_json).collect::<Vec<_>>()})
    }
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn list<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    value
        .get(name)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn present(value: &Value, name: &str) -> bool {
    value.get(name).is_some_and(|found| !found.is_null())
}

/// A value as a person reads it: text as it is, anything else as indented
/// JSON.
fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => encode_pretty(other),
    }
}

fn role(message: &Value) -> &str {
    match text(message, "role") {
        known @ ("user" | "assistant" | "tool" | "system" | "developer") => known,
        _ => "other",
    }
}

/// Text a harness put into a user message: environment and instruction
/// blocks, reminders, background-task notices and records of local slash
/// commands. Keep in step with `HARNESS_WRAPPER` in
/// `packages/viewer/session-model.ts`.
static HARNESS_WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[\t\n\f\r ]*<(?:environment_context|system|instructions|user_instructions|AGENTS|task-notification|local-command-|command-name|command-message|command-args)")
        .expect("the pattern compiles")
});

/// The role as the viewer reads it. A user message made only of tool results
/// is agent activity, and one made only of harness wrappers is provided
/// context, so neither replaces the human input a response is linked to.
fn reading_role(message: &Value) -> &str {
    let role = role(message);
    let content = list(message, "content");
    if role != "user" || content.is_empty() {
        return role;
    }
    let results = content.iter().all(|part| part["type"] == "tool_result");
    let wrapped = content
        .iter()
        .all(|part| part["type"] == "text" && HARNESS_WRAPPER.is_match(text(part, "text")));
    if results {
        "tool"
    } else if wrapped {
        "system"
    } else {
        role
    }
}

/// What is being read while the catalog is built, across spans.
#[derive(Default)]
struct Builder {
    units: Vec<Unit>,
    /// For each agent, the prompt its next output answers.
    prompts: HashMap<String, Vec<String>>,
    prompt_gaps: HashMap<String, bool>,
    seen_tools: HashSet<String>,
    legacy_codex: bool,
}

/// The span being read.
struct Place<'a> {
    span: &'a str,
    scope: &'a str,
}

impl Builder {
    /// Lists a piece that can be picked.
    fn add(&mut self, at: &Place, id: String, role: &str, kind: &str, text: &str, call: &str) {
        self.push(at, id, [role, kind, text, call], false);
    }

    /// Lists a piece that is shown and can never be picked.
    fn gap(&mut self, at: &Place, id: String, role: &str, kind: &str, text: &str) {
        self.push(at, id, [role, kind, text, ""], true);
    }

    fn push(
        &mut self,
        at: &Place,
        id: String,
        [role, kind, text, call]: [&str; 4],
        unavailable: bool,
    ) {
        if text.is_empty() && !unavailable {
            return;
        }
        // A call recorded both in a message and as a span is one call.
        if !call.is_empty()
            && kind == "tool_call"
            && !self
                .seen_tools
                .insert(encode(&json!([at.scope, kind, call, text])))
        {
            return;
        }
        let mut unit = Unit {
            id,
            span_id: at.span.to_owned(),
            role: role.to_owned(),
            kind: kind.to_owned(),
            text: text.to_owned(),
            call_id: call.to_owned(),
            scope: at.scope.to_owned(),
            unavailable,
            ..Unit::default()
        };
        if role != "user" {
            unit.prompt_ids = self.prompts.get(at.scope).cloned().unwrap_or_default();
            unit.prompt_incomplete = self.prompt_gaps.get(at.scope).copied().unwrap_or(false);
        }
        self.units.push(unit);
    }

    fn result_parts(&mut self, at: &Place, items: &[Value], base: &str, call: &str) {
        if !call.is_empty()
            && !self
                .seen_tools
                .insert(encode(&json!([at.scope, "tool_result", call, items])))
        {
            return;
        }
        self.parts(at, items, base, "tool", "tool_result", call);
    }

    fn parts(
        &mut self,
        at: &Place,
        items: &[Value],
        base: &str,
        role: &str,
        kind: &str,
        call: &str,
    ) {
        for (index, part) in items.iter().enumerate() {
            let id = format!("{base}-{index}");
            match part["type"].as_str() {
                Some("text") => {
                    let said = text(part, "text");
                    self.add(at, id.clone(), role, kind, said, call);
                    if role != "user" && kind == "text" {
                        for (number, definition) in definition_blocks(said).iter().enumerate() {
                            self.add(at, format!("{id}-ref-{number}"), role, "source_reference", definition, "");
                        }
                    }
                }
                Some("thinking") => {
                    let thought = text(part, "text");
                    let unavailable = part["unavailable"] == true
                        || thought.trim().is_empty()
                        || (self.legacy_codex && thought.trim() == "[reasoning]");
                    let shown = if unavailable {
                        "Reasoning text isn't available in this capture."
                    } else {
                        thought
                    };
                    if unavailable {
                        self.gap(at, id, role, "thinking", shown);
                    } else {
                        self.add(at, id, role, "thinking", shown, "");
                    }
                }
                Some("tool_call") => {
                    let shown = display(&json!({"name": part["name"], "arguments": part["arguments"]}));
                    self.add(at, id, "assistant", "tool_call", &shown, text(part, "id"));
                }
                Some("tool_result") => {
                    self.result_parts(at, list(part, "content"), &id, text(part, "tool_call_id"));
                }
                Some("data") => self.add(at, id, role, "data", &display(&part["data"]), ""),
                _ => self.gap(
                    at,
                    id,
                    role,
                    "unavailable",
                    "This image, attachment, or unrecognized content cannot be included in a text excerpt yet.",
                ),
            }
        }
    }
}

/// Lists a session's pieces. A history that is replayed whole in each call
/// is listed once, for each agent by itself; a harness that records only
/// what is new each turn keeps every turn, a repeated prompt included. Tool
/// evidence is read both from messages and from tool spans of their own.
pub fn build_catalog(run: &Value) -> Catalog {
    let spans = list(run, "spans");
    let by_id: HashMap<&str, &Value> = spans.iter().map(|span| (text(span, "id"), span)).collect();
    // The agent a span works for: its nearest ancestor that is one.
    let scope_of = |span: &Value| -> String {
        let mut parent = text(span, "parent_id");
        let mut seen = HashSet::new();
        while !parent.is_empty() && seen.insert(parent) {
            let Some(found) = by_id.get(parent) else {
                break;
            };
            if found["type"] == "agent" {
                return parent.to_owned();
            }
            parent = text(found, "parent_id");
        }
        String::new()
    };
    let source = &run["source"];
    let delta = source["kind"] == "import"
        || (source["kind"] == "sdk" && text(source, "label").starts_with("pi-extension@"));
    let mut builder = Builder {
        legacy_codex: source["kind"] == "import" && source["harness"] == "codex",
        ..Builder::default()
    };
    let mut history: HashMap<String, Vec<String>> = HashMap::new();
    let mut systems: HashMap<String, String> = HashMap::new();
    for (index, span) in spans.iter().enumerate() {
        let scope = scope_of(span);
        let at = Place {
            span: text(span, "id"),
            scope: &scope,
        };
        let (input, output) = (&span["input"], &span["output"]);
        let system = text(input, "system");
        if !system.is_empty() && systems.get(&scope).map(String::as_str) != Some(system) {
            builder.add(
                &at,
                format!("u{index}-system"),
                "system",
                "text",
                system,
                "",
            );
            systems.insert(scope.clone(), system.to_owned());
        }
        for field in ["system_ref", "messages_ref", "tools_ref"] {
            if present(input, field) {
                builder.gap(
                    &at,
                    format!("u{index}-{field}"),
                    "system",
                    "unavailable",
                    "Additional context is stored outside this document and is unavailable for selection.",
                );
            }
        }
        let (given, made) = (list(input, "messages"), list(output, "messages"));
        let mut fingerprints: Vec<String> = given.iter().map(encode).collect();
        // A call that begins with everything said before it is a replay.
        let before = history.get(&scope).map_or(&[][..], Vec::as_slice);
        let replayed = !delta
            && !before.is_empty()
            && given.len() >= before.len()
            && before == &fingerprints[..before.len()];
        let skip = if replayed { before.len() } else { 0 };
        for (side, messages, skip) in [("in", given, skip), ("out", made, 0)] {
            for (number, message) in messages.iter().enumerate().skip(skip) {
                let role = reading_role(message);
                let first = builder.units.len();
                builder.parts(
                    &at,
                    list(message, "content"),
                    &format!("u{index}-{side}-{number}"),
                    role,
                    "text",
                    "",
                );
                if role == "user" {
                    let (whole, gaps): (Vec<&Unit>, Vec<&Unit>) = builder.units[first..]
                        .iter()
                        .partition(|unit| !unit.unavailable);
                    let prompt = whole.iter().map(|unit| unit.id.clone()).collect();
                    let gap = !gaps.is_empty();
                    builder.prompts.insert(scope.clone(), prompt);
                    builder.prompt_gaps.insert(scope.clone(), gap);
                }
            }
        }
        if !given.is_empty() || !made.is_empty() {
            fingerprints.extend(made.iter().map(encode));
            history.insert(scope.clone(), fingerprints);
        }
        if span["type"] == "tool_call" {
            let call = text(input, "tool_call_id");
            if present(input, "arguments") {
                let name = match text(input, "name") {
                    "" => text(span, "name"),
                    name => name,
                };
                let shown = display(&json!({"name": name, "arguments": input["arguments"]}));
                builder.add(
                    &at,
                    format!("u{index}-call"),
                    "assistant",
                    "tool_call",
                    &shown,
                    call,
                );
            }
            if present(output, "result") {
                let base = format!("u{index}-result");
                match output["result"].as_array() {
                    Some(parts) if parts.first().is_some_and(|part| present(part, "type")) => {
                        builder.result_parts(&at, parts, &base, call);
                    }
                    _ => {
                        let shown = json!([{"type": "text", "text": display(&output["result"])}]);
                        builder.result_parts(&at, shown.as_array().expect("a list"), &base, call);
                    }
                }
            }
        }
        if span["status"] == "error" {
            let message = text(&span["error"], "message");
            if !message.is_empty() {
                builder.add(
                    &at,
                    format!("u{index}-error"),
                    "system",
                    "error",
                    message,
                    "",
                );
            }
        }
    }
    Catalog {
        units: builder.units,
    }
}
