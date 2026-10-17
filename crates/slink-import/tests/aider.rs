//! The Aider importer over its fixtures in `testdata/import`, and the runs
//! of a chat history as the sessions of a project.
//!
//! Aider writes local clock times. Every test here reads them as UTC,
//! wherever it runs.

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{ids, listed, nowhere, scratch, write};
use serde_json::Value;
use slink_import::{Candidate, Input, aider_in_utc, aider_run, import, latest, load_file};

/// A time as the nanoseconds since 1970 a session's recency is counted in.
fn nanos(time: &str) -> i64 {
    let (seconds, nanos) = slink_format::instant(time).unwrap();
    seconds * 1_000_000_000 + i64::from(nanos)
}

/// A time as a file's modification time.
fn at(time: &str) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos(time).try_into().unwrap())
}

/// The Aider runs recorded for `cwd`, in a project the test made. The walk
/// up from `cwd` goes on above the test's own directory, so a history that
/// happens to be up there is left out.
fn runs(cwd: &Path, id: &str, project: &Path) -> Vec<Candidate> {
    // No store is named: Aider's history is a file in the project.
    listed(&nowhere(), "aider", cwd, id, 30)
        .into_iter()
        .filter(|candidate| candidate.dir == project)
        .collect()
}

fn imported(input: &Input) -> Value {
    import("aider", input).unwrap().unwrap()
}

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("aider");
    assert!(fixtures.len() >= 2, "{} fixtures", fixtures.len());
    aider_in_utc(|| {
        for fixture in fixtures {
            // An Aider fixture is the chat history file, with the input
            // history beside it when the run left one. Its last run is the
            // one imported, as it is for a file given by path.
            let (input, detected) = load_file(&fixture.join("input.chat.history.md")).unwrap();
            assert_eq!(detected, "aider", "{}", fixture.display());
            common::check_golden("aider", &fixture, &input);
        }
    });
}

/// A history of two runs. Aider ends each line it marks with two spaces.
const TWO_RUNS: &str = concat!(
    "\n",
    "# aider chat started at 2026-10-06 09:00:00\n",
    "\n",
    "> Aider v0.86.2  \n",
    "> Main model: anthropic/claude-sonnet with diff edit format  \n",
    "> Added upload.py to the chat.  \n",
    "\n",
    "#### Find the race in the upload test  \n",
    "\n",
    "The retry starts before the first attempt releases its lock.\n",
    "\n",
    "> Tokens: 2.4k sent, 38 received.  \n",
    "\n",
    "#### /exit  \n",
    "\n",
    "# aider chat started at 2026-10-06 10:30:00\n",
    "\n",
    "> Aider v0.86.2  \n",
    "> Model: openai/gpt-small with whole edit format  \n",
    "\n",
    "#### /add notes.md  \n",
    "> Added notes.md to the chat  \n",
    "\n",
    "#### Write the fix up  \n",
    "#### in two lines  \n",
    "\n",
    "Done.\n",
);

