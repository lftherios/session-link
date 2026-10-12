//! A draft of an excerpt, and the document made from it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::catalog::{Catalog, Unit};
use super::{EXTENSION, lower};

/// One piece picked for an excerpt: a whole unit, or a passage of it by its
/// place in UTF-16 code units, which is how a browser counts a selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub id: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

/// What a person is composing: a title, a note of their own, the pieces, and
/// the one a reader arrives at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub note: String,
    pub items: Vec<Selection>,
    pub primary: String,
}

impl Draft {
    pub fn to_json(&self) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|item| {
                let mut selection = Map::new();
                selection.insert("id".into(), json!(item.id));
                if let Some(start) = item.start {
                    selection.insert("start".into(), json!(start));
                }
                if let Some(end) = item.end {
                    selection.insert("end".into(), json!(end));
                }
                Value::Object(selection)
            })
            .collect();
        json!({"title": self.title, "note": self.note, "items": items, "primary": self.primary})
    }

    /// Reads a draft a page sent. A field this code does not know, or one of
    /// the wrong type, refuses the draft: nothing unexamined is stored.
    pub fn from_json(value: &Value) -> Result<Draft, String> {
        let fields = object(value, &["title", "note", "items", "primary"])?;
        let text = |name: &str| match fields.get(name) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(text)) => Ok(text.clone()),
            Some(_) => Err(format!("{name} must be text")),
        };
        let mut items = Vec::new();
        match fields.get("items") {
            None | Some(Value::Null) => {}
            Some(Value::Array(list)) => {
                for item in list {
                    let fields = object(item, &["id", "start", "end"])?;
                    let place = |name: &str| match fields.get(name) {
                        None | Some(Value::Null) => Ok(None),
                        Some(number) => number
                            .as_i64()
                            .map(Some)
                            .ok_or(format!("{name} must be a whole number")),
                    };
                    items.push(Selection {
                        id: match fields.get("id") {
                            None | Some(Value::Null) => String::new(),
                            Some(Value::String(id)) => id.clone(),
                            Some(_) => return Err("id must be text".into()),
                        },
                        start: place("start")?,
                        end: place("end")?,
                    });
                }
            }
            Some(_) => return Err("items must be a list".into()),
        }
        Ok(Draft {
            title: text("title")?,
            note: text("note")?,
            items,
            primary: text("primary")?,
        })
    }

    /// Whether the draft can be saved, and when `ready`, turned into an
    /// excerpt.
    pub fn validate(&self, catalog: &Catalog, ready: bool) -> Result<(), String> {
        if self.title.chars().count() > 256 {
            return Err("keep the title within 256 characters".into());
        }
        if self.note.chars().count() > 10_000 {
            return Err("keep the author note within 10,000 characters".into());
        }
        if self.items.len() > 1000 {
            return Err("select at most 1,000 pieces of content".into());
        }
        if ready && self.items.is_empty() {
            return Err("select something to include in the excerpt".into());
        }
        if ready && self.title.trim().is_empty() {
            return Err("give the excerpt a title".into());
        }
        let units: HashMap<&str, &Unit> = catalog
            .units
            .iter()
            .map(|unit| (unit.id.as_str(), unit))
            .collect();
        let mut seen = HashSet::new();
        for item in &self.items {
            let unit = units
                .get(item.id.as_str())
                .filter(|unit| !unit.unavailable)
                .ok_or_else(|| {
                    format!(
                        "selection {:?} is unavailable; reload the source session",
                        item.id
                    )
                })?;
            if !seen.insert(item.id.as_str()) {
                return Err(format!("selection {:?} appears more than once", item.id));
            }
            selected_text(&unit.text, item)?;
        }
        if !self.primary.is_empty() && !seen.contains(self.primary.as_str()) {
            return Err("the starting point must be included in the excerpt".into());
        }
        Ok(())
    }
}

/// An object with no field but the ones named.
pub(crate) fn object<'a>(
    value: &'a Value,
    known: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let fields = value.as_object().ok_or("expected an object")?;
    match fields.keys().find(|key| !known.contains(&key.as_str())) {
        Some(unknown) => Err(format!("unknown field {unknown:?}")),
        None => Ok(fields),
    }
}

