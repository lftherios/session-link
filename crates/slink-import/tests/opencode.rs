//! The opencode importer over its fixtures in `testdata/import`, and over
//! databases laid out as opencode lays one out.

mod common;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;
use slink_import::{Agent, Candidate, Found, Input, Loader, Locations, recent};

/// The three tables opencode keeps a session in, with the columns every
/// version of it has. `store` adds the columns a session's row brings, so a
/// fixture from a newer opencode makes a newer database.
const TABLES: &str = "
    CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_created INTEGER);
    CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
    CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);";

fn read(file: &Path) -> Value {
    let text =
        fs::read_to_string(file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", file.display()))
}

fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// What a fixture records of the sessions its sub-agents ran in, or null
/// for a fixture without any.
fn recorded_agents(fixture: &Path) -> Value {
    let file = fixture.join("input.agents.json");
    if file.is_file() {
        read(&file)
    } else {
        Value::Null
    }
}

/// Recorded sub-agents as the loader hands them to the importer: each one's
/// session row and messages, and its own sub-agents the same way.
fn agents(recorded: &Value) -> Vec<Agent> {
    list(recorded)
        .iter()
        .map(|agent| Agent {
            id: agent["id"].as_str().unwrap_or_default().to_owned(),
            call: String::new(),
            name: agent["name"].as_str().unwrap_or_default().to_owned(),
            input: Input {
                session: agent["session"].clone(),
                messages: list(&agent["messages"]),
                agents: agents(&agent["agents"]),
                ..Input::default()
            },
        })
        .collect()
}

/// A new database under the build's own temporary directory, holding what
/// `statements` put in it. The connection is returned so a test can go on
/// changing the database as a running opencode would.
fn database(name: &str, statements: &str) -> (PathBuf, Connection) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("opencode");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.db"));
    let _ = fs::remove_file(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(statements).unwrap();
    (path, db)
}

/// A JSON value as the cell a row holds it in.
fn cell(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Cell;
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

/// Writes a session as opencode stores one: its row, a row for each message
/// with the message as JSON text, a row for each part the same way, and
/// then each sub-agent's session, which names this one as its parent in its
/// own row.
fn store(db: &Connection, session: &Value, messages: &Value, agents: &Value) {
    let row = session.as_object().expect("a session row");
    let known: Vec<String> = db
        .prepare("SELECT name FROM pragma_table_info('session')")
        .unwrap()
        .query_map([], |column| column.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for column in row.keys().filter(|column| !known.contains(column)) {
        db.execute(&format!("ALTER TABLE session ADD COLUMN {column}"), [])
            .unwrap();
    }
    let columns: Vec<&str> = row.keys().map(String::as_str).collect();
    db.execute(
        &format!(
            "INSERT INTO session ({}) VALUES ({})",
            columns.join(", "),
            vec!["?"; columns.len()].join(", ")
        ),
        rusqlite::params_from_iter(row.values().map(cell)),
    )
    .unwrap();
    for message in list(messages) {
        db.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
            rusqlite::params![
                cell(&message["id"]),
                cell(&session["id"]),
                cell(&message["time_created"]),
                message["data"].to_string(),
            ],
        )
        .unwrap();
        // Parts are read back in the order of their ids, as opencode's own
        // ids put them in the order they were written.
        for (index, part) in list(&message["parts"]).iter().enumerate() {
            db.execute(
                "INSERT INTO part (id, message_id, session_id, time_created, data) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    format!("prt_{}_{index:04}", message["id"].as_str().unwrap()),
                    cell(&message["id"]),
                    cell(&session["id"]),
                    cell(&message["time_created"]),
                    part.to_string(),
                ],
            )
            .unwrap();
        }
    }
    for agent in list(agents) {
        store(db, &agent["session"], &agent["messages"], &agent["agents"]);
    }
}

/// A database holding one fixture: its session, and the sessions of its
/// sub-agents.
fn fixture_database(fixture: &Path, name: &str) -> (PathBuf, Connection, Value) {
    let (path, db) = database(name, TABLES);
    let session = read(&fixture.join("input.session.json"));
    store(
        &db,
        &session,
        &read(&fixture.join("input.messages.json")),
        &recorded_agents(fixture),
    );
    (path, db, session)
}

/// A session of the database at `path`, as a command given its id finds it.
fn by_id(path: &Path, id: &str) -> Found {
    Found {
        harness: "opencode",
        recency: 0,
        loader: Loader::Database {
            path: path.to_owned(),
            id: id.to_owned(),
        },
    }
}

/// opencode's sessions for `cwd` in the database at `path`, and what could
/// not be read. No other harness has a store here, and none is asked.
fn listed(path: &Path, cwd: &Path, id: &str, limit: usize) -> (Vec<Candidate>, Vec<String>) {
    let locations = Locations {
        claude: PathBuf::new(),
        pi: PathBuf::new(),
        omp: PathBuf::new(),
        codex: PathBuf::new(),
        opencode: path.to_owned(),
        hermes: PathBuf::new(),
        dsh: PathBuf::new(),
    };
    recent(&locations, "opencode", cwd, id, limit)
}

/// The sessions `listed` finds, when nothing stood in the way of reading.
fn found(path: &Path, cwd: &str, id: &str, limit: usize) -> Vec<Candidate> {
    let (candidates, problems) = listed(path, Path::new(cwd), id, limit);
    assert_eq!(problems, Vec::<String>::new());
    candidates
}

fn ids(candidates: &[Candidate]) -> String {
    let ids: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    ids.join(",")
}

fn agent_ids(agents: &[Agent]) -> String {
    let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
    ids.join(",")
}

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("opencode");
    assert!(fixtures.len() >= 5, "{} fixtures", fixtures.len());
    for fixture in fixtures {
        // The rows as a fixture holds them: the session's, its messages'
        // and, for a session that started sub-agents, theirs.
        let input = Input {
            session: read(&fixture.join("input.session.json")),
            messages: list(&read(&fixture.join("input.messages.json"))),
            agents: agents(&recorded_agents(&fixture)),
            fallback: "golden".into(),
            blobs: fixture.join("blobs"),
            ..Input::default()
        };
        common::check_golden("opencode", &fixture, &input);
    }
}

