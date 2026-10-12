//! Reading a transcript's JSON the way the importers were first written, in
//! JavaScript: a field that is missing, null or of another type is simply
//! not there, and "set" means what `if (x)` means.

use serde_json::{Map, Value};

/// A field of an object; `None` when the value is not an object, the field
/// is missing, or it is null.
pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| !found.is_null())
}

/// The field as text, or `""`.
pub(crate) fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// The field as it is, with null for a missing one: what Go and JavaScript
/// both write when a span is given a value that was never there.
pub(crate) fn or_null(value: &Value, name: &str) -> Value {
    value.get(name).cloned().unwrap_or(Value::Null)
}

/// JavaScript's truthiness.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(set)) => *set,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|float| float != 0.0),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// Copies a field under another name when the source has it at all, null
/// included: JSON leaves out what was never assigned and keeps what was.
pub(crate) fn put(target: &mut Map<String, Value>, key: &str, source: &Value, source_key: &str) {
    if let Some(found) = source.get(source_key) {
        target.insert(key.to_owned(), found.clone());
    }
}
