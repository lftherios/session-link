//! `slink push` and `slink share`: a session checked here, shown, confirmed
//! and published as an encrypted link. Nothing is sent before the person
//! says yes, or passes `--yes`, and only ciphertext is ever sent.

use std::env;
use std::fs;
use std::future::Future;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use slink_client::{DEFAULT_SERVER, resolve_target};
use slink_local::{
    Capture, ago, ago_unix, capture_dir, capture_id, display_path, home, list_captures, unix_now,
};

use crate::account;
use crate::flags::{Exit, Parsed, Run};
use crate::import::{import_to_file, latest_with_walk_up, no_session_hint};
use crate::list::{EMPTY, resolve_capture, row};
use crate::text::{clip, fmt_bytes, plural, quoted};
use crate::view::modified;

const NOT_SIGNED_IN: &str =
    "not signed in — run: slink login  (publishing needs an account; capture doesn't)";

/// Runs `future` to its end on a runtime of its own.
pub fn block_on<F: Future>(future: F) -> Result<F::Output, Exit> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(future))
        .map_err(|error| Exit::failure(error.to_string()))
}

/// Whether a person is at the keyboard to answer a question.
pub fn interactive() -> bool {
    io::stdin().is_terminal()
}

/// One line typed in answer, without its line break.
pub fn answer() -> String {
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    line.trim().to_owned()
}

/// Whether the answer to a `[y/N]` question was yes.
pub fn yes(line: &str) -> bool {
    matches!(line.to_lowercase().as_str(), "y" | "yes")
}

pub fn push(parsed: &Parsed) -> Run {
    let file = if !parsed.arg(0).is_empty() {
        resolve_capture(parsed.arg(0))?
    } else {
        let captures = list_captures(&capture_dir());
        let Some(newest) = captures.first() else {
            return Err(Exit::failure(format!(
                "{EMPTY}\n  (or pass a session/v0 .json path)"
            )));
        };
        if parsed.bool("pick") {
            pick_session(&captures, "publish which?")?
        } else {
            eprintln!("publishing your newest session:");
            eprintln!("{}", row(newest, unix_now()));
            newest.file.clone()
        }
    };
    let (target, key) = resolve_target(&home(), parsed.text("server"), parsed.text("key"));
    // The default server always wants an account, so say so before any
    // bytes move. Another server may take uploads without one; it decides.
    if key.is_empty() && target == DEFAULT_SERVER {
        return Err(Exit::failure(NOT_SIGNED_IN));
    }
    publish_file(&file, &target, &key, parsed.bool("yes"))
}

/// The picker every publishing command shares: the rows `slink list`
/// prints, and any id from it as the answer.
fn pick_session(captures: &[Capture], prompt: &str) -> Result<PathBuf, Exit> {
    if !interactive() {
        return Err(Exit::failure("--pick needs a TTY"));
    }
    let now = unix_now();
    for capture in captures.iter().take(10) {
        eprintln!("{}", row(capture, now));
    }
    if captures.len() > 10 {
        eprintln!(
            "  … and {} more — any id from `slink list` works too",
            captures.len() - 10
        );
    }
    eprint!("{prompt} [{}] ", capture_id(&captures[0].file));
    let reference = answer();
    if reference.is_empty() {
        return Ok(captures[0].file.clone());
    }
    resolve_capture(&reference)
}

pub fn share(parsed: &Parsed) -> Run {
    let (from, session) = (parsed.text("from"), parsed.text("session"));
    let file = if !parsed.arg(0).is_empty() || !session.is_empty() {
        share_target(parsed.arg(0), session, from)?
    } else if parsed.bool("pick") {
        share_pick(from)?
    } else {
        share_newest(from)?
    };
    let (target, mut key) = resolve_target(&home(), parsed.text("server"), parsed.text("key"));
    // An account is asked for at the moment it is needed and not before.
    // A script gets the error instead of a browser.
    if key.is_empty() && target == DEFAULT_SERVER {
        if !interactive() {
            return Err(Exit::failure(NOT_SIGNED_IN));
        }
        eprintln!("Sign-in required — publishing needs a free account (capture doesn't).");
        let signed = account::browser_login(&target)?;
        eprintln!("✓ signed in as @{}", signed.login);
        key = resolve_target(&home(), parsed.text("server"), parsed.text("key")).1;
    }
    publish_file(&file, &target, &key, parsed.bool("yes"))
}