/// There is no transcript file to hand a command, so in place of a run
/// through the binary each fixture is put in a database and read back out by
/// the code that reads a real one.
#[test]
fn a_fixture_in_a_database_reads_as_its_golden() {
    for fixture in common::fixtures("opencode") {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let (path, _db, session) = fixture_database(&fixture, &format!("fixture-{name}"));
        let mut input = by_id(&path, session["id"].as_str().unwrap())
            .load()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        // Each message comes back as the fixture has it, with its parts in
        // their order, and each sub-agent's session under its parent's.
        assert_eq!(
            input.messages,
            list(&read(&fixture.join("input.messages.json"))),
            "{name}"
        );
        let recorded = list(&recorded_agents(&fixture));
        assert_eq!(input.agents.len(), recorded.len(), "{name}");
        for (agent, recorded) in input.agents.iter().zip(&recorded) {
            assert_eq!(agent.id, recorded["id"], "{name}");
            assert_eq!(agent.name, recorded["name"], "{name}");
            assert_eq!(agent.input.session, recorded["session"], "{name}");
            assert_eq!(agent.input.messages, list(&recorded["messages"]), "{name}");
        }
        input.fallback = "golden".into();
        common::check_golden("opencode", &fixture, &input);
    }
}

#[test]
fn a_session_is_found_for_its_directory_and_by_its_id() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/import/opencode/v1.18-sub-agent");
    let (path, db, session) = fixture_database(&fixture, "found");
    let id = session["id"].as_str().unwrap();
    let child = recorded_agents(&fixture)[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(session["directory"], "/work/checks");
    // Beside the fixture's session: one for the same directory worked on
    // before it, one for the directory above worked on after it, and one
    // for another project.
    db.execute_batch(
        "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES
            ('ses_before', 'Before', '/work/checks', 1791387500000, 1791387500500),
            ('ses_above', 'Above', '/work', 1791387600000, 1791387600500),
            ('ses_other', 'Other', '/other', 1791387700000, 1791387700500)",
    )
    .unwrap();

    // From inside the project: its sessions and its parent directory's,
    // most recently active first. The sub-agent's session is the fixture
    // session's own, and is not offered.
    let all = found(&path, "/work/checks/src", "", 30);
    assert_eq!(ids(&all), format!("ses_above,{id},ses_before"));
    let own = &all[1];
    assert_eq!(own.found.harness, "opencode");
    assert_eq!(own.title, "Greeting check");
    assert_eq!(own.name, "Greeting check");
    assert_eq!(own.dir, Path::new("/work/checks"));
    assert_eq!(all[0].dir, Path::new("/work"));
    assert_eq!(own.file, None);
    // Active when the row was last updated, started when it was created.
    assert_eq!(session["time_updated"], 1_791_387_532_024_i64);
    assert_eq!(own.found.recency, 1_791_387_532_024_000_000);
    assert_eq!(session["time_created"], 1_791_387_530_936_i64);
    assert_eq!(own.started, Some((1_791_387_530, 936_000_000)));

    // The limit keeps the newest.
    assert_eq!(ids(&found(&path, "/work/checks/src", "", 1)), "ses_above");
    assert_eq!(
        ids(&found(&path, "/work/checks", "", 2)),
        format!("ses_above,{id}")
    );

    // An id names one session, a sub-agent's included, among the project's.
    assert_eq!(ids(&found(&path, "/work/checks", id, 1)), id);
    assert_eq!(ids(&found(&path, "/work/checks/src", &child, 1)), child);
    assert_eq!(ids(&found(&path, "/work/checks", "ses_other", 1)), "");
    assert_eq!(ids(&found(&path, "/work/checks", "ses_nobody", 1)), "");

    // What was found loads as the fixture's session.
    let chosen = found(&path, "/work/checks", id, 1);
    let mut input = chosen[0].found.load().unwrap();
    input.fallback = "golden".into();
    common::check_golden("opencode", &fixture, &input);
}

