//! The Hermes importer over its fixtures in `testdata/import`, and over
//! databases laid out as Hermes lays out its own.
//!
//! A fixture is rows, so no built CLI can be run over it. In place of that,
//! the same rows are written to a database here and read back by the code
//! that reads a real one. What finds the newest session without being told
//! where the database is reads the environment, which a test cannot set; the
//! functions behind it take a path and are tested beside them, in
//! `src/hermes.rs`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{agent_ids, agents, cell, ids, listed, nowhere, read, rows};
use rusqlite::Connection;
use serde_json::Value;
use slink_import::{Found, Input, Loader, Locations, import, recent};

const SECOND: i64 = 1_000_000_000;

/// A fixture's rows as the Go golden test hands them to the importer.
fn fixture_input(fixture: &Path) -> Input {
    let recorded = fixture.join("input.agents.json");
    Input {
        session: read(&fixture.join("input.session.json")),
        messages: rows(&read(&fixture.join("input.messages.json"))),
        fallback: "golden".into(),
        blobs: fixture.join("blobs"),
        agents: if recorded.is_file() {
            agents(&read(&recorded))
        } else {
            Vec::new()
        },
        ..Input::default()
    }
}

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("hermes");
    assert!(fixtures.len() >= 4, "{} fixtures", fixtures.len());
    for fixture in fixtures {
        common::check_golden("hermes", &fixture, &fixture_input(&fixture));
    }
}

/* ------------------------------------------------------------- databases */

/// The messages table of Hermes 0.15, reduced to the columns an import
/// reads.
const MESSAGES: &str = "CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT,
    timestamp REAL, token_count INTEGER, finish_reason TEXT, reasoning TEXT,
    reasoning_content TEXT, active INTEGER NOT NULL DEFAULT 1)";

/// Its sessions table, reduced to the columns discovery reads.
const SESSIONS: &str = "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT,
    model_config TEXT, parent_session_id TEXT, started_at REAL, ended_at REAL, end_reason TEXT,
    cwd TEXT, title TEXT)";

/// A sessions table with a column for every field a fixture's session has.
const WHOLE_SESSIONS: &str = "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT,
    model TEXT, model_config TEXT, system_prompt TEXT, parent_session_id TEXT,
    started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, billing_provider TEXT, cwd TEXT,
    title TEXT, display_name TEXT)";

/// A database of its own for one test, made by `statements`.
fn database(test: &str, statements: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("hermes")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("state.db");
    execute(&file, statements);
    file
}

