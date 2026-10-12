//! The dsh importer over its fixtures in `testdata/import`, and the finding
//! of sessions in a store laid out as dsh lays one out.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use slink_import::{Candidate, Input, Locations, import, load_file, read_lines, recent};

const TYPED: &str = r#"{"type":"user/message","seq":8,"time":1791451208534,"data":{"content":[{"type":"text","text":"Find the race in the upload test"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}"#;
const FALLBACK: &str = r#"{"type":"session/title","seq":12,"time":1791451208539,"data":{"title":"Find the race in the","messageSeqs":[8],"source":{"kind":"fallback"}}}"#;
const TITLED: &str = r#"{"type":"session/title","seq":18,"time":1791451208588,"data":{"title":"Upload test race","messageSeqs":[8],"source":{"kind":"provider","provider":"session-title-first-prompt-llm"}}}"#;
const REPLY: &str = r#"{"type":"assistant/message","seq":15,"time":1791451208583,"data":{"turn":1,"step":1,"message":{"role":"assistant","content":[{"type":"text","text":"Looking."}],"source":{"kind":"model","provider":"deepseek-official","model":"deepseek-flash"},"id":"m2"}},"surfaceOp":"append"}"#;
const STEP_START: &str =
    r#"{"type":"step/start","seq":6,"time":1791451208531,"data":{"turn":1,"step":1}}"#;

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);

/// A small dsh v4 log: a header and the events of one turn.
fn log_text(id: &str, cwd: &str, header: &str, events: &[&str]) -> String {
    let mut lines = vec![format!(
        r#"{{"type":"session","version":4,"id":"{id}","createdAt":1791451208304,"cwd":{},"isSeeded":false,"delegationDepth":0{header}}}"#,
        json!(cwd)
    )];
    lines.extend(events.iter().map(|event| (*event).to_owned()));
    lines.join("\n") + "\n"
}

/// A directory of one test's own, empty.
fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("dsh-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a log as it was at one moment.
fn write_log(file: &Path, body: impl AsRef<[u8]>, at: SystemTime) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, body).unwrap();
    let log = fs::File::options().write(true).open(file).unwrap();
    log.set_modified(at).unwrap();
}

/// A machine with a dsh store and no other agent's.
fn only_dsh(store: &Path) -> Locations {
    Locations {
        claude: PathBuf::new(),
        pi: PathBuf::new(),
        omp: PathBuf::new(),
        codex: PathBuf::new(),
        opencode: PathBuf::new(),
        hermes: PathBuf::new(),
        dsh: store.to_owned(),
    }
}

/// The sessions dsh recorded for a directory, and that nothing went wrong
/// reading them.
fn listed(store: &Path, cwd: &Path, id: &str) -> Vec<Candidate> {
    let (candidates, problems) = recent(&only_dsh(store), "dsh", cwd, id, 30);
    assert_eq!(problems, Vec::<String>::new());
    candidates
}

fn ids(candidates: &[Candidate]) -> Vec<&str> {
    candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect()
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .map(|entry| entry.unwrap().path())
        .collect();
    found.sort();
    found
}

/// The header of a session's log.
fn header(log: &Path) -> Value {
    slink_format::parse(&read_lines(log).unwrap()[0]).unwrap()
}

/// The logs in a store laid out as dsh lays one out.
fn logs(store: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for project in entries(store) {
        for session in entries(&project) {
            found.extend(entries(&session).into_iter().filter(|log| {
                let name = log.file_name().unwrap().to_string_lossy();
                name.starts_with("session.v") && name.contains(".jsonl")
            }));
        }
    }
    found
}

/// The session a dsh fixture is about: the one in its store that no other
/// session started.
fn fixture_session(store: &Path) -> PathBuf {
    let mut top: Vec<PathBuf> = logs(store)
        .into_iter()
        .filter(|log| header(log)["origin"] != "subagent")
        .collect();
    assert_eq!(top.len(), 1, "top-level sessions under {}", store.display());
    top.remove(0)
}

