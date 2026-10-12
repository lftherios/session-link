//! The pi and omp importer over its fixtures in `testdata/import`, and the
//! sessions of both found in stores laid out as each lays its own out.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use slink_import::{Candidate, Input, Locations, import, load_file, recent};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn every_fixture_reproduces_its_golden() {
    for (harness, at_least) in [("pi", 4), ("omp", 3)] {
        let fixtures = common::fixtures(harness);
        assert!(fixtures.len() >= at_least, "{} fixtures", fixtures.len());
        for fixture in fixtures {
            // Read as a file someone named, so the sub-agents kept beside the
            // transcript are found by the code that finds real ones.
            let (mut input, detected) = load_file(&fixture.join("input.session.jsonl")).unwrap();
            assert_eq!(detected, harness, "{}", fixture.display());
            input.fallback = "golden".into();
            // A fixture can carry the blob store its transcript refers to.
            input.blobs = fixture.join("blobs");
            common::check_golden(harness, &fixture, &input);
        }
    }
}

/// An empty directory of the test's own.
fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pi-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(file: &Path, body: &str, at: SystemTime) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, body).unwrap();
    let written = fs::File::options().write(true).open(file).unwrap();
    written.set_modified(at).unwrap();
}

/// Stores under `root` for the harnesses named here, and none for the rest:
/// discovery reads nothing of the machine the test runs on.
fn stores(root: &Path) -> Locations {
    Locations {
        claude: root.join("claude"),
        pi: root.join("pi"),
        omp: root.join("omp"),
        codex: PathBuf::new(),
        opencode: PathBuf::new(),
        hermes: PathBuf::new(),
        dsh: PathBuf::new(),
    }
}

