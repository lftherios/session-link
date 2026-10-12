//! A provider's wire format as `session/v0` content parts. Unknown block
//! types pass through untouched, and a block that would make an invalid part
//! becomes a `data` part holding it whole.

use serde_json::{Map, Value, json};

use crate::value::{data_part, field, text};

/// Anthropic message content as parts, with an image recorded inline kept in
/// its part as a data URL: a transcript has no raw request to find it in.
pub(crate) fn anthropic_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(only)) => vec![json!({"type": "text", "text": only})],
        Some(Value::Array(blocks)) => blocks.iter().map(anthropic_part).collect(),
        Some(_) => Vec::new(),
    }
}

fn anthropic_part(block: &Value) -> Value {
    let Some(kind) = block.get("type").and_then(Value::as_str) else {
        return data_part(block);
    };
    match kind {
        "text" => json!({"type": "text", "text": text(block, "text")}),
        "thinking" => json!({"type": "thinking", "text": text(block, "thinking")}),
        "tool_use" => match (block["id"].as_str(), block["name"].as_str()) {
            (Some(id), Some(name)) => {
                json!({"type": "tool_call", "id": id, "name": name, "arguments": block["input"]})
            }
            _ => data_part(block),
        },
        "tool_result" => {
            let Some(id) = block["tool_use_id"].as_str() else {
                return data_part(block);
            };
            let mut part = Map::new();
            part.insert("type".into(), json!("tool_result"));
            part.insert("tool_call_id".into(), json!(id));
            part.insert(
                "content".into(),
                Value::Array(anthropic_content(block.get("content"))),
            );
            if let Some(failed) = block.get("is_error") {
                part.insert("is_error".into(), failed.clone());
            }
            Value::Object(part)
        }
        "image" => {
            let source = &block["source"];
            let (mime, data) = (text(source, "media_type"), text(source, "data"));
            if text(source, "type") == "url" {
                json!({"type": "image", "url": source["url"], "mime": source["media_type"]})
            } else if text(source, "type") == "base64" && !mime.is_empty() && !data.is_empty() {
                json!({"type": "image", "url": format!("data:{mime};base64,{data}"), "mime": mime})
            } else {
                data_part(&json!({
                    "kind": "image",
                    "media_type": source["media_type"],
                    "note": "inline bytes omitted; see raw",
                }))
            }
        }
        _ => block.clone(),
    }
}

/// The token counts of an Anthropic response, under the format's names.
/// `None` when it reports none of them.
pub(crate) fn anthropic_usage(usage: Option<&Value>) -> Option<Value> {
    let usage = usage.filter(|usage| usage.is_object())?;
    let mut counts = Map::new();
    for (name, theirs) in [
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cache_read_tokens", "cache_read_input_tokens"),
    ] {
        if let Some(count) = field(usage, theirs) {
            counts.insert(name.to_owned(), count.clone());
        }
    }
    (!counts.is_empty()).then_some(Value::Object(counts))
}