fn lines(log: &[&str]) -> Input {
    Input {
        lines: log.iter().map(|line| (*line).to_owned()).collect(),
        ..Input::default()
    }
}

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("dsh");
    assert!(fixtures.len() >= 4, "{} fixtures", fixtures.len());
    for fixture in fixtures {
        // A dsh fixture is a store laid out as dsh lays it out, and is read
        // by the code that reads a real one: the session that is not a
        // sub-agent's, its log as stored, its sub-agents' logs and the
        // attachment store beside them.
        let log = fixture_session(&fixture.join("sessions"));
        let (input, detected) = load_file(&log).unwrap();
        assert_eq!(detected, "dsh", "{}", fixture.display());
        common::check_golden("dsh", &fixture, &input);
    }
}

#[test]
fn sessions_are_found_where_dsh_keeps_them() {
    let root = scratch("found").join("sessions");
    let project = "/work/upload";
    let now = SystemTime::now();
    let dir = root.join("--work-upload--");
    write_log(
        &dir.join("session-top").join("session.v4.jsonl"),
        log_text(
            "session-top",
            project,
            "",
            &[TYPED, FALLBACK, STEP_START, REPLY, TITLED],
        ),
        now,
    );
    // dsh keeps a generation it has moved on from; the highest is the
    // session.
    write_log(
        &dir.join("session-top").join("session.v3.jsonl"),
        "{\"type\":\"session\",\"version\":3,\"id\":\"session-top\",\"createdAt\":1,\"cwd\":\"/elsewhere\",\"isSeeded\":false,\"delegationDepth\":0}\n",
        now + HOUR,
    );
    write_log(
        &dir.join("child-1").join("session.v4.jsonl"),
        log_text(
            "child-1",
            project,
            r#","parentSession":"session-top","origin":"subagent""#,
            &[TYPED],
        ),
        now + MINUTE,
    );
    write_log(
        &dir.join("session-untitled").join("session.v4.jsonl"),
        log_text("session-untitled", project, "", &[TYPED, FALLBACK]),
        now - HOUR,
    );
    // "/work:upload" is filed in the same directory; its header says it is
    // another project's.
    write_log(
        &dir.join("session-other").join("session.v4.jsonl"),
        log_text("session-other", "/work:upload", "", &[TYPED]),
        now,
    );
    write_log(
        &root
            .join("--work-elsewhere--")
            .join("session-far")
            .join("session.v4.jsonl"),
        log_text("session-far", "/work/elsewhere", "", &[TYPED]),
        now,
    );

    let found = listed(&root, Path::new(project), "");
    assert_eq!(ids(&found), ["session-top", "session-untitled"]);
    assert_eq!(found[0].name, "Upload test race");
    assert_eq!(found[0].title, "Upload test race");
    assert_eq!(found[0].prompt, "Find the race in the upload test");
    // The title dsh cuts from the prompt is not a title; the prompt stands
    // in.
    assert_eq!(found[1].name, "");
    assert_eq!(found[1].title, "Find the race in the upload test");
    assert_eq!(found[0].started, Some((1_791_451_208, 304_000_000)));
    assert_eq!(found[0].dir, Path::new(project));
    let read = found[0].found.load().unwrap();
    assert_eq!(
        read.lines.len(),
        6,
        "the highest generation is the one read"
    );
    assert!(read.lines[0].contains("\"version\":4"), "{}", read.lines[0]);
    assert_eq!(read.fallback, "session-top");
    // Only its id reaches a sub-agent's own session.
    assert_eq!(
        ids(&listed(&root, Path::new(project), "child-1")),
        ["child-1"]
    );
    assert_eq!(
        ids(&listed(&root, Path::new(project), "session-untitled")),
        ["session-untitled"]
    );
    assert!(listed(&root, Path::new(project), "session-far").is_empty());
    // From inside the project, its sessions are still the ones offered.
    let deeper = Path::new(project).join("src");
    assert_eq!(
        ids(&listed(&root, &deeper, "")),
        ["session-top", "session-untitled"]
    );
}

