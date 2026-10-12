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
    send(port, method, path, headers, "")
}

fn send(
    port: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .expect("connecting to the viewer");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
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

/// A viewer started for a test: where it listens, its access key, and the
/// page it opened on.
struct Running {
    child: std::process::Child,
    port: String,
    key: String,
    page: String,
}

impl Running {
    fn start(home: &Home, args: &[&str]) -> Running {
        Running::of(home.command(args))
    }

    /// Starts the viewer a test has prepared a command for.
    fn of(mut command: Command) -> Running {
        use std::io::{BufRead, BufReader};
        let child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("starting the viewer");
        // Held from here on, so a check that fails below still stops it.
        let mut running = Running {
            child,
            port: String::new(),
            key: String::new(),
            page: String::new(),
        };
        let mut url = String::new();
        BufReader::new(running.child.stdout.take().unwrap())
            .read_line(&mut url)
            .unwrap();
        let url = url.trim();
        let rest = url
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("not a loopback URL: {url}"));
        // The sessions page is the address with no path at all.
        let (address, key) = rest.split_once("#access=").unwrap();
        let (port, page) = address.split_once('/').unwrap_or((address, ""));
        running.port = port.to_owned();
        running.key = key.split('&').next().unwrap().to_owned();
        running.page = format!("/{page}");
        running
    }

    /// A request the page itself would make: with the key, and for anything
    /// that changes something, the header that marks a local action.
    fn ask(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let headers = [("X-Slink-Access", self.key.as_str()), ("x-slink", "1")];
        send(&self.port, method, path, &headers, body)
    }

    fn json(&self, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
        let (status, text) = self.ask(method, path, body);
        (
            status,
            serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}")),
        )
    }

    fn stop(mut self) {
        assert_eq!(self.ask("POST", "/api/stop", "").0, 200);
        assert!(
            self.child
                .wait()
                .expect("the viewer exits when stopped")
                .success()
        );
    }
}

