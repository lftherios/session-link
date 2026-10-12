//! The Go exporter's tests, held to the same fixtures.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use slink_format::encode;

use super::export::{missing_references, selected_text};
use super::{Catalog, Draft, EXTENSION, Selection, Unit, build_catalog, export};

fn message(role: &str, text: &str) -> Value {
    json!({"role": role, "content": [{"type": "text", "text": text}]})
}

fn call(id: &str, parent: &str, given: Value, made: Value) -> Value {
    json!({"id": id, "parent_id": parent, "type": "llm_call", "input": {"messages": given}, "output": {"messages": made}})
}

fn containing<'a>(catalog: &'a Catalog, text: &str) -> Vec<&'a Unit> {
    catalog
        .units
        .iter()
        .filter(|unit| unit.text.contains(text))
        .collect()
}

fn whole(id: &str) -> Selection {
    Selection {
        id: id.to_owned(),
        ..Selection::default()
    }
}

fn passage(id: &str, start: i64, end: i64) -> Selection {
    Selection {
        id: id.to_owned(),
        start: Some(start),
        end: Some(end),
    }
}

fn testdata(path: &str) -> Value {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(path);
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|err| panic!("{}: {err}", file.display())),
    )
    .unwrap()
}

#[test]
fn a_catalog_keeps_each_agents_prompts_and_repeated_turns() {
    let (asked, answered) = (
        message("user", "parent prompt"),
        message("assistant", "parent answer"),
    );
    let mut run = json!({"source": {"kind": "proxy"}, "spans": [
        {"id": "root", "type": "agent"},
        call("one", "root", json!([asked]), json!([answered])),
        {"id": "child", "parent_id": "one", "type": "agent"},
        call("two", "child", json!([message("user", "child prompt")]), json!([message("assistant", "child answer")])),
        call("three", "root", json!([asked, answered, message("user", "parent followup")]), json!([message("assistant", "continued answer")])),
    ]});
    let catalog = build_catalog(&run);
    assert_eq!(
        containing(&catalog, "parent prompt").len(),
        1,
        "a replayed prompt is listed once"
    );
    for (answer, prompt) in [
        ("child answer", "child prompt"),
        ("continued answer", "parent followup"),
    ] {
        let unit = containing(&catalog, answer)[0];
        assert_eq!(
            unit.prompt_ids,
            [containing(&catalog, prompt)[0].id.clone()],
            "{answer}"
        );
    }
    // A harness that records only what is new keeps a prompt said twice.
    for source in [
        json!({"kind": "import"}),
        json!({"kind": "sdk", "label": "pi-extension@0.1.0"}),
    ] {
        run["source"] = source;
        run["spans"] = json!([
            call("one", "", json!([asked]), Value::Null),
            call("two", "", json!([asked]), json!([answered]))
        ]);
        assert_eq!(containing(&build_catalog(&run), "parent prompt").len(), 2);
    }
}

#[test]
fn tool_results_keep_repeated_parts_and_one_copy_of_a_call() {
    let parts =
        json!([{"type": "text", "text": "same result"}, {"type": "text", "text": "same result"}]);
    let arguments = json!({"command": "echo hi"});
    let run = json!({"spans": [
        call("one", "", json!([message("user", "run it")]), json!([{"role": "assistant", "content": [{"type": "tool_call", "id": "c1", "name": "bash", "arguments": arguments}]}])),
        {"id": "tool", "type": "tool_call", "parent_id": "one", "name": "bash", "input": {"tool_call_id": "c1", "arguments": arguments}, "output": {"result": parts}},
        call("two", "", json!([{"role": "tool", "content": [{"type": "tool_result", "tool_call_id": "c1", "content": parts}]}]), json!([message("assistant", "done")])),
    ]});
    let catalog = build_catalog(&run);
    assert_eq!(containing(&catalog, "same result").len(), 2);
    let calls = containing(&catalog, "echo hi");
    assert_eq!(
        calls.len(),
        1,
        "the call in the message and the call as a span are one"
    );
    assert!(calls[0].text.contains("bash"));
    assert_eq!(
        calls[0].text,
        "{\n  \"arguments\": {\n    \"command\": \"echo hi\"\n  },\n  \"name\": \"bash\"\n}"
    );
}

