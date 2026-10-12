//! The Codex importer over its fixtures in `testdata/import`, and the
//! sessions it finds in a store laid out as Codex lays one out.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use slink_import::{Candidate, Input, Locations, import, load_file, read_lines, recent};

const CWD: &str = "/work/project";

/// An empty directory of this test's own.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a rollout as if it was last written at `at`.
fn write(file: &Path, body: &str, at: SystemTime) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, body).unwrap();
    let written = fs::File::options().write(true).open(file).unwrap();
    written.set_modified(at).unwrap();
}

/// Codex's store and no other harness's, so nothing is read from the stores
/// of the machine the tests run on.
fn only_codex(store: &Path) -> Locations {
    Locations {
        claude: PathBuf::new(),
        pi: PathBuf::new(),
        omp: PathBuf::new(),
        codex: store.to_owned(),
        opencode: PathBuf::new(),
        hermes: PathBuf::new(),
        dsh: PathBuf::new(),
    }
}

fn listed(store: &Path, cwd: &Path, id: &str, limit: usize) -> Vec<Candidate> {
    let (candidates, problems) = recent(&only_codex(store), "codex", cwd, id, limit);
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

/// One line of a rollout.
fn item(stamp: &str, payload: Value) -> String {
    json!({"timestamp": stamp, "type": "response_item", "payload": payload}).to_string()
}

fn imported(lines: Vec<String>, fallback: &str) -> Value {
    let input = Input {
        lines,
        fallback: fallback.to_owned(),
        ..Input::default()
    };
    import("codex", &input).unwrap().unwrap()
}

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("codex");
    assert!(fixtures.len() >= 5, "{} fixtures", fixtures.len());
    for fixture in fixtures {
        // Read as a file someone named, so a sub-agent's rollout beside the
        // session's own is found by the code that finds real ones.
        let (mut input, detected) = load_file(&fixture.join("input.session.jsonl")).unwrap();
        assert_eq!(detected, "codex", "{}", fixture.display());
        let spawned = usize::from(fixture.ends_with("sub-agent"));
        assert_eq!(input.agents.len(), spawned, "{}", fixture.display());
        input.fallback = "golden".into();
        common::check_golden("codex", &fixture, &input);
    }
}

#[test]
fn reasoning_is_text_only_when_it_can_be_read() {
    let secret = "CIPHERTEXT_MUST_NOT_ENTER_TEXT";
    for (name, mut payload, said, reason) in [
        (
            "summary",
            json!({"summary": [{"text": "Readable summary"}], "encrypted_content": secret}),
            "Readable summary",
            "",
        ),
        (
            "content after empty summary",
            json!({"summary": [], "content": [{"text": "Readable content"}]}),
            "Readable content",
            "",
        ),
        (
            "encrypted only",
            json!({"summary": [], "encrypted_content": secret}),
            "",
            "encrypted",
        ),
        (
            "no recorded text",
            json!({"summary": []}),
            "",
            "not_recorded",
        ),
    ] {
        payload["type"] = json!("reasoning");
        let run = imported(vec![item("2026-09-11T00:00:00Z", payload)], name);
        let part = &run["spans"][1]["output"]["messages"][0]["content"][0];
        assert_eq!(part["type"], "thinking", "{name}");
        assert_eq!(part["text"], said, "{name}");
        if reason.is_empty() {
            assert!(part.get("unavailable").is_none(), "{name}: {part}");
        } else {
            assert_eq!(part["unavailable"], true, "{name}: {part}");
            assert_eq!(part["reason"], reason, "{name}: {part}");
        }
        // Neither the ciphertext nor a made-up placeholder is readable text.
        let written = slink_format::encode(&run);
        assert!(!written.contains(secret), "{name}");
        assert!(!written.contains("[reasoning]"), "{name}");
    }
}