impl Drop for Running {
    /// A test that fails while its viewer runs must not leave it running.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A viewer that was started in the background, which no test holds as a
/// child: it is asked to stop when the test ends, however it ends.
struct Background {
    port: String,
    key: String,
}

impl Drop for Background {
    fn drop(&mut self) {
        use std::io::{Read, Write};
        // Nothing here may fail the test: a viewer already stopped refuses
        // the connection, which is the wanted end.
        if let Ok(mut stream) = std::net::TcpStream::connect(format!("127.0.0.1:{}", self.port)) {
            let _ = stream.write_all(
                format!(
                    "POST /api/stop HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Length: 0\r\nX-Slink-Access: {}\r\nx-slink: 1\r\n\r\n",
                    self.port, self.key
                )
                .as_bytes(),
            );
            let _ = stream.read_to_end(&mut Vec::new());
        }
    }
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
    let viewer = Running::start(&home, &["view", &id, "--no-browser"]);
    let (port, key, page) = (
        viewer.port.as_str(),
        viewer.key.as_str(),
        viewer.page.clone(),
    );
    assert!(page.starts_with("/p/") && key.len() == 43, "{page} {key}");

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
    viewer.stop();

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

/// An excerpt, end to end through the local server: the draft is saved by
/// revision, the export is the fixture's independently written example, and
/// the marker the fixture plants in every channel that could leak is in
/// nothing the server stores or serves for the excerpt.
#[test]
fn an_excerpt_is_composed_and_exported_without_what_was_left_out() {
    const MARKER: &str = "OMITTED_INTERNAL_STRATEGY";
    let home = Home::new("excerpt");
    let session = fixture("share/research/session.json");
    let draft = fs::read_to_string(fixture("share/research/draft.json")).unwrap();
    let viewer = Running::start(&home, &["view", "--session", &session, "--no-browser"]);
    let source = viewer.page.trim_start_matches("/p/").to_owned();

    let (status, page) = viewer.ask("GET", &format!("/compose/{source}"), "");
    assert!(status == 200 && page.contains("__COMPOSE__"), "{page}");
    let (status, composed) = viewer.json("GET", &format!("/api/compose/{source}"), "");
    assert_eq!(status, 200);
    assert_eq!(composed["revision"], 0);
    assert!(
        composed["catalog"]["units"]
            .to_string()
            .contains("Compare Atlas")
    );

    // The draft is saved one revision at a time, and only from this page.
    let saved = format!("{{\"revision\":0,\"draft\":{draft}}}");
    let draft_path = format!("/api/draft/{source}");
    assert_eq!(
        viewer.json("PUT", &draft_path, &saved),
        (200, serde_json::json!({"revision": 1}))
    );
    assert_eq!(
        viewer.ask("PUT", &draft_path, &saved).0,
        409,
        "a stale tab does not replace saved edits"
    );
    let foreign = [
        ("X-Slink-Access", viewer.key.as_str()),
        ("x-slink", "1"),
        ("Origin", "https://example.test"),
    ];
    assert_eq!(
        send(&viewer.port, "PUT", &draft_path, &foreign, &saved).0,
        403
    );
    assert_eq!(
        send(
            &viewer.port,
            "PUT",
            &draft_path,
            &[("X-Slink-Access", viewer.key.as_str())],
            &saved
        )
        .0,
        403
    );
    assert_eq!(
        viewer
            .ask(
                "PUT",
                &draft_path,
                "{\"revision\":1,\"draft\":{\"extra\":true}}"
            )
            .0,
        400
    );
    assert_eq!(
        viewer
            .ask(
                "PUT",
                &draft_path,
                "{\"revision\":1,\"draft\":{\"items\":[{\"id\":\"nope\"}]}}"
            )
            .0,
        422
    );
    assert_eq!(viewer.ask("POST", &draft_path, &saved).0, 405);

    // A title typed here is kept beside the preview, never in it.
    let title_path = format!("/api/title/{source}");
    assert_eq!(viewer.ask("PUT", &title_path, "{\"title\":\"  \"}").0, 422);
    assert_eq!(
        viewer
            .json("PUT", &title_path, "{\"title\":\" Atlas and Beacon \"}")
            .1["title"],
        "Atlas and Beacon"
    );
    assert_eq!(
        viewer.json("GET", &title_path, "").1["title"],
        "Atlas and Beacon"
    );
    viewer.stop();

    // Another run of the viewer finds the title, note and selections again.
    let viewer = Running::start(&home, &["view", "--session", &session, "--no-browser"]);
    assert_eq!(
        viewer.page,
        format!("/p/{source}"),
        "the same session is the same preview"
    );
    let (_, composed) = viewer.json("GET", &format!("/api/compose/{source}"), "");
    let wanted: serde_json::Value = serde_json::from_str(&draft).unwrap();
    assert_eq!(composed["revision"], 1);
    assert_eq!(composed["draft"], wanted);
    assert_eq!(composed["source_title"], "Atlas and Beacon");
    assert!(
        viewer
            .ask("GET", &viewer.page, "")
            .1
            .contains("\"title\":\"Atlas and Beacon\"")
    );

    let (status, exported) = viewer.json("POST", &format!("/api/export/{source}"), &draft);
    assert_eq!(status, 200, "{exported}");
    let example: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture("share/research/excerpt.json")).unwrap())
            .unwrap();
    assert_eq!(exported["document"], example);
    let url = exported["url"].as_str().unwrap().to_owned();
    let id = url.trim_start_matches("/p/").to_owned();
    assert_ne!(id, source);

    // What was left out is in none of the excerpt's channels: the file on
    // disk, the page, the document the page fetches or downloads.
    let stored =
        fs::read_to_string(home.0.join(".slink/previews").join(format!("{id}.json"))).unwrap();
    assert!(
        !stored.contains(MARKER) && !stored.contains(&source),
        "the stored excerpt leaks"
    );
    let (status, page) = viewer.ask("GET", &url, "");
    assert_eq!(status, 200);
    assert!(!page.contains(MARKER), "the excerpt's page leaks");
    assert!(
        page.contains("Edit selection") && page.contains(&format!("/compose/{source}?view={id}"))
    );
    let (status, document) = viewer.ask("GET", &format!("/api/document/{id}"), "");
    assert!(status == 200 && !document.contains(MARKER));
    assert!(
        fs::read_to_string(&session).unwrap().contains(MARKER),
        "the fixture still plants its marker"
    );