#[test]
fn a_session_lists_the_logs_of_its_sub_agents() {
    // The fixture's session started a sub-agent and then a fork of itself.
    let store = common::fixtures("dsh")
        .into_iter()
        .find(|fixture| fixture.ends_with("v4-sub-agents"))
        .unwrap()
        .join("sessions");
    let found = listed(&store, Path::new("/work/checks"), "");
    assert_eq!(
        ids(&found),
        ["session-431fac29-44de-4024-a816-7be59e5482b0"]
    );
    assert_eq!(found[0].title, "Shell check");
    let related: Vec<String> = found[0]
        .related
        .iter()
        .map(|log| header(log)["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        related,
        [
            "9161ed58-d47e-48af-b6de-c4ec4c4ad5f4",
            "3a456f9a-9e0c-4d3b-8bcf-a227a1dc039e"
        ]
    );
    // The fork began with a copy of its parent's history. The sub-agent in
    // that copy is the parent's, not one of its own.
    let fork = listed(
        &store,
        Path::new("/work/checks"),
        "3a456f9a-9e0c-4d3b-8bcf-a227a1dc039e",
    );
    assert_eq!(fork.len(), 1);
    assert!(fork[0].related.is_empty(), "{:?}", fork[0].related);
    assert!(fork[0].found.load().unwrap().agents.is_empty());
    let agents = found[0].found.load().unwrap().agents;
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].name, "Run the shell check");
    assert_eq!(agents[0].id, agents[0].input.fallback);
}

#[test]
fn a_dsh_log_is_told_from_a_pi_transcript() {
    let dir = scratch("sniff");
    let dsh = dir.join("dsh.jsonl");
    fs::write(
        &dsh,
        r#"{"type":"session","version":4,"id":"s","createdAt":1,"isSeeded":false,"delegationDepth":0}"#,
    )
    .unwrap();
    assert_eq!(load_file(&dsh).unwrap().1, "dsh");
    let pi = dir.join("pi.jsonl");
    fs::write(
        &pi,
        r#"{"type":"session","version":3,"id":"s","timestamp":"2026-10-07T10:00:00.000Z","cwd":"/w"}"#,
    )
    .unwrap();
    assert_eq!(load_file(&pi).unwrap().1, "pi");
}

#[test]
fn a_compressed_log_cut_off_mid_write_is_read() {
    let store = common::fixtures("dsh")
        .into_iter()
        .find(|fixture| fixture.ends_with("v4-run"))
        .unwrap()
        .join("sessions");
    let compressed: Vec<PathBuf> = logs(&store)
        .into_iter()
        .filter(|log| log.to_string_lossy().ends_with(".jsonl.zstd"))
        .collect();
    assert_eq!(compressed.len(), 1, "the compressed fixture");
    let whole = read_lines(&compressed[0]).unwrap();
    assert!(whole.len() >= 20, "the fixture's frames: {}", whole.len());

    let raw = fs::read(&compressed[0]).unwrap();
    let root = scratch("cut").join("sessions");
    let cut = root
        .join("--work-greeting--")
        .join("session-cut")
        .join("session.v4.jsonl.zstd");
    fs::create_dir_all(cut.parent().unwrap()).unwrap();
    fs::write(&cut, &raw[..raw.len() - 40]).unwrap();
    // What was complete before the cut is kept as it was.
    let kept = read_lines(&cut).unwrap();
    assert!(
        !kept.is_empty() && kept.len() < whole.len(),
        "{}",
        kept.len()
    );
    assert_eq!(kept, whole[..kept.len()]);
    // Its header is still read, which is what finds it for its project.
    let found = listed(&root, Path::new("/work/greeting"), "");
    assert_eq!(
        ids(&found),
        ["session-13acf23d-ac96-43ac-add4-6c1de6b47e5a"]
    );
    assert_eq!(found[0].found.load().unwrap().lines, kept);

    // A log cut before its first line ended has nothing to give.
    fs::write(&cut, &raw[..20]).unwrap();
    let error = read_lines(&cut).unwrap_err().to_string();
    assert_eq!(error, format!("{}: unexpected EOF", cut.display()));
    assert!(listed(&root, Path::new("/work/greeting"), "").is_empty());
}

