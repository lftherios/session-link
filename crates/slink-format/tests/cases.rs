//! The Rust side of `testdata/format`: the cases every validator and scanner
//! is held to (`testdata/format/README.md`). The JavaScript test in
//! `test/format-cases.test.mjs` and the Go tests in `go/internal/format` and
//! `go/internal/scan` read the same two files.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use slink_format::{parse, scan_for_secrets, validate};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(name: &str) -> Vec<Value> {
    let path = root().join("testdata/format").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let cases: Vec<Value> = serde_json::from_str(&text).expect("the case file is JSON");
    assert!(!cases.is_empty(), "{name} has no cases");
    cases
}

/// What this crate makes of one case, or why that is not what the case says.
fn judge(case: &Value) -> Result<(), String> {
    // One verdict for every implementation, or one for each.
    let expected = case["expect"]
        .as_str()
        .or_else(|| case["expect"]["rust"].as_str())
        .ok_or("the case has no verdict for rust")?;
    let text = match case["text"].as_str() {
        Some(text) => text.to_owned(),
        None => case["doc"].to_string(),
    };
    let document = match parse(&text) {
        Ok(document) => document,
        Err(_) if expected == "unreadable" => return Ok(()),
        Err(err) => {
            return Err(format!(
                "want {expected}, but the text does not parse: {err}"
            ));
        }
    };
    let errors = validate(&document);
    let got = if errors.is_empty() {
        "valid"
    } else {
        "invalid"
    };
    if got != expected {
        return Err(format!("want {expected}, got {got}: {errors:?}"));
    }
    if let Some(at) = case["at"].as_str() {
        let prefix = format!("{at} ");
        if !errors.iter().any(|error| error.starts_with(&prefix)) {
            return Err(format!("no error at {at}: {errors:?}"));
        }
    }
    if let Some(error) = case["error"].as_str()
        && errors != [error]
    {
        return Err(format!("want [{error}], got {errors:?}"));
    }
    if let Some(count) = case["count"].as_u64()
        && errors.len() as u64 != count
    {
        return Err(format!("want {count} errors, got {}", errors.len()));
    }
    Ok(())
}

#[test]
fn every_format_case_gets_its_verdict() {
    let failures: Vec<String> = read("cases.json")
        .iter()
        .filter_map(|case| {
            judge(case)
                .err()
                .map(|why| format!("{}: {why}", case["name"]))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn every_scan_case_gets_its_findings() {
    let mut failures = Vec::new();
    for case in read("scan.json") {
        let text = case["text"].as_str().expect("a scan case has text");
        let found: Vec<Value> = scan_for_secrets(text)
            .into_iter()
            .map(|hit| serde_json::json!({ "pattern": hit.pattern, "preview": hit.preview }))
            .collect();
        if Value::Array(found.clone()) != case["hits"] {
            failures.push(format!("{}: found {found:?}", case["name"]));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Every fixture that is a whole document: the importers' goldens, the
/// format's examples and the viewer and share fixtures. Each is valid and
/// holds nothing the scan takes for a credential.
#[test]
fn every_fixture_document_is_valid_and_clean() {
    let root = root();
    let mut files = vec![
        root.join("packages/format/examples/chat.json"),
        root.join("packages/format/examples/agent-eval.json"),
        root.join("testdata/viewer/arrival/session.json"),
        root.join("testdata/share/research/session.json"),
        root.join("testdata/share/research/excerpt.json"),
    ];
    let importers = root.join("testdata/import");
    for harness in fs::read_dir(&importers).expect("testdata/import") {
        let harness = harness.unwrap().path();
        if !harness.is_dir() {
            continue;
        }
        let before = files.len();
        for fixture in fs::read_dir(&harness).unwrap() {
            let golden = fixture.unwrap().path().join("golden.run.json");
            if golden.is_file() {
                files.push(golden);
            }
        }
        assert!(
            files.len() > before,
            "no golden under {}",
            harness.display()
        );
    }
    for file in files {
        let text =
            fs::read_to_string(&file).unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        let document = parse(&text).unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        assert_eq!(
            validate(&document),
            Vec::<String>::new(),
            "{}",
            file.display()
        );
        assert_eq!(scan_for_secrets(&text), [], "{}", file.display());
    }
}