    // The saved view is listed for its session and opens with its draft.
    let (_, composed) = viewer.json("GET", &format!("/api/compose/{source}?view={id}"), "");
    assert_eq!(
        composed["views"],
        serde_json::json!([{"title": wanted["title"], "url": url}])
    );
    assert_eq!(composed["draft"], wanted);
    assert_eq!(
        viewer
            .ask("GET", &format!("/api/compose/{source}?view=absent"), "")
            .0,
        404
    );

    // Editing and exporting again makes another preview and leaves this one.
    let revised = draft.replacen(
        wanted["note"].as_str().unwrap(),
        "A revised request for review",
        1,
    );
    let (_, again) = viewer.json("POST", &format!("/api/export/{source}"), &revised);
    assert_ne!(again["url"], exported["url"]);
    assert_eq!(
        fs::read_to_string(home.0.join(".slink/previews").join(format!("{id}.json"))).unwrap(),
        stored
    );
    viewer.stop();
    home.assert_contained();
}

/// `--background` returns once the viewer is listening and leaves it
/// running, with its diagnostics in a log in the slink home.
#[test]
fn a_background_viewer_outlives_the_command_that_started_it() {
    let home = Home::new("background");
    let session = fixture("viewer/arrival/session.json");
    let started = home.run(&[
        "view",
        "--session",
        &session,
        "--background",
        "--no-browser",
    ]);
    let said = text(&started.stderr);
    assert!(started.status.success(), "{said}");
    assert!(
        said.contains("Local viewer running (PID ")
            && said.contains("Stop it from the sessions page."),
        "{said}"
    );
    let url = text(&started.stdout).trim().to_owned();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("not a loopback URL: {url}"));
    let (port, rest) = rest.split_once('/').unwrap();
    let (page, key) = rest.split_once("#access=").unwrap();
    let _stopped = Background {
        port: port.to_owned(),
        key: key.to_owned(),
    };
    let local = [("X-Slink-Access", key), ("x-slink", "1")];
    // The command has returned; the viewer still answers.
    let (status, body) = send(port, "GET", &format!("/{page}"), &local, "");
    assert!(status == 200 && body.contains("run-data"), "{status}");
    let logs: Vec<String> = home
        .files()
        .into_iter()
        .filter(|file| file.starts_with("viewer-") && file.ends_with(".log"))
        .collect();
    assert_eq!(logs.len(), 1, "{:?}", home.files());
    let log = fs::read_to_string(home.0.join(".slink").join(&logs[0])).unwrap();
    assert!(
        log.contains("Local preview ready")
            && log.contains("Stop the viewer from the sessions page."),
        "{log}"
    );
    assert_eq!(send(port, "POST", "/api/stop", &local, "").0, 200);
    // Stopped: nothing listens there any more.
    let gone = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_err()
    });
    assert!(gone, "the viewer kept listening after it was stopped");
    home.assert_contained();
}

/// Whatever opens the browser can stay for as long as the browser does. It
/// is given none of the command's streams, so a caller that reads them to
/// the end is done when the command is.
#[cfg(unix)]
#[test]
fn a_browser_that_stays_open_does_not_hold_the_caller() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new("opener");
    // An opener under either name that outlasts the command by far.
    let bin = home.0.join("work/bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["open", "xdg-open"] {
        let opener = bin.join(name);
        fs::write(&opener, "#!/bin/sh\nexec /bin/sleep 20\n").unwrap();
        fs::set_permissions(&opener, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let session = fixture("viewer/arrival/session.json");
    let mut command = home.command(&["view", "--session", &session, "--background"]);
    command.env("PATH", &bin);
    let began = std::time::Instant::now();
    let started = command.output().expect("running slink-rs");
    let took = began.elapsed();
    let said = text(&started.stderr);
    assert!(started.status.success(), "{said}");
    let url = text(&started.stdout).trim().to_owned();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("not a loopback URL: {url}"));
    let (port, rest) = rest.split_once('/').unwrap();
    let (_, key) = rest.split_once("#access=").unwrap();
    let _stopped = Background {
        port: port.to_owned(),
        key: key.to_owned(),
    };
    // The opener was found and started, and is still there.
    assert!(!said.contains("Could not open a browser"), "{said}");
    assert!(
        took < std::time::Duration::from_secs(10),
        "the command's streams stayed open for {took:?}"
    );
}

