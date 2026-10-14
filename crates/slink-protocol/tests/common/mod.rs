//! Reading `testdata/protocol`, for the tests that hold this crate to it.

use std::fs;
use std::path::Path;

use serde_json::Value;
use slink_protocol::Signed;

pub fn read(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_json::from_str(&text).expect("the case file is JSON")
}

/// One verdict for every implementation, or one for each.
pub fn expected(case: &Value) -> &str {
    case["expect"]
        .as_str()
        .or_else(|| case["expect"]["rust"].as_str())
        .expect("the case has a verdict for rust")
}

pub fn text<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field]
        .as_str()
        .unwrap_or_else(|| panic!("{}: no {field}", case["name"]))
}

/// A statement written out in a case.
pub fn signed(value: &Value) -> Signed {
    Signed {
        payload: text(value, "payload").to_owned(),
        signature: text(value, "signature").to_owned(),
    }
}

pub fn report(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