/// The newest local session or the agent's newest for this project,
/// whichever was active last, saying which. `--from` always means the
/// agent: a local capture behind that flag is not what was asked for.
fn share_newest(from: &str) -> Result<PathBuf, Exit> {
    let cwd = env::current_dir().unwrap_or_default();
    let agent = latest_with_walk_up(from, &cwd);
    if !from.is_empty() {
        let Some((found, _)) = agent else {
            return Err(Exit::failure(no_session_hint(from, &cwd)));
        };
        eprintln!(
            "Found the latest {} session in this project:",
            found.harness
        );
        return Ok(import_to_file(from, "", "")?.file);
    }
    let captures = list_captures(&capture_dir());
    // Last activity on both sides: the transcript's time against the
    // capture file's, since a recording's start would lose to any
    // transcript written after it.
    let local = captures.first().map_or(0, |capture| {
        let started = slink_format::instant(&capture.created_at).map_or(0, |(seconds, nanos)| {
            seconds * 1_000_000_000 + i64::from(nanos)
        });
        started.max(modified(&capture.file))
    });
    let agent_newer = agent
        .as_ref()
        .is_some_and(|(found, _)| found.recency > local);
    match (&agent, captures.first()) {
        (None, None) => Err(Exit::failure(EMPTY)),
        (Some((found, matched)), newest) if agent_newer || newest.is_none() => {
            let mut label = found.harness.to_owned();
            if *matched != cwd {
                label.push_str(&format!(" (from {})", matched.display()));
            }
            eprintln!("Found the latest {label} session in this project:");
            Ok(import_to_file(found.harness, "", "")?.file)
        }
        (_, Some(newest)) => {
            eprintln!("Sharing your newest local session:");
            eprintln!("{}", row(newest, unix_now()));
            Ok(newest.file.clone())
        }
        (Some(_), None) => unreachable!("the agent's session is taken when there is no capture"),
    }
}

/// A session named outright: a capture by id or path, published as it is,
/// or a transcript or a session id of an agent, imported first.
fn share_target(arg: &str, session: &str, from: &str) -> Result<PathBuf, Exit> {
    let reference = if arg.is_empty() { session } else { arg };
    if session.is_empty()
        && from.is_empty()
        && let Ok(file) = resolve_capture(reference)
        && session_document(&file)
    {
        return Ok(file);
    }
    Ok(import_to_file(from, session, arg)?.file)
}

/// Whether a file is a `session/v0` document rather than a transcript, by
/// its first 4 KiB.
fn session_document(file: &Path) -> bool {
    let mut head = Vec::with_capacity(4096);
    let read = fs::File::open(file).and_then(|handle| handle.take(4096).read_to_end(&mut head));
    let head = String::from_utf8_lossy(&head);
    read.is_ok() && (head.contains("\"session/v0\"") || head.contains("\"run/v0\""))
}

/// The local sessions and the agent's newest in one list, and one choice.
fn share_pick(from: &str) -> Result<PathBuf, Exit> {
    let captures = list_captures(&capture_dir());
    let cwd = env::current_dir().unwrap_or_default();
    let agent = latest_with_walk_up(from, &cwd);
    if captures.is_empty() && agent.is_none() {
        return Err(Exit::failure(EMPTY));
    }
    if !interactive() {
        return Err(Exit::failure("--pick needs a TTY"));
    }
    let now = unix_now();
    if let Some((found, _)) = &agent {
        eprintln!(
            "  {:<7} {:<9} your {} agent's newest session for this project (import + publish)",
            "agent",
            ago_unix(found.recency / 1_000_000_000, now),
            found.harness
        );
    }
    for capture in captures.iter().take(10) {
        eprintln!("{}", row(capture, now));
    }
    if captures.len() > 10 {
        eprintln!(
            "  … and {} more — any id from `slink list` works too",
            captures.len() - 10
        );
    }
    let default = match &agent {
        Some(_) => "agent".to_owned(),
        None => capture_id(&captures[0].file),
    };
    eprint!("share which? [{default}] ");
    let mut reference = answer();
    if reference.is_empty() {
        reference = default;
    }
    if reference == "agent" {
        let Some((found, _)) = agent else {
            return Err(Exit::failure("no agent session found for this project"));
        };
        return Ok(import_to_file(found.harness, "", "")?.file);
    }
    resolve_capture(&reference)
}

