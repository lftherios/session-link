//! What every importer's golden test shares. A fixture passes when the
//! import equals its golden run as parsed JSON, the test the Go importers
//! are held to, when the document is valid `session/v0`, and when the same
//! input gives the same bytes twice.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use slink_import::{Input, import};

/// The fixtures of one harness under `testdata/import`, by name.
pub fn fixtures(harness: &str) -> Vec<PathBuf> {
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

/// Holds one fixture's import to its golden run.
pub fn check_golden(harness: &str, fixture: &Path, input: &Input) {
    let name = format!(
        "{harness}/{}",
        fixture.file_name().unwrap().to_string_lossy()
    );
    let run = import(harness, input)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .unwrap_or_else(|| panic!("{name}: nothing imported"));
    assert_eq!(slink_format::validate(&run), Vec::<String>::new(), "{name}");
    // What is written is what is compared: the text, read back.
    let written = slink_format::encode(&run);
    let got: Value = serde_json::from_str(&written).unwrap();
    let golden: Value =
        serde_json::from_str(&fs::read_to_string(fixture.join("golden.run.json")).unwrap())
            .unwrap();
    assert!(
        got == golden,
        "{name} differs from its golden:\n{}",
        first_difference(&got, &golden, "")
    );
    // The same input gives the same bytes, which finding an unchanged import
    // again depends on.
    assert_eq!(
        written,
        slink_format::encode(&import(harness, input).unwrap().unwrap()),
        "{name}"
    );
}

/// Where two documents first part, for a failure someone can act on.
pub fn first_difference(got: &Value, want: &Value, at: &str) -> String {
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