#[test]
fn a_log_is_taken_for_compressed_by_its_name() {
    let store = common::fixtures("dsh")
        .into_iter()
        .find(|fixture| fixture.ends_with("v4-run"))
        .unwrap()
        .join("sessions");
    let compressed = logs(&store).remove(0);
    let (packed, text) = (
        fs::read(&compressed).unwrap(),
        read_lines(&compressed).unwrap().join("\n") + "\n",
    );
    let root = scratch("named").join("sessions");
    let project = root.join("--work-greeting--");
    let now = SystemTime::now();
    // Each is read whole whatever it is called: that goes by its first bytes.
    let unnamed = project.join("unnamed").join("session.v4.jsonl");
    write_log(&unnamed, &packed, now);
    let misnamed = project.join("misnamed").join("session.v4.jsonl.zstd");
    write_log(&misnamed, &text, now);
    assert_eq!(
        read_lines(&unnamed).unwrap(),
        read_lines(&compressed).unwrap()
    );
    assert_eq!(
        read_lines(&misnamed).unwrap(),
        read_lines(&compressed).unwrap()
    );
    // Its header alone is read by the name, as Go reads it, so neither is
    // found for its project. dsh writes neither.
    assert!(listed(&root, Path::new("/work/greeting"), "").is_empty());
    let (plain, named) = (
        project.join("plain").join("session.v4.jsonl"),
        project.join("named").join("session.v4.jsonl.zstd"),
    );
    write_log(&plain, &text, now - HOUR);
    write_log(&named, &packed, now);
    assert_eq!(
        ids(&listed(&root, Path::new("/work/greeting"), "")).len(),
        2
    );
}

#[test]
fn a_format_dsh_does_not_write_is_refused() {
    for version in ["2", "5"] {
        let header = format!(
            r#"{{"type":"session","version":{version},"id":"s","createdAt":1,"isSeeded":false,"delegationDepth":0}}"#
        );
        let error = import("dsh", &lines(&[&header])).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "import failed: dsh session format v{version} is not one slink reads (v3 and v4 are)"
            )
        );
    }
    // A log without a header is not a session.
    let error = import("dsh", &lines(&[TYPED])).unwrap_err().to_string();
    assert_eq!(
        error,
        "import failed: not a dsh session log: it does not start with a session header"
    );
}

/// v3, which the dsh 0.1 releases wrote, keeps a tool's result inside one
/// block of a user-role message. This is that shape as dsh's own migration
/// to v4 describes it, not a log from a run.
#[test]
fn a_v3_tool_result_is_read() {
    let run = import(
        "dsh",
        &lines(&[
            r#"{"type":"session","version":3,"id":"s","createdAt":1791451208304,"cwd":"/work/upload","isSeeded":false,"delegationDepth":0}"#,
            TYPED,
            STEP_START,
            REPLY,
            r#"{"type":"tool/call","seq":16,"time":1791451208584,"data":{"turn":1,"step":1,"callId":"call-1","name":"bash","arguments":"{\"command\":\"ls\"}"}}"#,
            r#"{"type":"tool/result","seq":19,"time":1791451208601,"data":{"turn":1,"step":1,"message":{"role":"user","source":{"kind":"tool","callId":"call-1"},"content":[{"type":"tool-result","toolCallId":"call-1","content":[{"type":"text","text":"ls: no such directory"}],"isError":true}],"id":"m3"}},"sourceEventSeqs":[16],"surfaceOp":"append"}"#,
        ]),
    )
    .unwrap()
    .unwrap();
    let tool = &run["spans"][2];
    assert_eq!(tool["type"], "tool_call");
    assert_eq!(tool["status"], "error");
    assert_eq!(
        tool["output"],
        json!({"result": "ls: no such directory", "is_error": true})
    );
    // Arguments are read from the JSON text dsh keeps.
    assert_eq!(tool["input"]["arguments"], json!({"command": "ls"}));
    assert_eq!(tool["parent_id"], "s1");
    assert_eq!(tool["ended_at"], "2026-10-08T09:20:08.601Z");
    assert_eq!(run["metadata"]["session_format"], 3);
}

