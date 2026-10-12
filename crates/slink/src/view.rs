//! `slink view`: a session as a saved local preview, read in the same viewer
//! the hosted service uses, served from this machine and nowhere else. This
//! is the command: which sessions it offers and how it starts the server in
//! `server.rs`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use slink_local::{
    Drafts, capture_dir, capture_id, display_path, list_captures, preview_dir, remember_preview,
    reusable_preview, save_preview, source_stamp, unix_now,
};
use tokio::sync::Notify;

use crate::flags::{Exit, Parsed, Run};
use crate::import::{check_harness, document};
use crate::picker::Row;
use crate::server::{Viewer, handle};
use crate::text::quoted;

/// A session this run can show.
pub struct Source {
    from: From,
    /// What the sessions page shows of it.
    pub row: Row,
    /// Whether it was found for this project and not named outright.
    of_project: bool,
}

/// Where a source's document comes from.
enum From {
    /// A capture or a transcript on disk, and the harness to read a
    /// transcript as; empty to recognize it.
    File(PathBuf, String),
    /// A session found in a harness's own history.
    Native(Box<slink_import::Candidate>),
}

impl Source {
    /// Saves a preview of the session and returns its id. A session found in
    /// a harness's files is not imported again while those files, and this
    /// binary, are what they were when its preview was made.
    pub fn preview(&self, previews: &Path) -> Result<String, String> {
        let From::Native(candidate) = &self.from else {
            return save_preview(previews, &self.document()?);
        };
        // The harness and the session's id name the source. The files alone
        // do not: every run in an Aider history is in the same file.
        let session = format!("{}\n{}", candidate.found.harness, candidate.id);
        let files: Vec<PathBuf> = candidate
            .file
            .iter()
            .chain(&candidate.related)
            .cloned()
            .collect();
        // Stamped before reading: a file that changes during the import
        // costs another import later, never a stale preview.
        let stamp = source_stamp(&build_stamp(), &session, &files);
        if let Some(stamp) = &stamp
            && let Some(id) = reusable_preview(previews, &session, &files, stamp)
        {
            return Ok(id);
        }
        let id = save_preview(previews, &self.document()?)?;
        if let Some(stamp) = &stamp {
            remember_preview(previews, &session, &files, stamp, &id);
        }
        Ok(id)
    }

    /// The session as a document's text.
    fn document(&self) -> Result<String, String> {
        match &self.from {
            From::File(file, harness) => document(file, harness),
            From::Native(candidate) => {
                let harness = candidate.found.harness;
                let input = candidate.found.load()?;
                match slink_import::import(harness, &input).map_err(|error| error.to_string())? {
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
        }
    }
}

pub fn run(parsed: &Parsed, args: &[String]) -> Run {
    let (session, span, from) = (
        parsed.text("session"),
        parsed.text("span"),
        parsed.text("from"),
    );
    if parsed.args.len() > 1 || (!session.is_empty() && !parsed.args.is_empty()) {
        return Err(Exit::failure(
            "pass one session reference, either as an argument or with --session",
        ));
    }
    let reference = if session.is_empty() {
        parsed.arg(0)
    } else {
        session
    };
    if parsed.bool("pick") && !reference.is_empty() {
        return Err(Exit::failure(
            "--pick cannot be combined with an explicit session",
        ));
    }
    let port = u16::try_from(parsed.int("port"))
        .map_err(|_| Exit::failure("--port must be between 0 and 65535"))?;
    if !span.is_empty() && !valid_span(span) {
        return Err(Exit::failure(
            "--span must be a span ID (1–64 letters, digits, dots, underscores, or hyphens)",
        ));
    }
    // The viewer that stays is a second run of this command, marked as the
    // child; this one waits for its URL and returns.
    let child = env::var_os("SLINK_VIEW_CHILD").is_some_and(|value| value == "1");
    if parsed.bool("background") && !child {
        let url = background(args)?;
        println!("{url}");
        let port = url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|rest| rest.split(['/', '#']).next())
            .and_then(|port| port.parse().ok());
        open_browser(&url, port.unwrap_or(0), parsed.bool("no-browser"));
        return Ok(());
    }
    check_harness(from)?;
    let cwd = env::current_dir().map_err(|error| Exit::failure(error.to_string()))?;
    let (sources, warning) =
        sources(&cwd, from, reference, !session.is_empty()).map_err(Exit::failure)?;
    if !warning.is_empty() {
        eprintln!("Some sessions could not be listed: {warning}");
    }
    let previews = preview_dir();
    let mut focus = String::new();
    let only =
        sources.len() == 1 && sources[0].of_project && !parsed.bool("pick") && warning.is_empty();
    if !reference.is_empty() || only {
        focus = format!(
            "/p/{}",
            sources[0].preview(&previews).map_err(Exit::failure)?
        );
    }
    if !span.is_empty() && focus.is_empty() {
        return Err(Exit::failure(
            "--span needs an explicit session when several sessions are available",
        ));
    }

    let mut key = [0; 32];
    // Never start an unprotected viewer because randomness failed.
    SystemRandom::new().fill(&mut key).map_err(|_| {
        Exit::failure("cannot start the local viewer: no randomness for its access key")
    })?;
    let viewer = Arc::new(Viewer {
        access: URL_SAFE_NO_PAD.encode(key),
        drafts: Drafts::beside(&previews),
        previews: previews.clone(),
        project: cwd,
        sources,
        warning,
        stop: Notify::new(),
        saving: std::sync::Mutex::new(()),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Exit::failure(error.to_string()))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|error| {
            Exit::failure(format!(
                "cannot start the local viewer: 127.0.0.1:{port}: {error}\n  try slink view --port 0 to use a free port"
            ))
        })?;
        let address = listener.local_addr().map_err(|error| Exit::failure(error.to_string()))?;
        // The key goes in the fragment, with the span beside it.
        let fragment = if span.is_empty() {
            format!("access={}", viewer.access)
        } else {
            format!("access={}&span={span}", viewer.access)
        };
        let url = format!("http://127.0.0.1:{}{focus}#{fragment}", address.port());
        // The URL alone on stdout, for whoever started this; the rest is for
        // a person and goes to stderr.
        println!("{url}");
        eprintln!(
            "Local preview ready · nothing uploaded\n  {url}\n  Saved previews: {}\n{}",
            display_path(&previews),
            if child {
                "Stop the viewer from the sessions page."
            } else {
                "Ctrl-C to stop the viewer."
            }
        );
        if !child {
            open_browser(&url, address.port(), parsed.bool("no-browser"));
        }
        let app = axum::Router::new().fallback(handle).with_state(viewer.clone());
        let stopped = viewer.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                tokio::select! {
                    () = stopped.stop.notified() => {}
                    () = interrupted() => {}
                }
            })
            .await
            .map_err(|error| Exit::failure(error.to_string()))
    })
}

