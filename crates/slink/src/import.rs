//! `slink import`: a coding agent's own record of a session, read as a
//! capture. Nothing is run again; the capture is marked `reconstructed`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use slink_import::{Found, HARNESSES, Input};
use slink_local::{
    ago, capture_dir, capture_id, display_path, list_captures, new_capture_path, stamp, unix_now,
    write_private,
};

use crate::flags::{Exit, Parsed, Run};
use crate::text::{clip, plural, quoted};

/// What an import left behind.
pub struct Imported {
    pub file: PathBuf,
}

pub fn run(parsed: &Parsed) -> Run {
    let imported = import_to_file(parsed.text("from"), parsed.text("session"), parsed.arg(0))?;
    let id = capture_id(&imported.file);
    eprintln!("\n  review:  slink view {id}\n  publish: slink share {id}");
    // No bare path on stdout: the stored line is for people, and scripts get
    // structured paths from `slink list --json`.
    Ok(())
}

fn known() -> String {
    HARNESSES.join(", ")
}

/// Whether a harness name is one of the eight; `Err` says which are.
pub fn check_harness(from: &str) -> Result<(), Exit> {
    if !from.is_empty() && !HARNESSES.contains(&from) {
        return Err(Exit::failure(format!(
            "unknown agent {} — known: {}",
            quoted(from),
            known()
        )));
    }
    Ok(())
}

/// What was resolved for an import: the harness, its input, the session it
/// was found as when it was not named by path, and the directory matched.
struct Resolved {
    harness: String,
    input: Input,
    found: Option<Found>,
    matched: Option<PathBuf>,
}

fn resolve(from: &str, session: &str, arg: &str) -> Result<Resolved, Exit> {
    let target = if arg.is_empty() { session } else { arg };
    let cwd = env::current_dir().unwrap_or_default();
    let dirs = slink_import::project_dirs(&cwd);
    if !from.is_empty() && !slink_import::ported(from) {
        return Err(Exit::failure(
            slink_import::Error::NotPorted(from.to_owned()).to_string(),
        ));
    }
    if !target.is_empty() {
        // A harness with a database has no file to name: --session is the id
        // it gave the session.
        if let Some(found) = slink_import::latest_by_id(from, target) {
            let slink_import::Loader::Database { path, .. } = &found.loader else {
                unreachable!("a database session is loaded from its database");
            };
            if !path.is_file() {
                // The agent's database is not there: say that, not what
                // opening it failed with.
                return Err(Exit::failure(format!(
                    "✗ no {from} database found on this machine\n\n  is {from} installed here? its sessions can only be imported where it runs"
                )));
            }
            let input = found.load().map_err(Exit::failure)?;
            return Ok(Resolved {
                harness: from.to_owned(),
                input,
                found: Some(found),
                matched: None,
            });
        }
        let mut file = PathBuf::from(target);
        if !file.is_file() {
            // An Aider run has no file of its own: its id is the time it
            // started, in the project's chat history.
            let run = (from.is_empty() || from == "aider")
                .then(|| {
                    dirs.iter()
                        .find_map(|dir| slink_import::aider_run(dir, target))
                })
                .flatten();
            if let Some(found) = run {
                let input = found.load().map_err(Exit::failure)?;
                return Ok(Resolved {
                    harness: "aider".to_owned(),
                    input,
                    found: Some(found),
                    matched: None,
                });
            }
            // A pasted session id is worth trying before giving up: Claude
            // Code and dsh name a session's file or directory by it.
            let by_id = (!target.contains(['/', '\\', '.']))
                .then(|| {
                    dirs.iter()
                        .find_map(|dir| slink_import::transcript_by_id(dir, target))
                })
                .flatten();
            file = by_id.ok_or_else(|| {
                Exit::failure(format!(
                    "cannot read {target} — pass a transcript path, or a session id from your agent's history"
                ))
            })?;
        }
        let (input, detected) = slink_import::load_file(&file).map_err(Exit::failure)?;
        let harness = if from.is_empty() { detected } else { from };
        if harness.is_empty() {
            return Err(Exit::failure(format!(
                "could not detect the agent for {target} — pass --from"
            )));
        }
        return Ok(Resolved {
            harness: harness.to_owned(),
            input,
            found: None,
            matched: None,
        });
    }
    // Running from a subdirectory of the project is the common case, and the
    // agents' histories are keyed on the project's root.
    for dir in dirs {
        if let Some(found) = slink_import::latest(from, &dir) {
            let input = found.load().map_err(Exit::failure)?;
            return Ok(Resolved {
                harness: found.harness.to_owned(),
                input,
                found: Some(found),
                matched: Some(dir),
            });
        }
    }
    Err(Exit::failure(no_session_hint(from, &cwd)))
}