#[test]
fn an_unavailable_reasoning_event_is_kept_without_an_answer() {
    let run = imported(
        vec![
            item(
                "2026-09-11T00:00:00Z",
                json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "An interrupted task"}],
                }),
            ),
            item(
                "2026-09-11T00:00:01Z",
                json!({
                    "type": "reasoning",
                    "summary": [],
                    "encrypted_content": "CIPHERTEXT_MUST_NOT_ENTER_TEXT",
                }),
            ),
        ],
        "Reasoning availability",
    );
    assert_eq!(slink_format::validate(&run), Vec::<String>::new());
    let written = slink_format::encode(&run);
    assert!(written.contains(r#""unavailable":true"#), "{written}");
    assert!(!written.contains("CIPHERTEXT_MUST_NOT_ENTER_TEXT"));
    assert!(!written.contains("[reasoning]"));
}

#[test]
fn a_rollout_is_recognized_from_response_items_alone() {
    // A rollout whose header came late or was stripped still opens with
    // response items, and those are Codex's.
    let file = scratch("codex-recognized").join("rollout.jsonl");
    let lines = [
        r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#,
        r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
    ];
    write(&file, &lines.join("\n"), SystemTime::now());
    let (input, detected) = load_file(&file).unwrap();
    assert_eq!(detected, "codex");
    assert_eq!(input.lines, lines);
    assert_eq!(input.fallback, "rollout");
}

#[test]
fn the_sessions_of_a_project_are_listed_and_an_id_reaches_an_older_one() {
    let store = scratch("codex-recent");
    let now = SystemTime::now();
    let hour = Duration::from_secs(3600);
    // A header names its session by `id`, and an older one by `session_id`.
    for key in ["id", "session_id"] {
        let body = [
            format!(r#"{{"type":"session_meta","payload":{{"{key}":"{key}","cwd":"{CWD}"}}}}"#),
            r#"{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"<environment_context>setup</environment_context>"}]}}"#.to_owned(),
            r#"{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"Review the health endpoint"}]}}"#.to_owned(),
        ];
        write(
            &store.join(format!("{key}.jsonl")),
            &format!("{}\n", body.join("\n")),
            now - 2 * hour,
        );
    }
    write(
        &store.join("other.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"other","cwd":"/work/project-other"}}"#,
        now + hour,
    );

    // From a subdirectory, the project's sessions and no other project's.
    let all = listed(&store, &Path::new(CWD).join("src"), "", 30);
    assert_eq!(ids(&all), "id,session_id");
    for candidate in &all {
        assert_eq!(candidate.found.harness, "codex");
        assert_eq!(candidate.title, "Review the health endpoint");
        assert_eq!(candidate.prompt, "Review the health endpoint");
        assert_eq!(candidate.name, "");
        assert_eq!(candidate.dir, Path::new(CWD));
        assert!(candidate.related.is_empty());
        let file = store.join(format!("{}.jsonl", candidate.id));
        assert_eq!(candidate.file.as_deref(), Some(file.as_path()));
    }
    // An id is matched before the limit a picker shows.
    let old = listed(&store, Path::new(CWD), "session_id", 1);
    assert_eq!(ids(&old), "session_id");
    assert_eq!(ids(&listed(&store, Path::new(CWD), "", 1)), "id");
    assert_eq!(ids(&listed(&store, Path::new(CWD), "no-such-id", 30)), "");
}

#[test]
fn a_session_with_no_prompt_is_listed_by_its_id() {
    let store = scratch("codex-untitled");
    let header = format!(
        r#"{{"timestamp":"2026-10-03T14:00:00.250Z","type":"session_meta","payload":{{"id":"quiet","cwd":"{CWD}"}}}}"#
    );
    write(
        &store.join("rollout-quiet.jsonl"),
        &header,
        SystemTime::now(),
    );
    // A header with no id at all: the rollout's file names the session.
    let unnamed = format!(r#"{{"type":"session_meta","payload":{{"cwd":"{CWD}"}}}}"#);
    write(
        &store.join("rollout-unnamed.jsonl"),
        &unnamed,
        SystemTime::now() - Duration::from_secs(60),
    );
    let all = listed(&store, Path::new(CWD), "", 30);
    assert_eq!(ids(&all), "quiet,rollout-unnamed");
    assert_eq!(all[0].title, "quiet");
    assert_eq!(
        all[0].started,
        slink_format::instant("2026-10-03T14:00:00.250Z")
    );
    assert_eq!(all[1].title, "rollout-unnamed");
    assert_eq!(all[1].started, None);
}

// Codex records a sub-agent's work as a rollout of its own, beside the
// session that started it and with the same working directory. It is not
// what the person means by their session, so only an explicit id reaches it.
#[test]
fn a_sub_agents_rollout_is_not_offered_as_the_persons_session() {
    let store = scratch("codex-auxiliary").join("sessions");
    let now = SystemTime::now();
    let rollout = |id: &str, source: &str, prompt: &str, at: SystemTime| {
        let body = [
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","session_id":"main","cwd":"{CWD}","source":{source}}}}}"#
            ),
            format!(
                r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{prompt}"}}]}}}}"#
            ),
        ];
        write(
            &store.join("2026/10/03").join(format!("rollout-{id}.jsonl")),
            &format!("{}\n", body.join("\n")),
            at,
        );
    };
    rollout(
        "main",
        r#""cli""#,
        "Rename the handler",
        now - Duration::from_secs(3600),
    );
    rollout(
        "scout",
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"main","depth":1}}}"#,
        "List the call sites",
        now - Duration::from_secs(60),
    );
    rollout(
        "review",
        r#"{"subagent":"review"}"#,
        "Review the diff",
        now - Duration::from_secs(30),
    );
    rollout(
        "memory",
        r#"{"internal":"memory_consolidation"}"#,
        "Consolidate",
        now,
    );

    let own = listed(&store, Path::new(CWD), "", 30);
    assert_eq!(ids(&own), "main");
    assert_eq!(own[0].title, "Rename the handler");
    let input = own[0].found.load().unwrap();
    assert!(
        input.lines[1].contains("Rename the handler"),
        "{:?}",
        input.lines
    );
    // An explicit id must still reach a sub-agent.
    assert_eq!(ids(&listed(&store, Path::new(CWD), "scout", 1)), "scout");
    assert_eq!(ids(&listed(&store, Path::new(CWD), "memory", 1)), "memory");
}