fn execute(file: &Path, statements: &[&str]) {
    let db = Connection::open(file).unwrap();
    for statement in statements {
        db.execute_batch(statement)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

/// Writes one row, a column for each of its fields.
fn insert(db: &Connection, table: &str, row: &Value) {
    let row = row.as_object().unwrap();
    let columns: Vec<&str> = row.keys().map(String::as_str).collect();
    let marks = vec!["?"; columns.len()].join(", ");
    let statement = format!(
        "INSERT INTO {table} ({}) VALUES ({marks})",
        columns.join(", ")
    );
    db.execute(
        &statement,
        rusqlite::params_from_iter(row.values().map(cell)),
    )
    .unwrap_or_else(|error| panic!("{statement}: {error}"));
}

/// Writes a session as Hermes records one: its row, a row for each message
/// in the order they were said, and the same for each of its sub-agents'
/// sessions, which name it as their parent.
fn record(db: &Connection, input: &Input) {
    insert(db, "sessions", &input.session);
    for message in &input.messages {
        let mut row = message.clone();
        row["session_id"] = input.session["id"].clone();
        insert(db, "messages", &row);
    }
    for agent in &input.agents {
        record(db, &agent.input);
    }
}

/// A database holding every fixture's session.
fn fixture_database(test: &str) -> PathBuf {
    let file = database(test, &[WHOLE_SESSIONS, MESSAGES]);
    let db = Connection::open(&file).unwrap();
    for fixture in common::fixtures("hermes") {
        record(&db, &fixture_input(&fixture));
    }
    file
}

/// A session of the database at `file`, as it is found when named by id.
fn session(file: &Path, id: &str) -> Found {
    Found {
        harness: "hermes",
        recency: 0,
        loader: Loader::Database {
            path: file.to_owned(),
            id: id.to_owned(),
        },
    }
}

/// Hermes's database at `file`, and no other harness's store anywhere.
fn stores(file: &Path) -> Locations {
    Locations {
        hermes: file.to_owned(),
        ..nowhere()
    }
}

fn written(input: &Input) -> String {
    slink_format::encode(&import("hermes", input).unwrap().unwrap())
}

#[test]
fn a_session_read_from_a_database_reproduces_its_golden() {
    let file = fixture_database("goldens");
    for fixture in common::fixtures("hermes") {
        let rows = fixture_input(&fixture);
        let id = rows.session["id"].as_str().unwrap();
        let loaded = session(&file, id).load().unwrap();
        common::check_golden("hermes", &fixture, &loaded);
        // A database gives a time as a real and a count as an integer,
        // whatever the fixture's JSON spells, and the bytes are the same.
        assert_eq!(written(&loaded), written(&rows), "{id}");
        assert_eq!(agent_ids(&loaded.agents), agent_ids(&rows.agents), "{id}");
        for (loaded, recorded) in loaded.agents.iter().zip(&rows.agents) {
            assert_eq!(
                (&loaded.name, &loaded.call),
                (&recorded.name, &recorded.call)
            );
        }
    }
}

#[test]
fn the_session_of_a_fixture_is_found_in_its_database() {
    let file = fixture_database("found");
    execute(
        &file,
        &["INSERT INTO sessions (id, title, cwd, started_at) VALUES
            ('before', 'The day before', '/tmp/demo', 1752800000),
            ('long before', NULL, '/tmp/demo', 1752700000)"],
    );
    let fixture = |name: &str| {
        let found = common::fixtures("hermes")
            .into_iter()
            .find(|fixture| fixture.ends_with(name));
        found.unwrap()
    };

    // For its directory, from inside it, newest first.
    let stores = stores(&file);
    let all = listed(&stores, "hermes", "/tmp/demo/src", "", 30);
    assert_eq!(ids(&all), ["hermes_golden_basic", "before", "long before"]);
    let basic = &all[0];
    assert_eq!(basic.dir, Path::new("/tmp/demo"));
    assert_eq!(basic.title, "List the project files");
    assert_eq!(basic.name, "List the project files");
    assert_eq!((&basic.file, basic.related.len()), (&None, 0));
    // As recent as its last message, and started when its row says.
    assert_eq!(basic.found.recency, 1_752_900_004 * SECOND);
    assert_eq!(basic.started, Some((1_752_900_000, 0)));
    // A session Hermes gave no title is shown by its id.
    assert_eq!(
        (all[2].title.as_str(), all[2].name.as_str()),
        ("long before", "")
    );
    common::check_golden("hermes", &fixture("basic"), &basic.found.load().unwrap());

    // The limit holds, and an id is matched before it.
    assert_eq!(
        ids(&listed(&stores, "hermes", "/tmp/demo", "", 1)),
        ["hermes_golden_basic"]
    );
    assert_eq!(ids(&listed(&stores, "hermes", "/tmp/demo", "", 2)).len(), 2);
    assert_eq!(
        ids(&listed(&stores, "hermes", "/tmp/demo", "long before", 1)),
        ["long before"]
    );
    assert!(listed(&stores, "hermes", "/tmp/demo", "no such session", 30).is_empty());
    assert!(listed(&stores, "hermes", "/tmp/elsewhere", "", 30).is_empty());

    // A delegate's session is in its parent's directory and is not the
    // person's own: only its id reaches it, and its parent brings it along.
    let (parent, delegate) = ("20261008_090000_d1e2f3", "20261008_090003_a7b8c9");
    let own = listed(&stores, "hermes", "/work/certs", "", 30);
    assert_eq!(ids(&own), [parent]);
    assert_eq!(
        ids(&listed(&stores, "hermes", "/work/certs", delegate, 1)),
        [delegate]
    );
    let loaded = own[0].found.load().unwrap();
    assert_eq!(agent_ids(&loaded.agents), [delegate]);
    common::check_golden("hermes", &fixture("delegate"), &loaded);
}

/* --------------------------------- what the Go tests hold discovery to */

/// A session chosen from a listing stays that session, whatever is recorded
/// after it was listed.
#[test]
fn a_listed_session_is_the_one_loaded() {
    // A store from before Hermes linked sessions or kept rewound messages.
    let file = database(
        "listed",
        &[
            "CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at INTEGER)",
            "CREATE TABLE messages (id INTEGER, session_id TEXT, role TEXT, content TEXT,
            tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL,
            token_count INTEGER, finish_reason TEXT, reasoning TEXT, reasoning_content TEXT)",
            "INSERT INTO messages (id, session_id, role, content)
            VALUES (1, 'older', 'user', 'Original prompt')",
            "INSERT INTO sessions VALUES ('older', 'First task', '/work/project', 1),
            ('newer', 'Second task', '/work/project', 2), ('elsewhere', 'Other task', '/other', 3)",
        ],
    );
    let stores = stores(&file);
    assert_eq!(
        ids(&listed(&stores, "hermes", "/work/project/src", "", 30)),
        ["newer", "older"]
    );
    let chosen = listed(&stores, "hermes", "/work/project", "older", 1);
    assert_eq!(ids(&chosen), ["older"]);
    execute(
        &file,
        &["UPDATE sessions SET started_at = 100 WHERE id = 'newer'"],
    );
    let input = chosen[0].found.load().unwrap();
    assert_eq!(input.session["id"], "older");
    assert_eq!(input.messages.len(), 1);
    assert_eq!(input.messages[0]["content"], "Original prompt");
    // An incompatible message schema must fail, not silently omit evidence.
    execute(&file, &["DROP TABLE messages"]);
    let error = chosen[0].found.load().unwrap_err();
    assert!(
        error.starts_with("cannot read hermes messages: "),
        "{error}"
    );
}

#[test]
fn a_store_that_cannot_be_read_is_reported_beside_what_was_read() {
    let file = database("incompatible", &["CREATE TABLE incompatible (id TEXT)"]);
    // Claude Code's store beside it, with one session for the directory.
    let claude = file.with_file_name("claude");
    let project = claude.join("-work-project");
    fs::create_dir_all(&project).unwrap();
    let entry = r#"{"type":"user","cwd":"/work/project","message":{"role":"user","content":"Tidy the importer"}}"#;
    fs::write(project.join("one.jsonl"), format!("{entry}\n")).unwrap();

    let locations = Locations {
        claude,
        ..stores(&file)
    };
    let (candidates, problems) = recent(&locations, "", Path::new("/work/project"), "", 30);
    assert_eq!(ids(&candidates), ["one"]);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("cannot read hermes sessions: "),
        "{problems:?}"
    );
    // A machine without Hermes is not a problem to report.
    let absent = Locations {
        hermes: file.with_file_name("absent.db"),
        ..locations
    };
    let (candidates, problems) = recent(&absent, "", Path::new("/work/project"), "", 30);
    assert_eq!(ids(&candidates), ["one"]);
    assert_eq!(problems, Vec::<String>::new());
}

/// Hermes links sessions to a parent for three different reasons.
#[test]
fn sessions_are_listed_as_hermes_lists_its_own() {
    let file = database(
        "own-list",
        &[
            SESSIONS,
            MESSAGES,
            // A conversation whose context was compressed: Hermes ends the
            // session and continues in a new one, for which it records no
            // directory.
            "INSERT INTO sessions VALUES ('first', 'cli', NULL, NULL, 100, 200, 'compression',
            '/work/project', 'Billing refactor')",
            "INSERT INTO sessions VALUES ('continued', 'cli', NULL, 'first', 200, NULL, NULL, NULL,
            'Billing refactor #2')",
            // Started while its parent was live: a sub-agent or a background
            // review.
            "INSERT INTO sessions VALUES ('delegate', 'cli', NULL, 'continued', 250, 260,
            'completed', '/work/project', NULL)",
            // Two branches: one after its parent ended as branched, one
            // marked.
            "INSERT INTO sessions VALUES ('trunk', 'cli', NULL, NULL, 300, 400, 'branched',
            '/work/project', 'Trunk')",
            "INSERT INTO sessions VALUES ('branch', 'cli', NULL, 'trunk', 400, NULL, NULL,
            '/work/project', 'Branch')",
            r#"INSERT INTO sessions VALUES ('marked', 'cli', '{"_branched_from":"trunk"}', 'trunk',
            350, NULL, NULL, '/work/project', 'Marked branch')"#,
            "INSERT INTO sessions VALUES ('elsewhere', 'cli', 'not json', NULL, 500, NULL, NULL,
            '/other', 'Other project')",
            // The person rewound the first exchange; Hermes keeps it as
            // inactive.
            "INSERT INTO messages (session_id, role, content, timestamp, active) VALUES
            ('first', 'user', 'Delete the legacy module', 101, 0),
            ('first', 'assistant', 'Deleted.', 102, 0),
            ('first', 'user', 'Keep the legacy module and add a test', 103, 1),
            ('first', 'assistant', 'Test added.', 104, 1)",
        ],
    );
    let stores = stores(&file);
    let all = listed(&stores, "hermes", "/work/project/src", "", 30);
    assert_eq!(
        ids(&all),
        ["branch", "marked", "trunk", "continued", "first"]
    );
    for candidate in &all {
        assert_eq!(
            candidate.dir,
            Path::new("/work/project"),
            "{}",
            candidate.id
        );
    }
    assert_eq!(all[0].found.recency, 400 * SECOND);
    assert_eq!(
        ids(&listed(&stores, "hermes", "/work/project", "delegate", 1)),
        ["delegate"],
        "an explicit id must still reach a sub-agent"
    );
    assert_eq!(
        ids(&listed(&stores, "hermes", "/work/project", "continued", 1)),
        ["continued"],
        "a continuation must be found in its conversation's directory"
    );

    let input = all[4].found.load().unwrap();
    let said: Vec<&str> = input
        .messages
        .iter()
        .map(|message| message["content"].as_str().unwrap())
        .collect();
    assert_eq!(
        said,
        ["Keep the legacy module and add a test", "Test added."],
        "rewound messages must stay out of the import"
    );
}

