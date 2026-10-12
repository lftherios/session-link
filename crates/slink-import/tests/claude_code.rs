//! The Claude Code importer over its fixtures in `testdata/import`.

mod common;

use slink_import::{HARNESSES, import, load_file, ported};

#[test]
fn every_fixture_reproduces_its_golden() {
    let fixtures = common::fixtures("claude-code");
    assert!(fixtures.len() >= 6, "{} fixtures", fixtures.len());
    for fixture in fixtures {
        // Read as a file someone named, so the sub-agents kept beside the
        // transcript are found by the code that finds real ones.
        let (mut input, detected) = load_file(&fixture.join("input.session.jsonl")).unwrap();
        assert_eq!(detected, "claude-code", "{}", fixture.display());
        input.fallback = "golden".into();
        common::check_golden("claude-code", &fixture, &input);
    }
}

#[test]
fn a_harness_without_an_importer_says_so() {
    for harness in HARNESSES.into_iter().filter(|harness| !ported(harness)) {
        let error = import(harness, &Default::default()).unwrap_err();
        assert!(error.to_string().contains(harness), "{error}");
    }
    assert!(import("nope", &Default::default()).is_err());
}