#[test]
fn the_runs_of_one_file_are_its_sessions() {
    aider_in_utc(|| {
        let project = scratch("runs");
        let written = "2026-10-06T10:45:00Z";
        write(
            &project.join(".aider.chat.history.md"),
            TWO_RUNS,
            at(written),
        );
        write(
            &project.join(".aider.input.history"),
            "\n# 2026-10-06 09:00:07.250000\n+Find the race in the upload test\n\n# 2026-10-06 10:30:02.000000\n+/add notes.md\n\n# 2026-10-06 10:31:05.500000\n+Write the fix up\n+in two lines\n",
            at(written),
        );
        let deeper = project.join("src").join("upload");
        fs::create_dir_all(&deeper).unwrap();

        // Aider keeps the file at the project root; work goes on inside it.
        let found = runs(&deeper, "", &project);
        assert_eq!(
            ids(&found),
            ["2026-10-06T10:30:00", "2026-10-06T09:00:00"],
            "the runs of the file, newest first"
        );
        // A slash command is not what the run was about.
        assert_eq!(found[0].title, "Write the fix up\nin two lines");
        assert_eq!(found[1].title, "Find the race in the upload test");
        assert_eq!(found[0].prompt, found[0].title);
        assert_eq!(found[0].file, Some(project.join(".aider.chat.history.md")));
        // The last run was active until the file was last written; the one
        // before it, until the next began.
        assert_eq!(found[0].found.recency, nanos(written));
        assert_eq!(found[1].found.recency, nanos("2026-10-06T10:30:00Z"));
        assert_eq!(
            found[1].started,
            slink_format::instant("2026-10-06T09:00:00Z")
        );

        let earlier = imported(&found[1].found.load().unwrap());
        assert_eq!(earlier["name"], "Find the race in the upload test");
        assert_eq!(earlier["created_at"], "2026-10-06T09:00:00.000Z");
        let metadata = &earlier["metadata"];
        assert_eq!(metadata["session_id"], "2026-10-06T09:00:00");
        assert_eq!(metadata["cwd"], project.to_str().unwrap());
        assert_eq!(metadata["harness_version"], "0.86.2");
        // The startup notices, the prompt and its reply, the token report,
        // and /exit, after the run itself.
        let spans = earlier["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 5);
        // What Aider says before anything is typed is setup, not something
        // it did.
        assert_eq!(spans[1]["type"], "custom");
        assert_eq!(spans[1]["input"]["messages"][0]["role"], "system");
        let call = &spans[2];
        assert_eq!(call["type"], "llm_call");
        assert_eq!(call["model"]["id"], "anthropic/claude-sonnet");
        assert_eq!(call["started_at"], "2026-10-06T09:00:07.250Z");
        assert!(
            call.get("ended_at").is_none(),
            "aider records no time for a reply, so none is given"
        );
        let reply = call["output"]["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            reply.starts_with("The retry starts") && !reply.contains("Tokens"),
            "{reply}"
        );
        assert_eq!(spans[3]["type"], "tool_call");
        assert_eq!(
            spans[3]["output"]["result"],
            "Tokens: 2.4k sent, 38 received."
        );
        assert_eq!(spans[4]["name"], "/exit");
        assert!(spans[4].get("started_at").is_none());

        // A run by the time it started.
        let one = runs(&project, "2026-10-06T10:30:00", &project);
        assert_eq!(one.len(), 1);
        let later = imported(&one[0].found.load().unwrap());
        // The startup, /add, its notice, then the two-line prompt and its
        // reply.
        let spans = later["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 5);
        // A command to aider is kept where it was typed.
        assert_eq!(spans[2]["name"], "/add notes.md");
        assert_eq!(spans[2]["type"], "custom");
        assert_eq!(spans[2]["started_at"], "2026-10-06T10:30:02.000Z");
        // A typed entry of two lines, and the model of the second run.
        assert_eq!(spans[4]["started_at"], "2026-10-06T10:31:05.500Z");
        assert_eq!(
            spans[4]["input"]["messages"][0]["content"][0]["text"],
            "Write the fix up\nin two lines"
        );
        assert_eq!(spans[4]["model"]["id"], "openai/gpt-small");

        // The project's newest run, which is the file's last.
        let newest = latest("aider", &project).unwrap();
        assert_eq!(newest.harness, "aider");
        assert_eq!(newest.recency, nanos(written));
        assert_eq!(
            imported(&newest.load().unwrap())["metadata"]["session_id"],
            "2026-10-06T10:30:00"
        );
        assert!(
            latest("aider", &deeper).is_none(),
            "a directory is asked for its own history; the command walks up"
        );
        // A run is found by the time it started, and only by that.
        let first = aider_run(&project, "2026-10-06T09:00:00").unwrap();
        assert_eq!(first.recency, nanos("2026-10-06T09:00:00Z"));
        assert_eq!(
            imported(&first.load().unwrap())["metadata"]["session_id"],
            "2026-10-06T09:00:00"
        );
        assert!(aider_run(&project, "2026-10-06T09:00:01").is_none());
    });
}

#[test]
fn only_what_the_input_history_matches_is_timed() {
    aider_in_utc(|| {
        let dir = scratch("times");
        let chat = dir.join("input.chat.history.md");
        fs::write(
            &chat,
            "\n# aider chat started at 2026-10-06 09:00:00\n\n#### again  \n\nOne.\n\n#### again  \n\nTwo.\n\n#### reworded later  \n\nThree.\n",
        )
        .unwrap();
        // An entry from before the run, the same text typed twice, and one
        // the chat words differently.
        fs::write(
            dir.join("input.input.history"),
            "\n# 2026-10-05 08:00:00.000000\n+again\n\n# 2026-10-06 09:00:10.000000\n+again\n\n# 2026-10-06 09:00:20.000000\n+again\n\n# 2026-10-06 09:00:30.000000\n+reworded\n",
        )
        .unwrap();
        let (input, harness) = load_file(&chat).unwrap();
        assert_eq!(harness, "aider", "an aider history given by path");
        assert_eq!(input.session["inputs"].as_array().unwrap().len(), 3);
        assert!(
            input.session.get("cwd").is_none(),
            "a history that is not where aider keeps it says nothing of its project"
        );
        let run = imported(&input);
        let starts: Vec<Option<&str>> = run["spans"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|span| span.get("started_at").and_then(Value::as_str))
            .collect();
        assert_eq!(
            starts,
            [
                Some("2026-10-06T09:00:10.000Z"),
                Some("2026-10-06T09:00:20.000Z"),
                None
            ],
            "the times of the three prompts"
        );
    });
}

#[test]
fn an_aider_history_is_told_from_other_text() {
    let dir = scratch("told");
    let renamed = dir.join("yesterday.md");
    fs::write(
        &renamed,
        "\n# aider chat started at 2026-10-06 09:00:00\n\n#### hi  \n\nHello.\n",
    )
    .unwrap();
    assert_eq!(
        load_file(&renamed).unwrap().1,
        "aider",
        "a renamed aider history"
    );

    let notes = dir.join("notes.md");
    fs::write(&notes, "# Notes\n\n> a quote\n").unwrap();
    assert_eq!(
        load_file(&notes).unwrap().1,
        "",
        "a Markdown file that is not a chat history"
    );

    let empty = dir.join(".aider.chat.history.md");
    fs::write(&empty, "#### typed before any run started\n").unwrap();
    assert_eq!(
        load_file(&empty).unwrap_err(),
        format!("{} holds no aider run", empty.display()),
        "a history with no run in it has nothing to import"
    );

    let loose = Input {
        lines: ["#### hi  ", "", "Hello."].map(str::to_owned).to_vec(),
        ..Input::default()
    };
    assert_eq!(
        import("aider", &loose).unwrap_err().to_string(),
        "import failed: not an aider run: it does not begin with a \"# aider chat started at\" line",
        "lines that do not begin a run are not one"
    );
}