#[test]
fn what_cannot_be_shown_is_kept_and_who_wrote_what() {
    let image = format!(
        r#"{{"type":"image","attachment":{{"attachmentId":"sha256:{}","mediaType":"image/png","bytes":73}}}}"#,
        "ab".repeat(32)
    );
    let file = format!(
        r#"{{"type":"file","attachment":{{"attachmentId":"sha256:{}","name":"notes.txt"}}}}"#,
        "cd".repeat(32)
    );
    let typed = format!(
        r#"{{"type":"user/message","seq":1,"time":1791451208534,"data":{{"content":[{{"type":"text","text":"What is in the picture?"}},{image},{file}],"source":{{"kind":"user"}},"role":"user","id":"m1"}},"surfaceOp":"append"}}"#
    );
    let run = import(
        "dsh",
        &lines(&[
            r#"{"type":"session","version":4,"id":"s","createdAt":1791451208304,"isSeeded":false,"delegationDepth":0}"#,
            &typed,
            r#"{"type":"user/message","seq":2,"time":1791451208535,"data":{"content":[{"type":"text","text":"Earlier turns, condensed."}],"source":{"kind":"compact-checkpoint","compactionId":"c1"},"role":"user","id":"m2"},"surfaceOp":"append"}"#,
            STEP_START,
            r#"{"type":"assistant/message","seq":15,"time":1791451208583,"data":{"turn":1,"step":1,"interrupted":true,"message":{"role":"assistant","content":[{"type":"text","text":"It sh"}],"source":{"kind":"model"},"id":"m3"}},"surfaceOp":"append"}"#,
        ]),
    )
    .unwrap()
    .unwrap();
    let call = &run["spans"][1];
    let messages = &call["input"]["messages"];
    // Only what the person wrote is theirs.
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "system");
    // The image's bytes are not at hand and a file is never inlined: both
    // stay as recorded.
    let parts = &messages[0]["content"];
    assert_eq!(
        parts[1],
        json!({"type": "data", "data": slink_format::parse(&image).unwrap()})
    );
    assert_eq!(parts[1]["data"]["attachment"]["bytes"], 73);
    assert_eq!(parts[2]["type"], "data");
    // An interrupted reply, and a model the log does not name.
    assert_eq!(call["metadata"], json!({"interrupted": true}));
    assert_eq!(call["model"], json!({"id": "unknown"}));
    assert_eq!(run["name"], "What is in the picture?");
}

#[test]
fn a_fork_leaves_out_the_history_it_began_with() {
    let header = r#"{"type":"session","version":4,"id":"fork","createdAt":1791451208304,"isSeeded":true,"origin":"subagent","delegationDepth":1}"#;
    let log = [
        header,
        TYPED,
        STEP_START,
        REPLY,
        r#"{"type":"session/end-seed","seq":27,"time":1791451208700,"data":{"inherited":true}}"#,
        r#"{"type":"user/message","seq":28,"time":1791451208800,"data":{"content":[{"type":"text","text":"The fork's own task"}],"source":{"kind":"user"}}}"#,
    ];
    // Read as a session of its own, it is all there.
    let own = import("dsh", &lines(&log)).unwrap().unwrap();
    assert_eq!(own["spans"].as_array().unwrap().len(), 3);
    assert_eq!(own["metadata"].get("inherited_events_skipped"), None);
    // Nested in the session that started it, only its own work is.
    let mut parent = lines(&[
        r#"{"type":"session","version":4,"id":"top","createdAt":1791451208304,"isSeeded":false}"#,
        TYPED,
        REPLY,
        r#"{"type":"tool/call","seq":16,"time":1791451208584,"data":{"callId":"call-1","name":"subagent_fork","arguments":"{}"}}"#,
        r#"{"type":"subagent/catalog","seq":17,"time":1791451208590,"data":{"childId":"fork","label":"Forked"}}"#,
        r#"{"type":"tool/result","seq":18,"time":1791451208900,"data":{"message":{"toolCallId":"call-1","content":[{"type":"text","text":"done"}]}}}"#,
    ]);
    parent.agents = vec![slink_import::Agent {
        id: "fork".into(),
        name: "Forked".into(),
        input: lines(&log),
        ..Default::default()
    }];
    let run = import("dsh", &parent).unwrap().unwrap();
    let spans = run["spans"].as_array().unwrap();
    let shown: Vec<(&str, &str, &str)> = spans
        .iter()
        .map(|span| {
            (
                span["id"].as_str().unwrap(),
                span["parent_id"].as_str().unwrap_or(""),
                span["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("root", "", "Find the race in the upload test"),
            ("s1", "root", "turn"),
            ("s2", "s1", "subagent_fork"),
            ("a1", "s2", "Forked"),
            ("a1.s1", "a1", "trailing message(s) — no assistant reply"),
        ]
    );
    assert_eq!(run["metadata"]["sub_agents"], 1);
}