/// A session started in the home directory belongs to work done there, and
/// to no project beneath it.
#[test]
fn a_session_in_the_home_directory_belongs_to_home_only() {
    // Only the home directory's name is used: nothing in it is read.
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let Some(home) = std::env::var_os(name).filter(|home| !home.is_empty()) else {
        return;
    };
    let home = PathBuf::from(home);
    let file = database(
        "home",
        &["CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at INTEGER)"],
    );
    Connection::open(&file)
        .unwrap()
        .execute(
            "INSERT INTO sessions VALUES ('hermes', 'At home', ?, 1)",
            [home.to_string_lossy()],
        )
        .unwrap();
    let project = home.join("code").join("proj");
    let (at_home, _) = recent(&stores(&file), "hermes", &home, "", 30);
    assert_eq!(ids(&at_home), ["hermes"]);
    let (beneath, problems) = recent(&stores(&file), "hermes", &project, "", 30);
    assert!(beneath.is_empty(), "{:?}", ids(&beneath));
    assert_eq!(problems, Vec::<String>::new());
}

/// A long-running session is as recent as its last activity, as a
/// transcript file is as recent as its last write.
#[test]
fn sessions_rank_by_their_last_message() {
    let file = database(
        "activity",
        &[
            "CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at REAL)",
            "CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT,
            content TEXT, timestamp REAL)",
            "INSERT INTO sessions VALUES
            ('long-running', 'Started first, still going', '/work/project', 1),
            ('quick', 'Started later, finished', '/work/project', 5),
            ('silent', 'No messages yet', '/work/project', 3)",
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES
            ('long-running', 'user', 'still here', 9), ('quick', 'user', 'done', 6)",
        ],
    );
    let all = listed(&stores(&file), "hermes", "/work/project", "", 30);
    assert_eq!(ids(&all), ["long-running", "quick", "silent"]);
    assert_eq!(all[0].found.recency, 9 * SECOND);
    assert_eq!(all[0].started, Some((1, 0)));
}