/// Every run in an Aider history is in one file, so a saved preview is found
/// again by the run and not by the file alone: viewing a second run never
/// shows the first.
#[test]
fn two_runs_in_one_aider_history_are_two_previews() {
    let home = Home::new("aider-runs");
    let history =
        fs::read_to_string(fixture("import/aider/v0.86-two-runs/input.chat.history.md")).unwrap();
    fs::write(home.0.join("work/.aider.chat.history.md"), &history).unwrap();
    // A run's id is the time it started, as Aider wrote it.
    let runs: Vec<String> = history
        .lines()
        .filter_map(|line| line.strip_prefix("# aider chat started at "))
        .map(|stamp| stamp.trim().replacen(' ', "T", 1))
        .collect();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let mut pages = Vec::new();
    for _ in 0..2 {
        for run in &runs {
            let mut command =
                home.command(&["view", "--from", "aider", "--session", run, "--no-browser"]);
            command.env("TZ", "UTC");
            let viewer = Running::of(command);
            pages.push(viewer.page.clone());
            viewer.stop();
        }
    }
    assert_ne!(pages[0], pages[1], "the second run was shown as the first");
    // Seen again, each run is the preview it was before.
    assert_eq!(pages[..2], pages[2..]);
    home.assert_contained();
}

/// What the pi extension's `/slink view` asks for, word for word: the
/// session pi is in, by its file, in the background. The extension shows
/// the last line of what is printed as the preview's address, and what is
/// written to stderr when the command fails. Nothing here passes
/// `--no-browser`, so the test runs as a remote shell does, where no browser
/// is opened and the command says how to reach the port.
#[test]
fn the_pi_extension_gets_a_local_address_for_the_session_it_names() {
    let home = Home::new("pi-handoff");
    let session = home.0.join("work/resumed session.jsonl");
    fs::copy(fixture("import/pi/basic/input.session.jsonl"), &session).unwrap();
    let session = session.to_string_lossy().into_owned();
    let view = |session: &str| {
        let mut command =
            home.command(&["view", "--from", "pi", "--session", session, "--background"]);
        command.env("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22");
        command.output().expect("running slink-rs")
    };
    let started = view(&session);
    let said = text(&started.stderr);
    assert!(started.status.success(), "{said}");
    let printed = text(&started.stdout);
    let url = printed.trim().lines().last().unwrap_or_default();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("not a loopback URL: {printed}"));
    let (port, rest) = rest.split_once('/').unwrap();
    let (page, key) = rest.split_once("#access=").unwrap();
    let _stopped = Background {
        port: port.to_owned(),
        key: key.to_owned(),
    };
    assert!(page.starts_with("p/"), "{url}");
    assert!(
        said.contains(&format!("ssh -N -L {port}:127.0.0.1:{port} <this-host>")),
        "{said}"
    );
    let local = [("X-Slink-Access", key), ("x-slink", "1")];
    // The page is that session, read as pi's.
    let id = page.strip_prefix("p/").unwrap();
    let (status, document) = send(port, "GET", &format!("/api/document/{id}"), &local, "");
    assert_eq!(status, 200);
    let document: serde_json::Value = serde_json::from_str(&document).unwrap();
    assert_eq!(
        document["source"]["harness"], "pi",
        "{}",
        document["source"]
    );
    assert_eq!(document["source"]["fidelity"], "reconstructed");
    // Nothing was recorded as a capture or sent anywhere: a preview is all
    // there is.
    assert!(
        home.files().iter().all(|file| !file.starts_with("runs/")),
        "{:?}",
        home.files()
    );
    assert_eq!(send(port, "POST", "/api/stop", &local, "").0, 200);

    // A session that cannot be read is said on stderr, with nothing printed
    // that could be taken for an address.
    let missing = view(&home.0.join("work/absent.jsonl").to_string_lossy());
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        text(&missing.stdout).is_empty(),
        "{}",
        text(&missing.stdout)
    );
    let why = text(&missing.stderr);
    assert!(
        why.contains("could not start the background viewer: no session")
            && why.contains("absent.jsonl"),
        "{why}"
    );
    home.assert_contained();
}