/// What a file holds, read and checked before anything is sent.
pub struct Inspection {
    pub text: String,
    pub name: String,
    pub created_at: String,
    pub fidelity: String,
    pub spans: usize,
    pub models: Vec<String>,
    pub in_progress: bool,
    pub errors: Vec<String>,
    pub secrets: Vec<slink_format::Hit>,
}

/// Reads, validates and scans a session file. Nothing is uploaded.
pub fn inspect(file: &Path) -> Result<Inspection, String> {
    let shown = file.display();
    let text = fs::read_to_string(file).map_err(|_| format!("cannot read {shown}"))?;
    let document = slink_format::parse(&text).map_err(|_| format!("{shown} is not valid JSON"))?;
    let field = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let spans = document["spans"].as_array().map_or(&[][..], Vec::as_slice);
    let mut models: Vec<String> = Vec::new();
    for span in spans.iter().filter(|span| span["type"] == "llm_call") {
        if let Some(id) = span["model"]["id"].as_str().filter(|id| !id.is_empty())
            && !models.iter().any(|seen| seen == id)
        {
            models.push(id.to_owned());
        }
    }
    let errors = slink_format::validate(&document);
    let secrets = if errors.is_empty() {
        slink_format::scan_for_secrets(&text)
    } else {
        Vec::new()
    };
    Ok(Inspection {
        name: field(&document["name"]),
        created_at: field(&document["created_at"]),
        fidelity: field(&document["source"]["fidelity"]),
        spans: spans.len(),
        models,
        in_progress: document["metadata"]["in_progress"] == true,
        errors,
        secrets,
        text,
    })
}

/// The facts of what is about to be published, under its title.
fn gate_summary(inspection: &Inspection) -> String {
    let mut parts = vec![plural(inspection.spans, "span")];
    if !inspection.models.is_empty() {
        parts.push(inspection.models.join(", "));
    }
    parts.push(fmt_bytes(inspection.text.len()));
    if !inspection.fidelity.is_empty() {
        parts.push(inspection.fidelity.clone());
    }
    if !inspection.created_at.is_empty() {
        parts.push(format!(
            "captured {}",
            ago(&inspection.created_at, unix_now())
        ));
    }
    parts.join(" · ")
}