/// Starts the viewer as a process of its own and returns the URL it prints,
/// once it is really listening. Its diagnostics go to a log in the slink
/// home, so whatever started this command can wait for it and move on.
fn background(args: &[String]) -> Result<String, Exit> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let failed = |error: std::io::Error| Exit::failure(error.to_string());
    let home = slink_local::home();
    let log = home.join(format!("viewer-{}-{}.log", std::process::id(), unix_now()));
    slink_local::write_private(&log, b"").map_err(failed)?;
    let errors = fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .map_err(failed)?;
    let mut command = Command::new(env::current_exe().map_err(failed)?);
    command
        .arg("view")
        .args(args)
        .env("SLINK_VIEW_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(errors);
    // In a group of its own, so a Ctrl-C in this terminal is not also its.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS and CREATE_NEW_PROCESS_GROUP.
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let mut viewer = command.spawn().map_err(failed)?;
    let output = viewer.stdout.take().expect("the viewer's stdout is piped");
    let (ready, first_line) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(output).read_line(&mut line);
        let _ = ready.send(line.trim().to_owned());
    });
    let url = first_line
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap_or_default();
    if !url.starts_with("http://127.0.0.1:") {
        let _ = viewer.kill();
        let _ = viewer.wait();
        return Err(Exit::failure(format!(
            "could not start the background viewer: {}\n  details: {}",
            fs::read_to_string(&log).unwrap_or_default().trim(),
            log.display()
        )));
    }
    eprintln!(
        "Local viewer running (PID {}). Stop it from the sessions page.\nLog: {}",
        viewer.id(),
        display_path(&log)
    );
    Ok(url)
}

/// What names this build: a preview an older importer made is not used
/// after the binary changes.
fn build_stamp() -> String {
    let Ok(exe) = env::current_exe() else {
        return String::new();
    };
    match fs::metadata(&exe).and_then(|found| Ok((found.len(), found.modified()?))) {
        Ok((size, modified)) => format!(
            "{} {size} {}",
            exe.display(),
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ),
        Err(_) => String::new(),
    }
}

/// Ctrl-C, or the signal a service manager or a parent sends to end it.
async fn interrupted() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = interrupt => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = interrupt.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}

fn valid_span(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn open_browser(url: &str, port: u16, disabled: bool) {
    let set = |name: &str| env::var_os(name).is_some_and(|value| !value.is_empty());
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        eprintln!(
            "Remote shell: forward this port from your own computer:\n  ssh -N -L {port}:127.0.0.1:{port} <this-host>\nThen open the printed URL locally."
        );
        return;
    }
    if disabled {
        return;
    }
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    if let Err(error) = command.arg(url).spawn() {
        eprintln!(
            "Could not open a browser: {error}\nOpen the printed URL manually; the preview is ready."
        );
    }
}

