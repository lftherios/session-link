//! What every importer's golden test shares. A fixture passes when the
//! import equals its golden run as parsed JSON, the test the Go importers
//! are held to, when the document is valid `session/v0`, and when the same
//! input gives the same bytes twice. The helpers the discovery tests share
//! are here too: stores of a test's own, and the rows of a fixture kept in
//! a database.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::types::Value as Cell;
use serde_json::Value;
use slink_import::{Agent, Candidate, Input, Locations, import, recent};

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

/* ------------------------------------------------------------- discovery */

/// An empty directory of this test's own.
pub fn scratch(test: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{}-{test}", env!("CARGO_CRATE_NAME")));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a file, and the directories it is in, as if it was last written
/// at `at`.
pub fn write(file: &Path, body: impl AsRef<[u8]>, at: SystemTime) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, body).unwrap();
    let written = fs::File::options().write(true).open(file).unwrap();
    written.set_modified(at).unwrap();
}

/// A machine with no agent's store, so that discovery reads nothing of the
/// machine the tests run on. A test names the stores it has beside it.
pub fn nowhere() -> Locations {
    Locations {
        claude: PathBuf::new(),
        pi: PathBuf::new(),
        omp: PathBuf::new(),
        codex: PathBuf::new(),
        opencode: PathBuf::new(),
        hermes: PathBuf::new(),
        dsh: PathBuf::new(),
    }
}

/// The sessions of `harness` listed for `cwd`, all of which must have been
/// read.
pub fn listed(
    stores: &Locations,
    harness: &str,
    cwd: impl AsRef<Path>,
    id: &str,
    limit: usize,
) -> Vec<Candidate> {
    let (candidates, problems) = recent(stores, harness, cwd.as_ref(), id, limit);
    assert_eq!(problems, Vec::<String>::new());
    candidates
}

pub fn ids(candidates: &[Candidate]) -> Vec<&str> {
    candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect()
}

pub fn agent_ids(agents: &[Agent]) -> Vec<&str> {
    agents.iter().map(|agent| agent.id.as_str()).collect()
}

/* ------------------------------------------------- fixtures that are rows */

pub fn read(file: &Path) -> Value {
    let text =
        fs::read_to_string(file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
    slink_format::parse(&text).unwrap_or_else(|error| panic!("{}: {error}", file.display()))
}

pub fn rows(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// Recorded sub-agents as the loader hands them to the importer: each one's
/// session row and messages, and its own sub-agents the same way.
pub fn agents(recorded: &Value) -> Vec<Agent> {
    rows(recorded)
        .iter()
        .map(|agent| Agent {
            id: agent["id"].as_str().unwrap_or_default().to_owned(),
            call: String::new(),
            name: agent["name"].as_str().unwrap_or_default().to_owned(),
            input: Input {
                session: agent["session"].clone(),
                messages: rows(&agent["messages"]),
                agents: agents(&agent["agents"]),
                ..Input::default()
            },
        })
        .collect()
}

/// A fixture's value as the cell a row holds it in: an array or an object as
/// its JSON text, which is how both harnesses keep one.
pub fn cell(value: &Value) -> Cell {
    match value {
        Value::Null => Cell::Null,
        Value::Number(number) => match number.as_i64() {
            Some(whole) => Cell::Integer(whole),
            None => Cell::Real(number.as_f64().unwrap()),
        },
        Value::String(text) => Cell::Text(text.clone()),
        other => Cell::Text(other.to_string()),
    }
}