#[test]
fn a_prompt_with_a_part_that_cannot_be_shared_is_not_whole() {
    let run = json!({"spans": [call("one", "", json!([{"role": "user", "content": [
        {"type": "text", "text": "Explain the picture"},
        {"type": "image", "url": "https://example.test/private.png"},
    ]}]), json!([message("assistant", "It depicts a forest.")]))]});
    let catalog = build_catalog(&run);
    let answer = containing(&catalog, "It depicts")[0];
    assert!(answer.prompt_incomplete);
    assert_eq!(answer.prompt_ids.len(), 1);
    assert!(
        catalog.units[1].unavailable,
        "an attachment is not offered as text"
    );
}

#[test]
fn a_reference_definition_is_picked_apart_from_the_text_around_it() {
    let text =
        "Finding from [docs][source].\n\nDO_NOT_SHARE\n\n[source]: https://example.test/docs";
    let run = json!({"created_at": "2026-09-11T00:00:00Z", "spans": [call("one", "", Value::Null, json!([message("assistant", text)]))]});
    let catalog = build_catalog(&run);
    assert_eq!(catalog.units.len(), 2);
    assert_eq!(catalog.units[1].kind, "source_reference");
    let mut draft = Draft {
        title: "Finding".into(),
        primary: catalog.units[0].id.clone(),
        items: vec![passage(&catalog.units[0].id, 0, 28)],
        ..Draft::default()
    };
    assert!(
        export(&run, &catalog, &draft).is_err(),
        "a citation's target is not silently left out"
    );
    draft.items.push(whole(&catalog.units[1].id));
    let out = encode(&export(&run, &catalog, &draft).unwrap());
    assert!(!out.contains("DO_NOT_SHARE") && out.contains("https://example.test/docs"));
}

#[test]
fn every_harness_fixture_has_evidence_that_can_be_picked() {
    for fixture in [
        "claude-code/basic",
        "codex/basic",
        "pi/basic",
        "opencode/basic",
        "hermes/basic",
        "dsh/v4-run",
        "dsh/v4-sub-agents",
        "aider/v0.86-edit",
    ] {
        let run = testdata(&format!("import/{fixture}/golden.run.json"));
        let catalog = build_catalog(&run);
        let mut roles = std::collections::HashSet::new();
        for unit in catalog.units.iter().filter(|unit| !unit.unavailable) {
            roles.insert(unit.role.as_str());
            let draft = Draft {
                title: "Selected evidence".into(),
                items: vec![whole(&unit.id)],
                primary: unit.id.clone(),
                ..Draft::default()
            };
            export(&run, &catalog, &draft)
                .unwrap_or_else(|error| panic!("{fixture}: unit {}: {error}", unit.id));
        }
        for role in ["user", "assistant", "tool"] {
            assert!(
                roles.contains(role),
                "{fixture}: no {role} evidence in {roles:?}"
            );
        }
    }
}

#[test]
fn reference_cards_keep_the_source_text_and_ignore_code() {
    let note = "[^source]: `Atlas` documentation\n    Additional detail.";
    let text = format!(
        "```md\n[example]: https://example.test/not-a-citation\n```\n\n{note}\n[docs]: https://example.test/docs"
    );
    let run =
        json!({"spans": [call("one", "", Value::Null, json!([message("assistant", &text)]))]});
    let catalog = build_catalog(&run);
    assert_eq!(catalog.units.len(), 3);
    assert_eq!(catalog.units[1].text, note);
    assert_eq!(catalog.units[2].text, "[docs]: https://example.test/docs");
    assert!(
        !missing_references(&["Finding[^source]", note]),
        "inline code in a footnote keeps its definition"
    );
}

#[test]
fn reasoning_the_capture_does_not_hold_is_never_shared_as_text() {
    for legacy in [false, true] {
        let missing = if legacy {
            json!({"type": "thinking", "text": "[reasoning]"})
        } else {
            json!({"type": "thinking", "text": "", "unavailable": true, "reason": "encrypted"})
        };
        let mut run = json!({"schema": "session/v0", "created_at": "2026-09-11T00:00:00Z", "source": {"kind": "import", "harness": "codex"}, "spans": [
            call("one", "", json!([message("user", "Compare options")]), json!([{"role": "assistant", "content": [
                missing,
                {"type": "thinking", "text": "Actual readable summary"},
                {"type": "text", "text": "The answer"},
            ]}])),
        ]});
        let catalog = build_catalog(&run);
        let (missing, readable, answer) = (&catalog.units[1], &catalog.units[2], &catalog.units[3]);
        assert!(missing.unavailable && !readable.unavailable);
        assert_eq!(answer.id, "u0-out-0-2");
        let draft = |title: &str, ids: &[&str]| Draft {
            title: title.into(),
            items: ids.iter().map(|id| whole(id)).collect(),
            ..Draft::default()
        };
        assert!(export(&run, &catalog, &draft("Missing", &[&missing.id])).is_err());
        export(
            &run,
            &catalog,
            &draft("Readable", &[&readable.id, &answer.id]),
        )
        .unwrap();
        if legacy {
            // The rule for old Codex imports does not touch another
            // harness's literal text.
            run["source"] = json!({"kind": "import", "harness": "another-harness"});
            assert!(!build_catalog(&run).units[1].unavailable);
        }
    }
}