fn modified(file: &Path) -> i64 {
    fs::metadata(file)
        .and_then(|found| found.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_nanos()).ok())
        .unwrap_or(0)
}

fn named(file: &Path, harness: &str) -> Source {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Source {
        from: From::File(file.to_owned(), harness.to_owned()),
        row: Row {
            id: name,
            harness: harness.to_owned(),
            updated: modified(file),
            ..Row::default()
        },
        of_project: false,
    }
}

fn native(candidate: slink_import::Candidate) -> Source {
    Source {
        row: Row {
            id: candidate.id.clone(),
            harness: candidate.found.harness.to_owned(),
            dir: candidate.dir.to_string_lossy().into_owned(),
            name: candidate.name.clone(),
            prompt: candidate.prompt.clone(),
            updated: candidate.found.recency,
            started: candidate.started.map_or(0, |(seconds, nanos)| {
                seconds * 1_000_000_000 + i64::from(nanos)
            }),
        },
        of_project: !candidate.dir.as_os_str().is_empty(),
        from: From::Native(Box::new(candidate)),
    }
}

/// The sessions this run offers: the one referred to, or what is found for
/// the project in the agents' own histories and among the captures. The
/// second value is what could not be listed.
fn sources(
    cwd: &Path,
    from: &str,
    reference: &str,
    explicit: bool,
) -> Result<(Vec<Source>, String), String> {
    let locations = slink_import::Locations::default();
    if !reference.is_empty() {
        let path = Path::new(reference);
        if path.is_file() {
            let whole = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
            return Ok((vec![named(&whole, from)], String::new()));
        }
        if !explicit && from.is_empty() {
            let matches: Vec<PathBuf> = list_captures(&capture_dir())
                .into_iter()
                .filter(|capture| capture_id(&capture.file) == reference)
                .map(|capture| capture.file)
                .collect();
            match matches.as_slice() {
                [only] => return Ok((vec![named(only, "")], String::new())),
                [] => {}
                _ => {
                    return Err(format!(
                        "session ID {} matches multiple captures; pass a full path",
                        quoted(reference)
                    ));
                }
            }
        }
        let (mut candidates, problems) = slink_import::recent(&locations, from, cwd, reference, 30);
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        return match candidates.len() {
            0 => Err(format!(
                "no session {} for this project; pass its transcript path, or use slink view --pick",
                quoted(reference)
            )),
            1 => Ok((vec![native(candidates.remove(0))], String::new())),
            _ => Err(format!(
                "session ID {} is ambiguous; add --from or pass a transcript path",
                quoted(reference)
            )),
        };
    }
    // A view an agent opens from its own shell tool is of the session that
    // is running it, when that session is one of this project's.
    if let Some((harness, id)) = slink_import::active_session()
        && (from.is_empty() || from == harness)
    {
        let (mut active, problems) = slink_import::recent(&locations, harness, cwd, &id, 30);
        if problems.is_empty() && active.len() == 1 {
            return Ok((vec![native(active.remove(0))], String::new()));
        }
    }
    let (candidates, problems) = slink_import::recent(&locations, from, cwd, "", 30);
    if !problems.is_empty() && !from.is_empty() {
        return Err(problems.join("\n"));
    }
    let mut found: Vec<Source> = candidates.into_iter().map(native).collect();
    if !from.is_empty() {
        return Ok((found, String::new()));
    }
    let dirs = slink_import::project_dirs(cwd);
    for capture in list_captures(&capture_dir()) {
        // A capture belongs here when it was recorded in one of the
        // project's directories. The always-on recorder serves every shell
        // and records none, so a capture that says nowhere is offered
        // everywhere.
        let recorded = fs::read_to_string(&capture.file)
            .ok()
            .and_then(|text| slink_format::parse(&text).ok())
            .and_then(|document| document["metadata"]["cwd"].as_str().map(str::to_owned))
            .unwrap_or_default();
        if !recorded.is_empty()
            && recorded != "/"
            && !dirs.iter().any(|dir| dir == Path::new(&recorded))
        {
            continue;
        }
        found.push(Source {
            row: Row {
                id: capture
                    .file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                dir: recorded.clone(),
                name: capture.name.clone(),
                updated: modified(&capture.file),
                ..Row::default()
            },
            from: From::File(capture.file, String::new()),
            of_project: !recorded.is_empty(),
        });
    }
    found.sort_by_key(|source| std::cmp::Reverse(source.row.updated));
    Ok((found, problems.join("\n")))
}