/// Import's empty state. "Nothing to import here" is three situations, and
/// the binary can tell which one this is, so it says so with one next step.
fn no_session_hint(from: &str, cwd: &Path) -> String {
    let record = "  or record fresh here:  slink record -- <your agent command>";
    if let Some(elsewhere) = slink_import::newest_anywhere(from) {
        // History exists, only not for this project: point at the newest
        // session elsewhere, with a way to it that can be pasted.
        let kind = if from.is_empty() { "agent" } else { from };
        let head = format!(
            "✗ no {kind} session for this project ({} or its parents)",
            display_path(cwd)
        );
        let described = if elsewhere.title.is_empty() {
            elsewhere.harness.to_owned()
        } else {
            format!(
                "{} · {}",
                quoted(&clip(&elsewhere.title, 48)),
                elsewhere.harness
            )
        };
        let when = if elsewhere.recency > 0 {
            format!(
                " · {}",
                ago(&rfc3339(elsewhere.recency / 1_000_000_000), unix_now())
            )
        } else {
            String::new()
        };
        let (place, next) = if elsewhere.dir.is_empty() {
            (
                String::new(),
                elsewhere.file.as_deref().map_or(String::new(), |file| {
                    format!("    → slink import {}", shell_path(file))
                }),
            )
        } else {
            let flag = if from.is_empty() {
                String::new()
            } else {
                format!(" --from {from}")
            };
            (
                format!(" · {}", display_path(Path::new(&elsewhere.dir))),
                format!(
                    "    → cd {} && slink import{flag}",
                    shell_path(Path::new(&elsewhere.dir))
                ),
            )
        };
        return format!(
            "{head}\n\n  your newest session elsewhere:\n    {described}{place}{when}\n{next}\n\n{record}"
        );
    }
    // No history for this agent, or for any agent, at all.
    if from.is_empty() {
        format!(
            "✗ no coding-agent history found on this machine\n  (looked for: {})\n\n  record a session instead:  slink record -- <your agent command>",
            known()
        )
    } else {
        format!(
            "✗ no {from} history found on this machine\n  (known agents: {})\n\n  record a session instead:  slink record -- <your agent command>",
            known()
        )
    }
}

/// A time in seconds as RFC 3339, for the one function that words an age.
fn rfc3339(unix: i64) -> String {
    let stamp = stamp(unix);
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &stamp[..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    )
}

/// A path to paste into a shell: with `~` when that is safe, and in double
/// quotes, whole, when it holds a space or a quote.
fn shell_path(path: &Path) -> String {
    let text = path.display().to_string();
    if text.contains([' ', '\'', '"', '\\', '$', '`']) {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        display_path(path)
    }
}

