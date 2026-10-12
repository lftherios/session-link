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
    for part in ["sqlite", "viewer", "http", "format", "protocol"] {
        assert!(stdout.contains(&format!("ok    {part}")), "{stdout}");
    }
}

/// The commands that only print: help, completion and a command line that
/// is wrong. None of them may touch the home.
#[test]
fn help_and_usage_touch_nothing() {
    let usage = run("usage", &[]);
    assert_eq!(usage.status.code(), Some(1));
    assert!(text(&usage.stderr).starts_with("slink — record LLM sessions locally"));
    let help = run("usage", &["--help"]);
    assert!(help.status.success());
    assert_eq!(text(&help.stdout), text(&usage.stderr));

    for command in [
        "share",
        "record",
        "dev",
        "view",
        "open",
        "list",
        "ls",
        "import",
        "tap",
        "setup",
        "on",
        "off",
        "status",
        "login",
        "logout",
        "push",
        "delete",
        "prune",
        "doctor",
        "completion",
    ] {
        let output = run("usage", &[command, "-h"]);
        assert!(output.status.success(), "{command}");
        assert!(text(&output.stdout).starts_with("Usage:\n  "), "{command}");
        assert_eq!(text(&output.stderr), "", "{command}");
        assert_eq!(
            text(&run("usage", &["help", command]).stdout),
            text(&output.stdout),
            "{command}"
        );
    }

    let unknown = run("usage", &["shar"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert_eq!(
        text(&unknown.stderr),
        "slink: unknown command \"shar\"\n  did you mean: slink share?\n  run `slink --help` for the command list\n"
    );
    let wrong = run("usage", &["completion", "--nope"]);
    assert_eq!(wrong.status.code(), Some(2));
    assert!(text(&wrong.stderr).starts_with("flag provided but not defined: -nope\nUsage:"));
    let zsh = run("usage", &["completion", "zsh"]);
    assert!(text(&zsh.stdout).contains("compdef _slink slink"));
    assert_eq!(run("usage", &["completion"]).status.code(), Some(2));
}

/// A home of its own for a test that is meant to write: the directory that
/// stands for the user's home, with the slink home inside it.
struct Home(PathBuf);

impl Home {
    fn new(test: &str) -> Home {
        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(test);
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join("work")).expect("creating the test home");
        Home(home)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_slink-rs"));
        command
            .args(args)
            .current_dir(self.0.join("work"))
            .env_clear()
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .env("SLINK_HOME", self.0.join(".slink"));
        if let Some(root) = env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("running slink-rs")
    }

    /// Every file under the slink home, by its path from there.
    fn files(&self) -> Vec<String> {
        fn walk(dir: &std::path::Path, root: &std::path::Path, found: &mut Vec<String>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else {
                    let relative = path.strip_prefix(root).unwrap();
                    found.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.0.join(".slink"), &self.0.join(".slink"), &mut found);
        found.sort();
        found
    }

    /// Nothing may appear in the user's home beside the slink home.
    fn assert_contained(&self) {
        let mut left: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert!(
            left.iter().all(|name| name == ".slink" || name == "work"),
            "{left:?}"
        );
    }
}

fn fixture(path: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(path)
        .to_string_lossy()
        .into_owned()
}

/// One request to the local viewer, by hand: the status line and the body.
fn fetch(port: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .expect("connecting to the viewer");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n"
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map_or("", |(_, body)| body)
        .to_owned();
    (status, body)
}

/// The first local journey, whole: a Claude Code session is imported, shows
/// in the listing, and is read in the viewer on loopback. Everything it
/// writes is private and inside the slink home, and nothing is uploaded.
#[test]
fn a_session_is_imported_listed_and_viewed() {
    let home = Home::new("journey");
    let transcript = fixture("import/claude-code/sub-agent/input.session.jsonl");

    let imported = home.run(&["import", &transcript]);
    let said = text(&imported.stderr);
    assert!(imported.status.success(), "{said}");
    assert_eq!(text(&imported.stdout), "");
    assert!(
        said.starts_with("✓ imported from claude-code (reconstructed)\n"),
        "{said}"
    );
    let files = home.files();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(
        files[0].starts_with("runs/") && files[0].ends_with(".json"),
        "{files:?}"
    );

    // The document is the fixture's golden, as Go would have written it.
    let capture = home.0.join(".slink").join(&files[0]);
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&capture).unwrap()).unwrap();
    let golden: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixture("import/claude-code/sub-agent/golden.run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(written["spans"], golden["spans"]);
    assert_eq!(written["source"]["fidelity"], "reconstructed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&capture), 0o600);
        assert_eq!(mode(capture.parent().unwrap()), 0o700);
        assert_eq!(mode(&home.0.join(".slink")), 0o700);
    }

    // The same session again is the same capture, not a second one.
    let again = home.run(&["import", "--from", "claude-code", &transcript]);
    assert!(
        text(&again.stderr).starts_with("✓ already imported, unchanged — reusing "),
        "{}",
        text(&again.stderr)
    );
    assert_eq!(home.files().len(), 1);

    let listed = home.run(&["list", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(text(&listed.stdout)).unwrap();
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["spans"], written["spans"].as_array().unwrap().len());
    assert_eq!(rows[0]["name"], written["name"]);
    let id = rows[0]["id"].as_str().unwrap().to_owned();
    assert!(files[0].ends_with(&format!("-{id}.json")));
    let table = home.run(&["ls"]);
    assert!(
        text(&table.stdout).starts_with(&format!("  {id}  ")),
        "{}",
        text(&table.stdout)
    );

    // The viewer: its URL is the one line on stdout.
    use std::io::{BufRead, BufReader};
    let mut viewer = home
        .command(&["view", &id, "--no-browser"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("starting the viewer");
    let mut url = String::new();
    BufReader::new(viewer.stdout.take().unwrap())
        .read_line(&mut url)
        .unwrap();
    let url = url.trim();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("not a loopback URL: {url}"));
    let (port, rest) = rest.split_once('/').unwrap();
    let (page, key) = rest.split_once("#access=").unwrap();
    assert!(page.starts_with("p/") && key.len() == 43, "{url}");
    let page = format!("/{page}");

    // Without the key there is no session, and a page that navigates here
    // gets a shell that holds none either.
    assert_eq!(fetch(port, "GET", &page, &[]).0, 403);
    let (status, shell) = fetch(port, "GET", &page, &[("Sec-Fetch-Mode", "navigate")]);
    assert_eq!(status, 200);
    assert!(
        shell.contains("Opening local viewer") && !shell.contains("DELEGATE"),
        "{shell}"
    );
    assert_eq!(
        fetch(port, "GET", "/", &[("X-Slink-Access", &"A".repeat(43))]).0,
        403
    );
    // The bundle is public; another host name is refused whatever it sends.
    assert_eq!(fetch(port, "GET", "/assets/viewer.js", &[]).0, 200);
    let mut other = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    {
        use std::io::{Read, Write};
        let request = format!(
            "GET {page} HTTP/1.1\r\nHost: example.test\r\nX-Slink-Access: {key}\r\nConnection: close\r\n\r\n"
        );
        other.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        other.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }

    // With it, the page holds the document the capture holds.
    let access = [("X-Slink-Access", key)];
    let (status, body) = fetch(port, "GET", &page, &access);
    assert_eq!(status, 200);
    let embedded = body
        .split_once("<script type=\"application/json\" id=\"run-data\">")
        .and_then(|(_, rest)| rest.split_once("</script>"))
        .map(|(json, _)| json)
        .expect("the session in the page");
    let shown: serde_json::Value = serde_json::from_str(embedded).unwrap();
    assert_eq!(shown, written);
    assert!(body.contains("<script src=\"/assets/viewer.js\"></script>"));
    assert!(fetch(port, "GET", "/", &access).1.contains("Stop viewer"));
    assert_eq!(fetch(port, "GET", "/p/nothing", &access).0, 404);

    // Publishing is not in this binary: the page's own endpoint says so,
    // and a request from another origin cannot stop the viewer.
    let local = [("X-Slink-Access", key), ("x-slink", "1")];
    let (status, refused) = fetch(
        port,
        "POST",
        &format!("/api/publish-preview{}", &page[2..]),
        &local,
    );
    assert_eq!(status, 501, "{refused}");
    assert!(refused.contains("nothing was uploaded"), "{refused}");
    let foreign = [
        ("X-Slink-Access", key),
        ("x-slink", "1"),
        ("Origin", "https://example.test"),
    ];
    assert_eq!(fetch(port, "POST", "/api/stop", &foreign).0, 403);
    assert_eq!(fetch(port, "POST", "/api/stop", &access).0, 403);
    assert_eq!(fetch(port, "POST", "/api/stop", &local).0, 200);
    let status = viewer.wait().expect("the viewer exits when stopped");
    assert!(status.success());

    // What the journey left: the capture, and the preview saved from it
    // under the hash of its bytes.
    let files = home.files();
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(
        files[0],
        format!("{}.json", page.replace("/p/", "previews/"))
    );
    home.assert_contained();
}

/// Nothing leaves the machine from this binary: every command that would
/// publish says it cannot, and writes nothing.
#[test]
fn nothing_can_be_published_yet() {
    let home = Home::new("publication");
    for command in ["share", "push", "login", "logout", "delete"] {
        let output = home.run(&[command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(text(&output.stdout), "");
        let said = text(&output.stderr);
        assert!(
            said.contains("cannot publish yet") && said.contains("nothing was uploaded"),
            "{said}"
        );
    }
    assert_eq!(home.files(), Vec::<String>::new());
    home.assert_contained();
}

#[test]
fn a_command_that_is_not_ported_fails_and_says_so() {
    let home = Home::new("unported");
    for command in [
        "record", "tap", "setup", "status", "doctor", "prune", "on", "off",
    ] {
        let output = home.run(&[command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(text(&output.stdout), "");
        assert!(
            text(&output.stderr).contains(&format!("`{command}` is not ported yet")),
            "{command}"
        );
    }
    let codex = home.run(&["import", "--from", "codex"]);
    assert_eq!(codex.status.code(), Some(1));
    assert!(text(&codex.stderr).contains("codex sessions cannot be imported by slink-rs yet"));
    let unknown = home.run(&["import", "--from", "nope"]);
    assert!(
        text(&unknown.stderr)
            .starts_with("unknown agent \"nope\" — known: aider, claude-code, codex")
    );
    assert_eq!(home.files(), Vec::<String>::new());
}

/// With nothing imported, each command says what is missing and how to get
/// it, and discovery reads only the home it was given.
#[test]
fn an_empty_home_explains_itself() {
    let home = Home::new("empty");
    let listed = home.run(&["list"]);
    assert!(listed.status.success());
    assert!(text(&listed.stderr).starts_with("no sessions yet\n"));
    assert_eq!(text(&home.run(&["list", "--json"]).stdout), "[]\n");
    let imported = home.run(&["import"]);
    assert_eq!(imported.status.code(), Some(1));
    assert!(
        text(&imported.stderr).starts_with("✗ no coding-agent history found on this machine"),
        "{}",
        text(&imported.stderr)
    );
    let viewed = home.run(&["view", "nosuch"]);
    assert_eq!(viewed.status.code(), Some(1));
    assert!(
        text(&viewed.stderr).starts_with("no session \"nosuch\" for this project"),
        "{}",
        text(&viewed.stderr)
    );
    assert_eq!(home.files(), Vec::<String>::new());
    home.assert_contained();
}