/// A view an agent opens from its own shell tool is of the session running
/// it, not of the one written to last. DeepSeek Harness says which that is,
/// in `DSH_SESSION_ID` beside `DSH_SHELL`.
#[test]
fn a_view_opened_from_inside_a_session_is_of_that_session() {
    let home = Home::new("dsh-active");
    let work = home.0.join("work");
    // dsh files a project's sessions under its path: each run of separators
    // as one dash, and any other character it does not write as it is as
    // `~` and its code unit.
    let mut key = String::new();
    let mut separators = false;
    for unit in work.to_string_lossy().encode_utf16() {
        match u8::try_from(unit) {
            Ok(b'/' | b'\\' | b':') => {
                if !separators {
                    key.push('-');
                }
                separators = true;
                continue;
            }
            Ok(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') => {
                key.push(char::from(byte));
            }
            _ => key.push_str(&format!("~{unit:04X}")),
        }
        separators = false;
    }
    let key = key.trim_start_matches('-');
    // The store is kept inside the working directory, so the test's home
    // holds nothing a command did not put there.
    let store = work.join("dsh-home");
    let project = store
        .join("sessions")
        .join(format!("--{}--", &key[..key.len().min(251)]));
    let now = std::time::SystemTime::now();
    for (id, age) in [("older-session", 3600), ("newer-session", 60)] {
        let header = serde_json::json!({
            "type": "session", "version": 4, "id": id, "createdAt": 1_791_451_208_304_i64,
            "cwd": work.to_string_lossy(), "isSeeded": false, "delegationDepth": 0,
        });
        let log = [
            header.to_string(),
            r#"{"type":"step/start","seq":6,"time":1791451208531,"data":{"turn":1,"step":1}}"#.to_owned(),
            r#"{"type":"user/message","seq":8,"time":1791451208534,"data":{"content":[{"type":"text","text":"Find the race in the upload test"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}"#.to_owned(),
            r#"{"type":"assistant/message","seq":15,"time":1791451208583,"data":{"turn":1,"step":1,"message":{"role":"assistant","content":[{"type":"text","text":"Looking."}],"source":{"kind":"model","provider":"deepseek-official","model":"deepseek-flash"},"id":"m2"}},"surfaceOp":"append"}"#.to_owned(),
        ];
        let file = project.join(id).join("session.v4.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, log.join("\n") + "\n").unwrap();
        let written = fs::File::options().write(true).open(&file).unwrap();
        written
            .set_modified(now - std::time::Duration::from_secs(age))
            .unwrap();
    }
    // What `view` opens: a session's page, by the id of the session it is
    // of, or the sessions page.
    let opened = |inside: &[(&str, &str)]| {
        let mut command = home.command(&["view", "--no-browser"]);
        command.env("DSH_HOME", &store).envs(inside.iter().copied());
        let viewer = Running::of(command);
        let of = match viewer.page.strip_prefix("/p/") {
            Some(id) => {
                let (status, document) = viewer.json("GET", &format!("/api/document/{id}"), "");
                assert_eq!(status, 200);
                document["metadata"]["session_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
            None => format!("the sessions page at {}", viewer.page),
        };
        viewer.stop();
        of
    };
    // Two sessions and no word on which: the person chooses.
    assert_eq!(opened(&[]), "the sessions page at /");
    let inside = [("DSH_SHELL", "1"), ("DSH_SESSION_ID", "older-session")];
    assert_eq!(opened(&inside), "older-session");
    // An id with no shell beside it, or of no session here, is no word.
    assert_eq!(opened(&inside[1..]), "the sessions page at /");
    assert_eq!(
        opened(&[("DSH_SHELL", "1"), ("DSH_SESSION_ID", "nobody")]),
        "the sessions page at /"
    );
    home.assert_contained();
}