#[test]
fn human_input_is_not_tool_results_or_harness_wrappers() {
    let result = json!({"role": "user", "content": [{"type": "tool_result", "tool_call_id": "c1", "content": [{"type": "text", "text": "file contents"}]}]});
    let run = json!({"source": {"kind": "import", "harness": "claude-code"}, "spans": [
        call("one", "", json!([message("user", "Fix the header")]), json!([{"role": "assistant", "content": [{"type": "tool_call", "id": "c1", "name": "Read", "arguments": {"file_path": "header.tsx"}}]}])),
        call("two", "", json!([result, message("user", "<system-reminder>Keep going</system-reminder>"), message("user", "<task-notification><summary>Tests finished</summary></task-notification>")]), json!([message("assistant", "Header fixed")])),
    ]});
    let catalog = build_catalog(&run);
    let prompt = containing(&catalog, "Fix the header")[0];
    assert_eq!(
        containing(&catalog, "Header fixed")[0].prompt_ids,
        std::slice::from_ref(&prompt.id)
    );
    assert_eq!(containing(&catalog, "file contents")[0].role, "tool");
    assert_eq!(containing(&catalog, "Keep going")[0].role, "system");
    assert_eq!(containing(&catalog, "Tests finished")[0].role, "system");
}

/// The fixture's marker is copied into every channel a careless export could
/// leak through. The export must be the independently written example, to
/// the byte, and hold the marker nowhere.
#[test]
fn the_research_export_leaves_out_every_channel_that_was_not_picked() {
    let run = testdata("share/research/session.json");
    let mut draft = Draft::from_json(&testdata("share/research/draft.json")).unwrap();
    let original = encode(&run);
    let catalog = build_catalog(&run);
    assert_eq!(
        containing(&catalog, "Compare Atlas").len(),
        1,
        "a replayed prompt is listed once"
    );
    let out = export(&run, &catalog, &draft).unwrap();
    let bytes = encode(&out);
    assert_eq!(bytes, encode(&testdata("share/research/excerpt.json")));
    for excluded in [
        "OMITTED_INTERNAL_STRATEGY",
        "A comparison of the two fictional",
        "Internal planning",
        "private_extra",
        "attachments",
        "raw",
        "params",
        "input_tokens",
    ] {
        assert!(
            !bytes.contains(excluded),
            "{excluded} survived in the export"
        );
    }
    for included in [
        "Compare Atlas and Beacon",
        "Atlas documents a self-serve",
        "\u{1f50e}",
        "https://atlas.example.test/onboarding",
        draft.note.as_str(),
    ] {
        assert!(
            bytes.contains(included),
            "{included} is missing from the export"
        );
    }
    assert_eq!(
        encode(&run),
        original,
        "an export does not change its source"
    );
    let envelope = &out["extensions"][EXTENSION];
    assert_eq!(envelope["original_fidelity"], "exact");
    assert_eq!(out["source"]["fidelity"], "partial");
    let items = envelope["items"].as_array().unwrap();
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[1]["passage"], true);
    assert!(items[1].get("prompt_missing").is_none());
    assert_ne!(
        envelope["primary_id"], items[0]["id"],
        "the starting point is not replaced by the earlier prompt"
    );
    // The order pieces were picked in does not change the document.
    draft.items.swap(0, 1);
    assert_eq!(encode(&export(&run, &catalog, &draft).unwrap()), bytes);
}