#[test]
fn a_session_that_is_not_there_says_which_and_where() {
    let (path, _db) = database("absent", TABLES);
    let error = by_id(&path, "ses_\"nobody\"\n").load().unwrap_err();
    assert_eq!(
        error,
        format!(
            "opencode session \"ses_\\\"nobody\\\"\\n\" not found in {}",
            path.display()
        )
    );
    // No database at all is an error too, never an empty session.
    assert!(
        by_id(&path.with_extension("gone"), "ses_nobody")
            .load()
            .is_err()
    );
}

#[test]
fn a_store_that_is_not_there_has_no_sessions_and_one_that_cannot_be_read_says_so() {
    let (path, _db) = database("unreadable", "CREATE TABLE incompatible (id TEXT)");
    let cwd = Path::new("/work/project");
    // A machine without opencode is ordinary.
    let (candidates, problems) = listed(&path.with_extension("gone"), cwd, "", 30);
    assert!(candidates.is_empty() && problems.is_empty(), "{problems:?}");
    let (candidates, problems) = listed(Path::new(""), cwd, "", 30);
    assert!(candidates.is_empty() && problems.is_empty(), "{problems:?}");
    // A database without opencode's tables is not.
    let (candidates, problems) = listed(&path, cwd, "", 30);
    assert!(candidates.is_empty());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("cannot read opencode sessions: "),
        "{problems:?}"
    );
    // Nor is a file that is no database.
    let text = path.with_extension("txt");
    fs::write(&text, "not a database, and long enough to be read as one\n").unwrap();
    let (_, problems) = listed(&text, cwd, "", 30);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("cannot read opencode sessions: "),
        "{problems:?}"
    );
}

/* -------------------------------------- the Go tests that concern opencode */