/// Hermes keeps a sub-agent's session apart from its parent's. Loading the
/// parent brings them along, and only the ones that are sub-agents.
#[test]
fn sub_agent_sessions_load_with_their_parent() {
    let file = database(
        "sub-agents",
        &[
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT, started_at REAL,
            ended_at REAL, end_reason TEXT, cwd TEXT, title TEXT)",
            MESSAGES,
            "INSERT INTO sessions VALUES
            ('main', NULL, 100, 200, 'compression', '/work/project', 'Main')",
            // Started while main was live: a delegate. Started after it
            // ended: its continuation.
            "INSERT INTO sessions VALUES
            ('delegate', 'main', 150, 160, 'completed', '/work/project', NULL)",
            "INSERT INTO sessions VALUES ('continued', 'main', 200, NULL, NULL, NULL, 'Main #2')",
            "INSERT INTO sessions VALUES
            ('live-delegate', 'continued', 250, NULL, NULL, '/work/project', NULL)",
        ],
    );
    let first = session(&file, "main").load().unwrap();
    assert_eq!(agent_ids(&first.agents), ["delegate"]);
    let running = session(&file, "continued").load().unwrap();
    assert_eq!(agent_ids(&running.agents), ["live-delegate"]);
}

#[test]
fn a_session_that_is_not_there_is_an_error() {
    // No database, or no such session in it: a made-up id must fail, and
    // never load as an empty session, which made a hollow capture.
    let file = database("missing", &[SESSIONS, MESSAGES]);
    assert!(
        session(&file.with_file_name("nope.db"), "bogus")
            .load()
            .is_err()
    );
    let error = session(&file, "bogus").load().unwrap_err();
    assert!(
        error.starts_with("hermes session \"bogus\" not found in "),
        "{error}"
    );
}