/// The text a selection picks, and whether it is a passage and not the whole.
/// A browser's offsets count UTF-16 code units, and one that falls inside a
/// surrogate pair is refused: an emoji must never silently become a
/// replacement character.
pub(crate) fn selected_text(text: &str, item: &Selection) -> Result<(String, bool), String> {
    let (start, end) = match (item.start, item.end) {
        (None, None) => return Ok((text.to_owned(), false)),
        (Some(start), Some(end)) => (start, end),
        _ => return Err("a passage needs both a start and an end".into()),
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let length = units.len() as i64;
    let boundary =
        |at: i64| at == 0 || at == length || !(0xdc00..=0xdfff).contains(&units[at as usize]);
    if start < 0 || end <= start || end > length || !boundary(start) || !boundary(end) {
        return Err("the selected passage has invalid text boundaries".into());
    }
    let passage = String::from_utf16_lossy(&units[start as usize..end as usize]);
    Ok((passage, start != 0 || end != length))
}

// Reference-style Markdown needs its definitions. A definition is never
// pulled in from text that was left out: the sender includes it or does not.
static REF_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!?\[([^\]\n]+)\]\[([^\]\n]*)\]|\[\^([^\]\n]+)\]").expect("the pattern compiles")
});
static REF_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^ {0,3}\[([^\]\n]+)\]:[ \t]*[^\t\n\f\r ]").expect("the pattern compiles")
});
static INLINE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`+[^`\n]*`+").expect("the pattern compiles"));
static SHORTCUT_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]").expect("the pattern compiles"));

/// The fence a line opens or closes, if it is one.
fn fence_mark(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// The reference definitions in some text, each with its continuation lines,
/// as they were written. Definitions inside a code block are examples, not
/// sources, and are left where they are.
pub(crate) fn definition_blocks(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut blocks = Vec::new();
    let mut fence = "";
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(mark) = fence_mark(line) {
            if fence.is_empty() {
                fence = mark;
            } else if fence == mark {
                fence = "";
            }
            index += 1;
            continue;
        }
        if !fence.is_empty() || !REF_DEFINITION.is_match(line) {
            index += 1;
            continue;
        }
        let start = index;
        while index + 1 < lines.len() {
            let next = lines[index + 1];
            let continues = !next.trim().is_empty()
                && (next.starts_with("  ") || next.starts_with('\t'))
                && !REF_DEFINITION.is_match(next);
            if !continues {
                break;
            }
            index += 1;
        }
        blocks.push(lines[start..=index].join("\n"));
        index += 1;
    }
    blocks
}

/// The text without its code: fenced blocks and inline spans.
fn prose_only(text: &str) -> String {
    let mut out = String::new();
    let mut fence = "";
    for line in text.split('\n') {
        if let Some(mark) = fence_mark(line) {
            if fence.is_empty() {
                fence = mark;
            } else if fence == mark {
                fence = "";
            }
            continue;
        }
        if fence.is_empty() {
            out.push_str(&INLINE_CODE.replace_all(line, ""));
            out.push('\n');
        }
    }
    out
}

/// The labels some texts define.
fn definitions<'a>(texts: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    let mut labels = HashSet::new();
    for text in texts {
        for block in definition_blocks(text) {
            if let Some(found) = REF_DEFINITION.captures(&block) {
                labels.insert(lower(found[1].trim()));
            }
        }
    }
    labels
}

/// The labels some texts refer to and do not define. A bare `[label]` only
/// counts when `known` says the source defines it somewhere.
pub(crate) fn missing_reference_labels(texts: &[&str], known: &HashSet<String>) -> HashSet<String> {
    let defined = definitions(texts.iter().copied());
    let mut missing = HashSet::new();
    for raw in texts {
        let prose = prose_only(raw);
        for found in REF_LINK.captures_iter(&prose) {
            let group = |index: usize| found.get(index).map_or("", |group| group.as_str());
            let label = if !group(3).is_empty() {
                format!("^{}", group(3))
            } else if group(2).is_empty() {
                group(1).to_owned()
            } else {
                group(2).to_owned()
            };
            let label = lower(label.trim());
            if !defined.contains(&label) {
                missing.insert(label);
            }
        }
        for found in SHORTCUT_REF.captures_iter(&prose) {
            let whole = found.get(0).expect("the match");
            if matches!(prose.as_bytes().get(whole.end()), Some(b'(' | b':' | b'[')) {
                continue;
            }
            let label = lower(found[1].trim());
            if known.contains(&label) && !defined.contains(&label) {
                missing.insert(label);
            }
        }
    }
    missing
}

#[cfg(test)]
pub(crate) fn missing_references(texts: &[&str]) -> bool {
    !missing_reference_labels(texts, &HashSet::new()).is_empty()
}

/// Builds the excerpt: a new document holding the pieces the draft picks, in
/// the order they happened, and a record of what each is and what was left
/// out around it.
pub fn export(run: &Value, catalog: &Catalog, draft: &Draft) -> Result<Value, String> {
    draft.validate(catalog, true)?;
    let items: HashMap<&str, &Selection> = draft
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let units: HashMap<&str, &Unit> = catalog
        .units
        .iter()
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    let message = |role: &str, text: &str| json!({"messages": [{"role": role, "content": [{"type": "text", "text": text}]}]});
    let prose =
        |unit: &Unit| matches!(unit.kind.as_str(), "text" | "source_reference" | "thinking");

    let mut spans =
        vec![json!({"id": "root", "type": "agent", "name": "Selected session material"})];
    let mut envelope = Map::new();
    envelope.insert("kind".into(), json!("excerpt"));
    envelope.insert("omissions".into(), json!(true));
    let fidelity = run["source"]["fidelity"].as_str().unwrap_or_default();
    if matches!(fidelity, "exact" | "reconstructed" | "partial") {
        envelope.insert("original_fidelity".into(), json!(fidelity));
    }
    if !draft.note.is_empty() {
        spans.push(json!({
            "id": "author-note",
            "parent_id": "root",
            "type": "custom",
            "name": "Author context",
            "input": message("author", &draft.note),
        }));
        envelope.insert("note_id".into(), json!("author-note"));
    }
    let mut records: Vec<Value> = Vec::new();
    let mut texts: Vec<String> = vec![draft.note.clone()];
    let mut last: Option<usize> = None;
    for (position, unit) in catalog.units.iter().enumerate() {
        let Some(item) = items.get(unit.id.as_str()) else {
            continue;
        };
        let (text, clipped) = selected_text(&unit.text, item)?;
        if prose(unit) {
            texts.push(text.clone());
        }
        let id = match (item.start, item.end) {
            (Some(start), Some(end)) => format!("{}-{start}-{end}", unit.id),
            _ => unit.id.clone(),
        };
        let label = match unit.kind.as_str() {
            "tool_call" => "Tool arguments",
            "tool_result" => "Tool result",
            "error" => "Recorded error",
            "thinking" => "Recorded reasoning",
            "data" => "Recorded data",
            "source_reference" => "Source reference",
            _ => unit.role.as_str(),
        };
        spans.push(json!({
            "id": id,
            "parent_id": "root",
            "type": "custom",
            "name": label,
            "input": message(&unit.role, &text),
        }));
        // Something between this piece and the one before it was left out,
        // unless all of it was reference cards.
        let from = last.map_or(0, |last| last + 1);
        let gap = catalog.units[from..position]
            .iter()
            .any(|skipped| skipped.kind != "source_reference");
        let mut record = Map::new();
        record.insert("id".into(), json!(id));
        record.insert("kind".into(), json!(unit.kind));
        record.insert("role".into(), json!(unit.role));
        record.insert("passage".into(), json!(clipped));
        record.insert("omitted_before".into(), json!(gap));
        if unit.kind == "error" || unit.role == "assistant" || unit.role == "tool" {
            let whole_prompt = !unit.prompt_ids.is_empty()
                && !unit.prompt_incomplete
                && unit.prompt_ids.iter().all(|prompt| {
                    match (items.get(prompt.as_str()), units.get(prompt.as_str())) {
                        (Some(picked), Some(asked)) => {
                            selected_text(&asked.text, picked).is_ok_and(|(_, clipped)| !clipped)
                        }
                        _ => false,
                    }
                });
            if !whole_prompt {
                record.insert("prompt_missing".into(), json!(true));
            }
        }
        if unit.kind == "tool_result" {
            let with_call = catalog.units.iter().any(|other| {
                items.contains_key(other.id.as_str())
                    && !unit.call_id.is_empty()
                    && other.scope == unit.scope
                    && other.call_id == unit.call_id
                    && other.kind == "tool_call"
            });
            if !with_call {
                record.insert("tool_call_missing".into(), json!(true));
            }
        }
        records.push(Value::Object(record));
        last = Some(position);
        if unit.id == draft.primary {
            envelope.insert("primary_id".into(), json!(id));
        }
    }
    let source_texts = catalog
        .units
        .iter()
        .filter(|unit| prose(unit))
        .map(|unit| unit.text.as_str());
    let known = definitions(source_texts);
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    for label in missing_reference_labels(&texts, &known) {
        if known.contains(&label) {
            return Err("a selected passage uses a source reference whose definition is omitted; include its Source reference card before previewing".into());
        }
        envelope.insert("references_unavailable".into(), json!(true));
    }
    if !envelope.contains_key("primary_id") {
        envelope.insert("primary_id".into(), records[0]["id"].clone());
    }
    envelope.insert("items".into(), Value::Array(records));
    let mut extensions = Map::new();
    extensions.insert(EXTENSION.into(), Value::Object(envelope));
    let out = json!({
        "schema": "session/v0",
        "created_at": run.get("created_at").cloned().unwrap_or(Value::Null),
        "name": draft.title,
        "source": {"kind": "import", "label": "session.link excerpt", "fidelity": "partial"},
        "extensions": extensions,
        "spans": spans,
    });
    let issues = slink_format::validate(&out);
    if !issues.is_empty() {
        return Err(format!("cannot create excerpt: {}", issues.join("; ")));
    }
    Ok(out)
}