/// The schema here is the oldest the queries fit: no parent, no time of
/// last update.
#[test]
fn a_chosen_session_stays_the_one_that_is_loaded() {
    let (path, db) = database(
        "recent",
        r#"CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_created INTEGER);
           CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
           CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
           INSERT INTO message VALUES ('msg','older',1,'{"role":"user"}');
           INSERT INTO part VALUES ('part','older','msg',1,'{"type":"text","text":"Original prompt"}');
           INSERT INTO session VALUES ('older','First task','/work/project',1),
               ('newer','Second task','/work/project',2), ('elsewhere','Other task','/other',3);"#,
    );
    assert_eq!(
        ids(&found(&path, "/work/project/src", "", 30)),
        "newer,older"
    );
    let chosen = found(&path, "/work/project", "older", 1);
    assert_eq!(ids(&chosen), "older");
    db.execute("UPDATE session SET time_created=100 WHERE id='newer'", [])
        .unwrap();
    let input = chosen[0].found.load().unwrap();
    assert_eq!(input.session["id"], "older");
    assert!(
        Value::Array(input.messages)
            .to_string()
            .contains("Original prompt")
    );
    // An incompatible message schema must fail, not silently omit evidence.
    db.execute("DROP TABLE part", []).unwrap();
    let error = chosen[0].found.load().unwrap_err();
    assert!(
        error.starts_with("cannot read opencode message parts: "),
        "{error}"
    );
    db.execute("DROP TABLE message", []).unwrap();
    let error = chosen[0].found.load().unwrap_err();
    assert!(
        error.starts_with("cannot read opencode messages: "),
        "{error}"
    );
}

/// A harness records a sub-agent's work as a session of its own, in the
/// same directory as the session that started it. It is not what the person
/// means by their session, so only an explicit id reaches it.
#[test]
fn a_sub_agents_session_is_not_offered_as_the_persons() {
    let (path, _db) = database(
        "offered",
        "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER);
         CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
         CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
         INSERT INTO session VALUES ('main', NULL, 'Rename the handler', '/work/project', 1),
             ('task', 'main', 'List the call sites', '/work/project', 2);",
    );
    assert_eq!(ids(&found(&path, "/work/project", "", 30)), "main");
    assert_eq!(ids(&found(&path, "/work/project", "task", 1)), "task");
}

/// A long-running session is as recent as its last activity, like a
/// transcript file is as recent as its last write.
#[test]
fn sessions_rank_by_when_they_were_last_active() {
    let (path, _db) = database(
        "active",
        "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER);
         INSERT INTO session VALUES ('long-running', NULL, 'Started first, still going', '/work/project', 1000, 9000),
             ('quick', NULL, 'Started later, finished', '/work/project', 5000, 6000),
             ('untouched', NULL, 'Never updated', '/work/project', 3000, NULL);",
    );
    let all = found(&path, "/work/project", "", 30);
    assert_eq!(ids(&all), "long-running,quick,untouched");
    assert_eq!(all[0].found.recency, 9_000_000_000);
    assert_eq!(all[0].started, Some((1, 0)));
    assert_eq!(all[0].title, "Started first, still going");
    // A session never updated was last active when it started.
    assert_eq!(all[2].found.recency, 3_000_000_000);
}

/// The store keeps a sub-agent's session apart from its parent's. Loading
/// the parent brings them along, and only the ones that are sub-agents.
#[test]
fn a_sub_agents_session_loads_with_its_parent() {
    let (path, _db) = database(
        "nested",
        "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER);
         CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
         CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
         INSERT INTO session VALUES ('main', NULL, 'Main', '/work/project', 1), ('task', 'main', 'Task', '/work/project', 2),
             ('deeper', 'task', 'Deeper', '/work/project', 3), ('other', NULL, 'Other', '/work/project', 4);",
    );
    let input = by_id(&path, "main").load().unwrap();
    assert_eq!(agent_ids(&input.agents), "task");
    assert_eq!(input.agents[0].name, "Task");
    assert_eq!(input.agents[0].call, "");
    assert_eq!(input.agents[0].input.session["title"], "Task");
    assert_eq!(agent_ids(&input.agents[0].input.agents), "deeper");
    assert_eq!(agent_ids(&input.agents[0].input.agents[0].input.agents), "");
}

/// A session started in the home directory belongs to work done there, and
/// to no project beneath it. Only the name of the home directory is used
/// here: nothing in it is read.
#[test]
fn a_session_in_the_home_directory_belongs_to_home_only() {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = PathBuf::from(env::var_os(variable).unwrap_or_default());
    let (path, db) = database(
        "home",
        "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_created INTEGER)",
    );
    db.execute(
        "INSERT INTO session VALUES ('opencode', 'At home', ?, 1)",
        [home.to_string_lossy()],
    )
    .unwrap();
    let (at_home, problems) = listed(&path, &home, "", 30);
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(ids(&at_home), "opencode");
    let (beneath, problems) = listed(&path, &home.join("code").join("proj"), "", 30);
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(ids(&beneath), "");
}