/// Checks, shows, confirms and publishes one file: what `push` and `share`
/// have in common.
fn publish_file(file: &Path, target: &str, api_key: &str, assume_yes: bool) -> Run {
    let inspection = inspect(file).map_err(Exit::failure)?;
    if !inspection.errors.is_empty() {
        let mut message = format!(
            "✗ not a valid session/v0 document\n    file:  {}",
            display_path(file)
        );
        for error in &inspection.errors {
            message.push_str(&format!("\n    {error}"));
        }
        return Err(Exit::failure(message));
    }
    let name = if inspection.name.is_empty() {
        file.display().to_string()
    } else {
        inspection.name.clone()
    };
    eprintln!("{}", quoted(&clip(&name, 76)));
    eprintln!("  {}", gate_summary(&inspection));
    if !inspection.secrets.is_empty() {
        let mut message =
            "✗ publish blocked — the session appears to contain credentials:".to_owned();
        for hit in &inspection.secrets {
            message.push_str(&format!("\n    {}  {}", hit.pattern, hit.preview));
        }
        let whole = std::path::absolute(file).unwrap_or_else(|_| file.to_owned());
        message.push_str(&format!(
            "\n  redact the local file and push again:\n    {}",
            whole.display()
        ));
        return Err(Exit::failure(message));
    }
    eprintln!("✓ no credentials detected");
    if inspection.in_progress {
        eprintln!("  still recording — publishing a snapshot");
    }
    if inspection.text.len() > 25 * 1024 * 1024 {
        eprintln!("  exceeds the server's 25MB ingest cap — this will likely be rejected");
    }
    if !assume_yes {
        if !interactive() {
            return Err(Exit::failure(
                "not a TTY — pass --yes to publish from scripts or CI",
            ));
        }
        let login = slink_client::config::read(&home()).login;
        let signed = if login.is_empty() {
            String::new()
        } else {
            format!(" as {login}")
        };
        eprintln!("\nready to publish to {target}{signed}");
        eprintln!("  encrypted — anyone with the complete link can view it");
        eprint!("\nPublish? [y/N] ");
        let _ = io::stderr().flush();
        if !yes(&answer()) {
            // A script must be able to tell a decline from a publish.
            return Err(Exit::failure("aborted — nothing was uploaded"));
        }
    }

    let upload = block_on(slink_client::upload(
        &home(),
        &inspection.text,
        target,
        api_key,
    ))?;
    if !upload.ok {
        let message = upload.body["error"]["message"]
            .as_str()
            .unwrap_or("upload failed");
        return Err(Exit::failure(match upload.status {
            0 => format!(
                "✗ publish failed for {target} ({})\n    retry to resume any pending encrypted upload",
                transport_cause(message)
            ),
            401 => "✗ the server refused the upload — not signed in (HTTP 401)\n\n  sign in:  slink login\n  or pass:  --key rk_…".to_owned(),
            status => format!("✗ {message} (HTTP {status})"),
        }));
    }
    let url = upload.body["url"].as_str().unwrap_or_default();
    // The service answers 200 for bytes it already has: the same link.
    let verb = if upload.status == 200 {
        "already published"
    } else {
        "published"
    };
    let copied = if clipboard(url) { "  (URL copied)" } else { "" };
    eprintln!("✓ {verb}{copied}");
    println!("{url}");
    eprintln!("  anyone with this link can view it");
    let id = published_id(url);
    if !id.is_empty() {
        eprintln!("\n  take offline:  slink delete s/{id}");
    }
    Ok(())
}

/// The last cause of a wrapped transport error; the server is already on
/// the line.
pub fn transport_cause(message: &str) -> &str {
    message
        .rsplit_once(": ")
        .map_or(message, |(_, cause)| cause)
}

/// The id in a published link, `…/s/<id>` or `…/r/<id>`, without its query,
/// fragment or trailing slash.
pub fn published_id(link: &str) -> String {
    for prefix in ["/r/", "/s/"] {
        if let Some(index) = link.rfind(prefix) {
            let rest = &link[index + 3..];
            let rest = rest.split('#').next().unwrap_or_default();
            let rest = rest.split('?').next().unwrap_or_default();
            return rest.trim_end_matches('/').to_owned();
        }
    }
    String::new()
}

/// Puts `text` on the clipboard, when the system has a way to. A link is
/// printed whether or not it was copied.
fn clipboard(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let candidates: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else if cfg!(windows) {
        &[&["clip"]]
    } else {
        &[&["wl-copy"], &["xclip", "-selection", "clipboard"]]
    };
    candidates.iter().any(|candidate| {
        let Ok(mut child) = Command::new(candidate[0])
            .args(&candidate[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
        child.wait().is_ok_and(|status| status.success()) && written
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_of_a_published_link() {
        for (link, id) in [
            (
                "https://session.link/s/23456789abcdef#key=abc",
                "23456789abcdef",
            ),
            (
                "https://session.link/r/9f3kx2mvq7wtd4/?x=1",
                "9f3kx2mvq7wtd4",
            ),
            ("https://session.link/s/abc/r/def", "def"),
            ("https://session.link/", ""),
        ] {
            assert_eq!(published_id(link), id, "{link}");
        }
    }

    #[test]
    fn a_transport_error_is_cut_to_its_cause() {
        assert_eq!(
            transport_cause("dial tcp: connect: connection refused"),
            "connection refused"
        );
        assert_eq!(transport_cause("no such host"), "no such host");
    }
}