// Codex keeps a sub-agent's rollout apart from its parent's. Loading the
// parent brings its sub-agents along, and only the ones that are its own.
#[test]
fn a_sessions_sub_agents_load_with_it() {
    let root = scratch("codex-agents").join("sessions");
    let rollout = |day: &str, id: &str, header: &str| {
        let file = root
            .join("2026/10")
            .join(day)
            .join(format!("rollout-{id}.jsonl"));
        let body = [
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{CWD}"{header}}}}}"#),
            format!(
                r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{id} at work"}}]}}}}"#
            ),
        ];
        write(&file, &format!("{}\n", body.join("\n")), SystemTime::now());
        file
    };
    let spawned = |parent: &str, path: &str| {
        format!(
            r#","parent_thread_id":"{parent}","agent_path":"{path}","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent}","depth":1}}}}}}"#
        )
    };
    let main = rollout("08", "main", r#","source":"cli""#);
    // A sub-agent's rollout can land in the next day's directory.
    let child = rollout("09", "child", &spawned("main", "/root/checker"));
    let grandchild = rollout(
        "09",
        "grandchild",
        &spawned("child", "/root/checker/prober"),
    );
    rollout("08", "unrelated", r#","source":"cli""#);

    let (input, detected) = load_file(&main).unwrap();
    assert_eq!(detected, "codex");
    let [agent] = input.agents.as_slice() else {
        panic!("agents {:?}", input.agents);
    };
    assert_eq!(agent.id, "/root/checker");
    assert_eq!(agent.name, "checker");
    assert_eq!(agent.input.fallback, "rollout-child");
    let [deeper] = agent.input.agents.as_slice() else {
        panic!("agents of the sub-agent {:?}", agent.input.agents);
    };
    assert_eq!(deeper.id, "/root/checker/prober");
    assert_eq!(deeper.name, "prober");

    // Each is nested in the session that started it.
    let run = import("codex", &input).unwrap().unwrap();
    assert_eq!(slink_format::validate(&run), Vec::<String>::new());
    assert_eq!(run["metadata"]["sub_agents"], 1);
    let names: Vec<&str> = run["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|span| span["type"] == "agent")
        .map(|span| span["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["rollout-main", "checker", "prober"]);
    assert!(!slink_format::encode(&run).contains("unrelated at work"));

    let found = listed(&root, Path::new(CWD), "main", 1);
    assert_eq!(ids(&found), "main");
    assert_eq!(found[0].related, [child, grandchild]);
}

// A sub-agent's thread opens with a copy of its parent's history. Opened on
// its own, it is that agent's work and names its own thread.
#[test]
fn a_sub_agents_thread_reads_from_where_its_own_history_starts() {
    let fixture = common::fixtures("codex")
        .into_iter()
        .find(|fixture| fixture.ends_with("sub-agent"))
        .unwrap();
    let rollouts: Vec<PathBuf> = fs::read_dir(&fixture)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|file| {
            let name = file.file_name().unwrap().to_string_lossy();
            name.starts_with("rollout-") && name.ends_with(".jsonl")
        })
        .collect();
    let [rollout] = rollouts.as_slice() else {
        panic!("{rollouts:?}");
    };
    let run = imported(read_lines(rollout).unwrap(), "thread");
    let written = slink_format::encode(&run);
    assert!(
        run["metadata"]["inherited_items_skipped"].is_u64(),
        "{}",
        run["metadata"]
    );
    assert!(
        !written.contains("DELEGATE: have a sub-agent"),
        "the parent's history was replayed as the sub-agent's"
    );
    assert!(written.contains("NEW_TASK"));
    assert!(written.contains("The shell works"));
}

// A session started in the home directory belongs to work done there, and
// to no project beneath it.
#[test]
fn a_session_started_at_home_belongs_to_home_only() {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let Some(home) = std::env::var_os(name).filter(|home| !home.is_empty()) else {
        return;
    };
    let home = PathBuf::from(home);
    let store = scratch("codex-home");
    let header = json!({"type": "session_meta", "payload": {"id": "codex", "cwd": home}});
    write(
        &store.join("rollout-codex.jsonl"),
        &format!("{header}\n"),
        SystemTime::now(),
    );
    assert_eq!(ids(&listed(&store, &home, "", 30)), "codex");
    assert_eq!(ids(&listed(&store, &home.join("code/proj"), "", 30)), "");
}
