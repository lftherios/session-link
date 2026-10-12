//! Every importer this crate has, over its fixtures in `testdata/import`. A
//! fixture passes when the import equals its golden run as parsed JSON, the
//! same test the Go importers are held to, and when the document is valid.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use slink_import::{HARNESSES, import, load_file, ported};

fn fixtures(harness: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/import")
        .join(harness);
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    found
}

#[test]
fn every_ported_importer_reproduces_its_goldens() {
    let mut ran = 0;
    for harness in HARNESSES.into_iter().filter(|harness| ported(harness)) {
        for fixture in fixtures(harness) {
            let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
            // Read as a file someone named, so the sub-agents kept beside
            // the transcript are found by the code that finds real ones.
            let (mut input, detected) = load_file(&fixture.join("input.session.jsonl")).unwrap();
            assert_eq!(detected, harness, "{harness}/{name}");
            input.fallback = "golden".into();
            let run = import(harness, &input)
                .unwrap()
                .unwrap_or_else(|| panic!("{harness}/{name}: nothing imported"));
            assert_eq!(
                slink_format::validate(&run),
                Vec::<String>::new(),
                "{harness}/{name}"
            );
            // What is written is what is compared: the text, read back.
            let written = slink_format::encode(&run);
            let got: Value = serde_json::from_str(&written).unwrap();
            let golden: Value =
                serde_json::from_str(&fs::read_to_string(fixture.join("golden.run.json")).unwrap())
                    .unwrap();
            assert!(
                got == golden,
                "{harness}/{name} differs from its golden:\n{}",
                first_difference(&got, &golden, "")
            );
            // The same input gives the same bytes, which is what finding an
            // unchanged import again depends on.
            assert_eq!(
                written,
                slink_format::encode(&import(harness, &input).unwrap().unwrap())
            );
            ran += 1;
        }
    }
    assert!(ran >= 6, "{ran} fixtures ran");
}

#[test]
fn a_harness_without_an_importer_says_so() {
    for harness in HARNESSES.into_iter().filter(|harness| !ported(harness)) {
        let error = import(harness, &Default::default()).unwrap_err();
        assert!(error.to_string().contains(harness), "{error}");
    }
    assert!(import("nope", &Default::default()).is_err());
}

/// Where two documents first part, for a failure someone can act on.
fn first_difference(got: &Value, want: &Value, at: &str) -> String {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) if x == y => {}
                    (Some(x), Some(y)) => return first_difference(x, y, &format!("{at}/{key}")),
                    (Some(_), None) => return format!("{at}/{key}: not in the golden"),
                    (None, _) => return format!("{at}/{key}: missing"),
                }
            }
            String::new()
        }
        (Value::Array(a), Value::Array(b)) => {
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                if x != y {
                    return first_difference(x, y, &format!("{at}/{index}"));
                }
            }
            format!("{at}: {} items, the golden has {}", a.len(), b.len())
        }
        _ => format!("{at}: got {got}, want {want}"),
    }
}
