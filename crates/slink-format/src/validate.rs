//! Schema and invariant validation, mirroring `validateRun` in
//! `packages/format/validate.mjs` and `ValidateRun` in the Go CLI.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::LazyLock;

use boon::{
    Compiler, Format, InstanceLocation, InstanceToken, SchemaIndex, Schemas, ValidationError,
};
use serde_json::Value;

use crate::datetime;

/// The canonical schema, as `packages/format` ships it.
pub const SCHEMA: &str = include_str!("../../../packages/format/session.schema.json");

/// Schema errors beyond this many are dropped, as the other validators do.
pub const MAX_SCHEMA_ERRORS: usize = 20;

struct Compiled {
    schemas: Schemas,
    index: SchemaIndex,
}

static COMPILED: LazyLock<Compiled> = LazyLock::new(|| {
    // Compiled under the schema's own `$id`. Nothing is fetched: every
    // reference in the schema is local, and the resource is added by hand.
    const LOCATION: &str = "https://session.link/schemas/session.v0.json";
    let schema: Value = serde_json::from_str(SCHEMA).expect("the embedded schema is JSON");
    let mut compiler = Compiler::new();
    // The JS validator asserts formats through ajv-formats and Go through
    // AssertFormat, so a malformed `created_at` is refused here too.
    compiler.enable_format_assertions();
    compiler.register_format(Format {
        name: "date-time",
        func: date_time,
    });
    compiler
        .add_resource(LOCATION, schema)
        .expect("the embedded schema loads");
    let mut schemas = Schemas::new();
    let index = compiler
        .compile(LOCATION, &mut schemas)
        .expect("the embedded schema compiles");
    Compiled { schemas, index }
});

fn date_time(value: &Value) -> Result<(), Box<dyn Error>> {
    match value {
        Value::String(text) => Ok(datetime::check(text)?),
        _ => Ok(()),
    }
}

/// Validates a parsed document. An empty list means it is valid.
///
/// Schema errors come first, one line each as `<instance path> <message>`
/// and at most [`MAX_SCHEMA_ERRORS`] of them. The invariants are checked only
/// on a document the schema accepts.
pub fn validate(document: &Value) -> Vec<String> {
    let compiled = &*COMPILED;
    match compiled.schemas.validate(document, compiled.index) {
        Ok(()) => invariant_errors(document),
        Err(error) => {
            let mut lines = Vec::new();
            collect(&error, &mut lines);
            lines
        }
    }
}

/// Only the errors with no cause beneath them say what is wrong; the nodes
/// above them say which part of the schema was being tried.
fn collect(error: &ValidationError, lines: &mut Vec<String>) {
    if lines.len() >= MAX_SCHEMA_ERRORS {
        return;
    }
    if error.causes.is_empty() {
        lines.push(format!(
            "{} {}",
            pointer(&error.instance_location),
            error.kind
        ));
        return;
    }
    for cause in &error.causes {
        collect(cause, lines);
    }
}

/// The location as a JSON pointer, with `/` for the document itself.
fn pointer(location: &InstanceLocation) -> String {
    if location.tokens.is_empty() {
        return "/".to_owned();
    }
    let mut out = String::new();
    for token in &location.tokens {
        out.push('/');
        match token {
            InstanceToken::Prop(name) => out.push_str(&name.replace('~', "~0").replace('/', "~1")),
            InstanceToken::Item(index) => out.push_str(&index.to_string()),
        }
    }
    out
}

fn id(span: &Value) -> &str {
    span.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// The rules JSON Schema cannot state: span ids are unique and no span is its
/// own ancestor. A `parent_id` that names no span is allowed.
pub fn invariant_errors(document: &Value) -> Vec<String> {
    let spans = document
        .get("spans")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut by_id: HashMap<&str, &Value> = HashMap::with_capacity(spans.len());
    for span in spans {
        if by_id.insert(id(span), span).is_some() {
            return vec![format!("duplicate span id \"{}\"", id(span))];
        }
    }

    let mut acyclic: HashSet<&str> = HashSet::with_capacity(spans.len());
    for span in spans {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut current = Some(span);
        while let Some(span) = current {
            let id = id(span);
            if acyclic.contains(id) {
                break;
            }
            if !seen.insert(id) {
                return vec![format!("parent_id cycle involving span \"{id}\"")];
            }
            current = match span.get("parent_id").and_then(Value::as_str) {
                Some(parent) if !parent.is_empty() => by_id.get(parent).copied(),
                _ => None,
            };
        }
        acyclic.extend(seen);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_embedded_schema_compiles() {
        LazyLock::force(&COMPILED);
    }

    #[test]
    fn a_minimal_document_is_valid() {
        let document = json!({
            "schema": "session/v0",
            "created_at": "2026-07-19T10:00:00Z",
            "spans": [{ "id": "root", "type": "agent" }],
        });
        assert_eq!(validate(&document), Vec::<String>::new());
    }

    #[test]
    fn schema_errors_name_the_place_and_stop_at_the_cap() {
        let spans: Vec<Value> = (0..40).map(|_| json!({ "type": "agent" })).collect();
        let document = json!({
            "schema": "session/v0",
            "created_at": "2026-07-19T10:00:00Z",
            "spans": spans,
        });
        let errors = validate(&document);
        assert_eq!(errors.len(), MAX_SCHEMA_ERRORS);
        assert!(errors[0].starts_with("/spans/0 "), "{}", errors[0]);
    }

    #[test]
    fn the_document_itself_is_reported_at_the_root() {
        let errors = validate(&json!([]));
        assert!(errors[0].starts_with("/ "), "{}", errors[0]);
    }

    #[test]
    fn invariants_do_not_assume_a_shape() {
        for value in [
            json!(null),
            json!(7),
            json!({ "spans": 7 }),
            json!({ "spans": [7] }),
        ] {
            invariant_errors(&value);
        }
    }
}