/// Reads the session meant, and writes it as a capture unless one with the
/// same bytes is already there.
pub fn import_to_file(from: &str, session: &str, arg: &str) -> Result<Imported, Exit> {
    check_harness(from)?;
    let resolved = resolve(from, session, arg)?;
    let harness = resolved.harness.as_str();
    if harness == "hermes" {
        eprintln!("note: hermes import is experimental — its mapping is inferred, not dogfooded");
    }
    let run = slink_import::import(harness, &resolved.input)
        .map_err(|error| Exit::failure(error.to_string()))?;
    let spans = run
        .as_ref()
        .and_then(|run| run["spans"].as_array())
        .map_or(0, Vec::len);
    let Some(run) = run.filter(|_| spans > 0) else {
        // An empty import fails loudly: a hollow capture is one that a
        // listing skips and a publish refuses.
        let source = if arg.is_empty() { session } else { arg };
        return Err(Exit::failure(if source.is_empty() {
            format!("nothing to import — the newest {harness} session has no messages")
        } else {
            format!("nothing to import from {source} — the transcript has no messages")
        }));
    };
    let text = slink_format::encode(&run);
    let name = run["name"].as_str().unwrap_or_default();
    let created = run["created_at"].as_str().unwrap_or_default();
    // Importing the same unchanged session again is common, so the capture
    // with the same bytes is used again and the store is not littered.
    if let Some(previous) = identical(&text) {
        eprintln!(
            "✓ already imported, unchanged — reusing {}",
            capture_id(&previous)
        );
        eprintln!("    stored:  {}", display_path(&previous));
        return Ok(Imported { file: previous });
    }
    let mut random = [0; 3];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| Exit::failure("write: no randomness for the capture's name"))?;
    let now = unix_now();
    let file = new_capture_path(&capture_dir(), now, random);
    write_private(&file, text.as_bytes())
        .map_err(|error| Exit::failure(format!("write: {error}")))?;

    let age = match &resolved.found {
        Some(found) if found.recency > 0 => {
            format!(
                " · last active {}",
                ago(&rfc3339(found.recency / 1_000_000_000), now)
            )
        }
        _ if !created.is_empty() => format!(" · started {}", ago(created, now)),
        _ => String::new(),
    };
    let cwd = env::current_dir().unwrap_or_default();
    let place = match &resolved.matched {
        Some(matched) if *matched != cwd => format!(" · {}", display_path(matched)),
        _ => String::new(),
    };
    // One meaning on each line: what happened, what it is, what it holds
    // and where it lives.
    eprintln!("✓ imported from {harness}{place} (reconstructed)");
    eprintln!("    {}", quoted(&clip(name, 76)));
    eprintln!("    {}{age}", plural(spans, "span"));
    eprintln!("    stored:  {}", display_path(&file));
    Ok(Imported { file })
}

/// A capture among the newest fifty that already holds exactly this text.
fn identical(text: &str) -> Option<PathBuf> {
    list_captures(&capture_dir())
        .into_iter()
        .take(50)
        .map(|capture| capture.file)
        .find(|file| {
            fs::metadata(file).is_ok_and(|found| found.len() == text.len() as u64)
                && fs::read(file).is_ok_and(|found| found == text.as_bytes())
        })
}

/// The document a file holds: a capture as it is, or a transcript imported.
/// What `slink view` reads when it is handed a path.
pub fn document(file: &Path, from: &str) -> Result<String, String> {
    let text = fs::read_to_string(file).map_err(|_| {
        format!(
            "cannot read session {}",
            quoted(&file.display().to_string())
        )
    })?;
    if let Ok(document) = slink_format::parse(&text)
        && matches!(document["schema"].as_str(), Some("session/v0" | "run/v0"))
    {
        return Ok(text);
    }
    let (input, detected) = slink_import::load_file(file)?;
    let harness = if from.is_empty() { detected } else { from };
    if harness.is_empty() {
        return Err(format!(
            "cannot identify the agent for {}; pass --from",
            quoted(&file.display().to_string())
        ));
    }
    let run: Option<Value> =
        slink_import::import(harness, &input).map_err(|error| error.to_string())?;
    match run {
        Some(run)
            if run["spans"]
                .as_array()
                .is_some_and(|spans| !spans.is_empty()) =>
        {
            Ok(slink_format::encode(&run))
        }
        _ => Err(format!("this {harness} session has no messages yet")),
    }
}