fn listed(stores: &Locations, harness: &str, cwd: &str, id: &str, limit: usize) -> Vec<Candidate> {
    let (candidates, problems) = recent(stores, harness, Path::new(cwd), id, limit);
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

#[test]
fn recent_sessions_are_listed_newest_first_and_stay_the_ones_chosen() {
    let root = scratch("recent");
    let stores = stores(&root);
    let now = SystemTime::now();
    write(
        &stores.claude.join("-work-project").join("same.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"Debug the failing request\"}}\n",
        now - HOUR,
    );
    let project = stores.pi.join("--work-project--");
    write(
        &project.join("timestamp.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"same\",\"cwd\":\"/work/project\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Compare the competitors\"}]}}\n",
        ),
        now,
    );
    write(
        &project.join("earlier.jsonl"),
        "{\"type\":\"session\",\"id\":\"one\"}",
        now - 2 * HOUR,
    );
    write(
        &stores.pi.join("--work-project-other--").join("other.jsonl"),
        "{\"type\":\"session\",\"id\":\"other\",\"cwd\":\"/work/project-other\"}\n",
        now + HOUR,
    );

    let all = listed(&stores, "", "/work/project/src", "", 30);
    assert_eq!(all.len(), 3, "{}", ids(&all));
    assert_eq!(all[0].found.harness, "pi");
    assert_eq!(
        all[0].id, "same",
        "the id in the header, not the file's name"
    );
    assert_eq!(all[0].title, "Compare the competitors");
    assert_eq!(all[0].dir, Path::new("/work/project"));
    assert_eq!(all[1].title, "Debug the failing request");
    // Nothing but a header: listed under its id.
    assert_eq!((all[2].id.as_str(), all[2].title.as_str()), ("one", "one"));
    // The same id in two harnesses stays ambiguous; the caller must ask for
    // a choice.
    assert_eq!(listed(&stores, "", "/work/project", "same", 1).len(), 2);
    // An id named outright is matched before the limit.
    assert_eq!(
        ids(&listed(&stores, "pi", "/work/project", "one", 1)),
        "one"
    );
    assert_eq!(ids(&listed(&stores, "pi", "/work/project", "", 1)), "same");
    assert_eq!(ids(&listed(&stores, "pi", "/work/project", "other", 1)), "");

    // A session written later does not change which one was chosen.
    write(
        &project.join("new.jsonl"),
        "{\"type\":\"session\",\"id\":\"new\"}",
        now + HOUR,
    );
    let chosen = all[0].found.load().unwrap();
    assert!(chosen.lines[0].contains("\"same\""), "{:?}", chosen.lines);
    assert_eq!(chosen.fallback, "timestamp");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_pi_session_is_listed_under_the_name_the_person_gave_it() {
    let root = scratch("named");
    let stores = stores(&root);
    let prompt = r#"{"type":"message","id":"m1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"Find the race"}]}}"#;
    let header = |id: &str| format!(r#"{{"type":"session","id":"{id}","cwd":"/work/project"}}"#);
    let info = |id: &str, parent: &str, name: &str| {
        format!(r#"{{"type":"session_info","id":"{id}","parentId":"{parent}","name":"{name}"}}"#)
    };
    let project = stores.pi.join("--work-project--");
    write(
        &project.join("named.jsonl"),
        &[
            header("named"),
            prompt.to_owned(),
            info("i1", "m1", "First name"),
            info("i2", "i1", " Upload test race "),
        ]
        .map(|line| line + "\n")
        .concat(),
        SystemTime::now(),
    );
    write(
        &project.join("cleared.jsonl"),
        &[
            header("cleared"),
            prompt.to_owned(),
            info("i1", "m1", "Old name"),
            info("i2", "i1", ""),
        ]
        .map(|line| line + "\n")
        .concat(),
        SystemTime::now() - MINUTE,
    );

    let listed = listed(&stores, "pi", "/work/project", "", 30);
    assert_eq!(ids(&listed), "named,cleared");
    assert_eq!(listed[0].name, "Upload test race");
    assert_eq!(listed[0].title, "Upload test race");
    assert_eq!(listed[0].prompt, "Find the race");
    assert_eq!(listed[1].name, "", "a cleared name stays cleared");
    assert_eq!(listed[1].title, "Find the race", "and the prompt stands in");
    // The import gives each the same name the listing showed.
    for candidate in &listed {
        let run = import("pi", &candidate.found.load().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(run["name"], candidate.title.as_str());
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_omp_session_is_listed_with_its_title_and_its_sub_agents() {
    let root = scratch("omp");
    let stores = stores(&root);
    let transcript = |id: &str, title: &str, prompt: &str| {
        [
            format!(r#"{{"type":"title","v":1,"title":"{title}","updatedAt":"2026-10-07T10:00:00.000Z","pad":"    "}}"#),
            format!(r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-10-07T10:00:00.000Z","cwd":"/work/elsewhere"}}"#),
            format!(r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-07T10:00:01.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{prompt}"}}]}}}}"#),
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-10-07T10:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"On it."}],"provider":"mock","model":"mock-model"}}"#.to_owned(),
        ]
        .map(|line| line + "\n")
        .concat()
    };
    // Neither under home nor under the temp directory, so omp names the
    // project's directory by its whole path.
    let project = stores.omp.join("--work-elsewhere--");
    let now = SystemTime::now();
    write(
        &project.join("a.jsonl"),
        &transcript("titled", "Upload test race", "Find the race"),
        now,
    );
    // A sub-agent's transcript sits in a directory beside its session's
    // file, and is no session of the person's.
    let scout = project.join("a").join("Scout.jsonl");
    write(
        &scout,
        &transcript("sub-agent", "", "Look around"),
        now + MINUTE,
    );
    write(
        &project.join("b.jsonl"),
        &transcript("untitled", "", "Then fix it"),
        now - HOUR,
    );

    let listed = listed(&stores, "omp", "/work/elsewhere/src", "", 30);
    assert_eq!(ids(&listed), "titled,untitled");
    let first = &listed[0];
    assert_eq!(first.found.harness, "omp");
    assert_eq!(
        (first.name.as_str(), first.title.as_str()),
        ("Upload test race", "Upload test race")
    );
    assert_eq!(first.prompt, "Find the race");
    assert_eq!(first.dir, Path::new("/work/elsewhere"));
    assert_eq!(first.related, [scout]);
    assert_eq!(
        first.started,
        slink_format::instant("2026-10-07T10:00:00.000Z")
    );
    let untitled = &listed[1];
    assert_eq!(
        (untitled.name.as_str(), untitled.title.as_str()),
        ("", "Then fix it"),
        "an untitled session falls back to its prompt"
    );
    assert!(untitled.related.is_empty());

    // The session is read with its sub-agent, whose work is nested in it.
    let input: Input = first.found.load().unwrap();
    assert_eq!(input.agents.len(), 1);
    assert_eq!(input.agents[0].id, "Scout");
    let run = import("omp", &input).unwrap().unwrap();
    assert_eq!(run["name"], "Upload test race");
    assert_eq!(run["metadata"]["sub_agents"], 1);
    assert_eq!(run["source"]["label"], "omp-import@0.1.0");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_omp_transcript_is_told_from_a_pi_one() {
    let root = scratch("sniff");
    let header = r#"{"type":"session","version":3,"id":"s","cwd":"/work/project"}"#;
    let title =
        r#"{"type":"title","v":1,"title":"","updatedAt":"2026-10-07T10:00:00.000Z","pad":"  "}"#;
    let now = SystemTime::now();
    write(
        &root.join("omp.jsonl"),
        &format!("{title}\n{header}\n"),
        now,
    );
    write(&root.join("pi.jsonl"), &format!("{header}\n"), now);
    assert_eq!(load_file(&root.join("omp.jsonl")).unwrap().1, "omp");
    assert_eq!(load_file(&root.join("pi.jsonl")).unwrap().1, "pi");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_image_is_shown_only_when_its_bytes_can_be_found() {
    let root = scratch("images");
    let blobs = root.join("blobs");
    let stored = "b".repeat(64);
    let missing = "c".repeat(64);
    // The first bytes of a PNG.
    fs::create_dir_all(&blobs).unwrap();
    fs::write(blobs.join(&stored), b"\x89PNG\r\n\x1a\n").unwrap();
    let image = |data: &str| format!(r#"{{"type":"image","data":"{data}"}}"#);
    let content = [
        image(&format!("blob:sha256:{stored}")),
        image(&format!("blob:sha256:{missing}")),
        image("blob:sha256:../blobs/x"),
        // No label, and too short a head to be told by.
        image("iVBORw"),
        image("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"),
    ]
    .join(",");
    let lines = vec![
        r#"{"type":"session","id":"s","timestamp":"2026-10-07T10:00:00.000Z"}"#.to_owned(),
        format!(
            r#"{{"type":"message","id":"m1","parentId":null,"message":{{"role":"user","content":[{content}]}}}}"#
        ),
    ];
    let input = Input {
        lines,
        fallback: "images".into(),
        blobs,
        ..Input::default()
    };
    let run = import("omp", &input).unwrap().unwrap();
    assert_eq!(slink_format::validate(&run), Vec::<String>::new());
    let parts = run["spans"][1]["input"]["messages"][0]["content"]
        .as_array()
        .unwrap();
    let kinds: Vec<&str> = parts
        .iter()
        .map(|part| part["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["image", "data", "data", "data", "image"]);
    assert_eq!(parts[0]["mime"], "image/png");
    // What cannot be shown is kept as it was recorded.
    assert_eq!(parts[1]["data"]["data"], format!("blob:sha256:{missing}"));
    assert_eq!(run["name"], "images", "no text to name it after");
    fs::remove_dir_all(&root).unwrap();
}