#[test]
fn a_passage_is_verbatim_and_ends_on_whole_characters() {
    let text = "before \u{1f50e} caf\u{e9} after";
    let item = |start: Option<i64>, end: Option<i64>| Selection {
        id: String::new(),
        start,
        end,
    };
    assert_eq!(
        selected_text(text, &item(Some(7), Some(14))).unwrap(),
        ("\u{1f50e} caf\u{e9}".to_owned(), true)
    );
    assert_eq!(
        selected_text(text, &item(None, None)).unwrap(),
        (text.to_owned(), false)
    );
    for (start, end) in [
        (Some(8), Some(14)),
        (Some(7), Some(8)),
        (Some(-1), Some(4)),
        (Some(0), Some(999)),
        (Some(4), Some(3)),
        (Some(0), None),
    ] {
        assert!(
            selected_text(text, &item(start, end)).is_err(),
            "{start:?}..{end:?}"
        );
    }
}

#[test]
fn a_draft_cannot_pick_what_is_unknown_twice_or_unavailable() {
    let unit = |id: &str, text: &str, unavailable: bool| Unit {
        id: id.into(),
        text: text.into(),
        unavailable,
        ..Unit::default()
    };
    let catalog = Catalog {
        units: vec![unit("text", "a", false), unit("blob", "", true)],
    };
    let draft = |title: &str, ids: &[&str], primary: &str| Draft {
        title: title.into(),
        items: ids.iter().map(|id| whole(id)).collect(),
        primary: primary.into(),
        ..Draft::default()
    };
    for refused in [
        draft("a", &[], ""),
        draft("a", &["unknown"], ""),
        draft("a", &["blob"], ""),
        draft("a", &["text", "text"], ""),
        draft("a", &["text"], "absent"),
        draft(&"x".repeat(257), &["text"], ""),
    ] {
        assert!(refused.validate(&catalog, true).is_err(), "{refused:?}");
    }
    Draft::default()
        .validate(&catalog, false)
        .expect("an empty draft can be saved");
    draft("a", &["text"], "text")
        .validate(&catalog, true)
        .unwrap();
}

#[test]
fn missing_prompt_and_tool_context_are_said() {
    let run = testdata("import/pi/error-and-trailing/golden.run.json");
    let catalog = build_catalog(&run);
    let result = catalog
        .units
        .iter()
        .rfind(|unit| unit.kind == "tool_result")
        .expect("a tool result");
    let called = catalog
        .units
        .iter()
        .rfind(|unit| unit.kind == "tool_call")
        .expect("a tool call");
    let mut draft = Draft {
        title: "Why did this fail?".into(),
        items: vec![whole(&result.id)],
        primary: result.id.clone(),
        ..Draft::default()
    };
    let out = export(&run, &catalog, &draft).unwrap();
    let item = &out["extensions"][EXTENSION]["items"][0];
    assert_eq!(item["prompt_missing"], true);
    assert_eq!(item["tool_call_missing"], true);
    draft.items.push(whole(&called.id));
    draft
        .items
        .extend(result.prompt_ids.iter().map(|id| whole(id)));
    let out = export(&run, &catalog, &draft).unwrap();
    for item in out["extensions"][EXTENSION]["items"].as_array().unwrap() {
        if item["kind"] == "tool_result" {
            assert!(
                item.get("prompt_missing").is_none() && item.get("tool_call_missing").is_none(),
                "{item}"
            );
        }
    }
}

#[test]
fn a_reference_travels_with_its_definition() {
    assert!(missing_references(&["See [the docs][onboarding]."]));
    assert!(!missing_references(&[
        "See [the docs][onboarding].",
        "[onboarding]: https://example.test/docs"
    ]));
    assert!(!missing_references(&[
        "Read [docs](https://example.test/docs)."
    ]));
    assert!(!missing_references(&[
        "Use `matrix[i][j]`.\n```python\nx = matrix[i][j]\n```"
    ]));
}

#[test]
fn a_draft_from_a_page_is_read_strictly() {
    let draft = Draft::from_json(&json!({"title": "T", "note": "", "items": [{"id": "a"}, {"id": "b", "start": 1, "end": 4}], "primary": "a"})).unwrap();
    assert_eq!(draft.items, [whole("a"), passage("b", 1, 4)]);
    assert_eq!(Draft::from_json(&draft.to_json()).unwrap(), draft);
    assert_eq!(Draft::from_json(&json!({})).unwrap(), Draft::default());
    for refused in [
        json!({"title": 7}),
        json!({"extra": true}),
        json!({"items": [{"id": "a", "start": 1.5, "end": 2}]}),
        json!({"items": [{"id": "a", "from": 1}]}),
        json!({"items": {}}),
        json!([]),
    ] {
        assert!(Draft::from_json(&refused).is_err(), "{refused}");
    }
}
