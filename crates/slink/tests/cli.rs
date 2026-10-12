//! Runs the built binary the way a user does, in a home of its own.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::{env, fs};

const VERSION: &str = match option_env!("SLINK_VERSION") {
    Some(version) => version,
    None => "0.0.0-dev",
};

/// Runs `slink-rs` with nothing of the caller's environment, then checks that
/// it left the home it was given empty: no command ported so far may write.
fn run(test: &str, args: &[&str]) -> Output {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).expect("creating the test home");

    let mut command = Command::new(env!("CARGO_BIN_EXE_slink-rs"));
    command
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("SLINK_HOME", home.join(".slink"));
    // Windows cannot open a socket without it.
    if let Some(root) = env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let output = command.output().expect("running slink-rs");

    let left: Vec<_> = fs::read_dir(&home)
        .expect("reading the test home")
        .map(|entry| entry.expect("reading the test home").file_name())
        .collect();
    assert!(left.is_empty(), "slink-rs {args:?} wrote {left:?}");
    output
}

fn text(bytes: &[u8]) -> &str {
    str::from_utf8(bytes).expect("output is UTF-8")
}

#[test]
fn every_spelling_of_version_prints_it() {
    for spelling in ["version", "--version", "-v", "-version"] {
        let output = run("version", &[spelling]);
        assert!(output.status.success(), "{spelling}");
        assert_eq!(text(&output.stdout), format!("slink {VERSION}\n"));
        assert_eq!(text(&output.stderr), "");
    }
}

#[test]
fn the_build_check_passes_on_this_target() {
    let output = run("build-check", &["build-check"]);
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{stdout}{}", text(&output.stderr));
    for part in ["sqlite", "viewer", "http"] {
        assert!(stdout.contains(&format!("ok    {part}")), "{stdout}");
    }
}

#[test]
fn a_command_that_is_not_ported_fails_and_says_so() {
    let output = run("unported", &["list"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    assert!(text(&output.stderr).contains("`list` is not ported yet"));
}
