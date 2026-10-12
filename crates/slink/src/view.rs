//! `slink view`: a session as a saved local preview, read in the same viewer
//! the hosted service uses, served from this machine and nowhere else.
//!
//! The server listens on loopback only. A page is given only to a request
//! that carries the access key, which is made for each run and travels in
//! the URL's fragment, where no server log, proxy or other local port sees
//! it. Publishing is not in this binary yet, so nothing here uploads.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};
use slink_local::{
    READING_SUFFIX, capture_dir, capture_id, display_path, list_captures, preview_dir, save_preview,
};
use tokio::sync::Notify;

use crate::flags::{Exit, Parsed, Run};
use crate::import::{check_harness, document};
use crate::text::quoted;
use crate::viewer;

const CSS: &str = include_str!("assets/page.css");
const ACCESS_SHELL: &str = include_str!("assets/access-shell.html");
/// The script that carries the access key from the fragment to each request.
/// It is the Go CLI's, read from where that keeps it.
const LOCAL_ACCESS_JS: &str = include_str!("../../../go/internal/open/local-access.js");
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'self'; base-uri 'none'; object-src 'none'; form-action 'none'; frame-ancestors 'none'";
/// The name an excerpt carries its selection under.
const EXCERPT: &str = "session_link.share.v1";

/// A session this run can show: a capture or a transcript on disk.
struct Source {
    file: PathBuf,
    /// The harness to read a transcript as; empty to recognize it.
    harness: String,
    title: String,
    detail: String,
    /// Whether it was found for this project and not named outright.
    of_project: bool,
    updated: u128,
}

struct Viewer {
    access: String,
    previews: PathBuf,
    project: PathBuf,
    sources: Vec<Source>,
    stop: Notify,
}

pub fn run(parsed: &Parsed) -> Run {
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
    if parsed.bool("background") {
        return Err(Exit::failure(
            "slink-rs: `view --background` is not ported yet",
        ));
    }
    check_harness(from)?;
    let cwd = env::current_dir().map_err(|error| Exit::failure(error.to_string()))?;
    let sources = sources(&cwd, from, reference, !session.is_empty()).map_err(Exit::failure)?;
    let previews = preview_dir();
    let mut focus = String::new();
    let only = sources.len() == 1 && sources[0].of_project && !parsed.bool("pick");
    if !reference.is_empty() || only {
        let text = document(&sources[0].file, &sources[0].harness).map_err(Exit::failure)?;
        focus = format!(
            "/p/{}",
            save_preview(&previews, &text).map_err(Exit::failure)?
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
        previews: previews.clone(),
        project: cwd,
        sources,
        stop: Notify::new(),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Exit::failure(error.to_string()))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|error| {
            Exit::failure(format!(
                "cannot start the local viewer: {error}\n  try slink view --port 0 to use a free port"
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
            "Local preview ready · nothing uploaded\n  {url}\n  Saved previews: {}\nCtrl-C to stop the viewer.",
            display_path(&previews)
        );
        open_browser(&url, address.port(), parsed.bool("no-browser"));
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

fn modified(file: &Path) -> u128 {
    fs::metadata(file)
        .and_then(|found| found.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos())
}

fn named(file: &Path, harness: &str) -> Source {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Source {
        file: file.to_owned(),
        harness: harness.to_owned(),
        title: name,
        detail: String::new(),
        of_project: false,
        updated: modified(file),
    }
}

/// The sessions this run offers: the one referred to, or what is found for
/// the project in the agent's own history and among the captures.
fn sources(cwd: &Path, from: &str, reference: &str, explicit: bool) -> Result<Vec<Source>, String> {
    let dirs = slink_import::project_dirs(cwd);
    if !reference.is_empty() {
        let path = Path::new(reference);
        if path.is_file() {
            let whole = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
            return Ok(vec![named(&whole, from)]);
        }
        if !explicit && from.is_empty() {
            let matches: Vec<PathBuf> = list_captures(&capture_dir())
                .into_iter()
                .filter(|capture| capture_id(&capture.file) == reference)
                .map(|capture| capture.file)
                .collect();
            match matches.as_slice() {
                [only] => return Ok(vec![named(only, "")]),
                [] => {}
                _ => {
                    return Err(format!(
                        "session ID {} matches multiple captures; pass a full path",
                        quoted(reference)
                    ));
                }
            }
        }
        if (from.is_empty() || from == "claude-code")
            && let Some(transcript) = dirs
                .iter()
                .find_map(|dir| slink_import::claude_transcript(dir, reference))
        {
            return Ok(vec![named(&transcript, "claude-code")]);
        }
        return Err(format!(
            "no session {} for this project; pass its transcript path, or use slink view --pick",
            quoted(reference)
        ));
    }
    if !from.is_empty() && !slink_import::ported(from) {
        return Err(slink_import::Error::NotPorted(from.to_owned()).to_string());
    }
    let mut found = Vec::new();
    if let Some(latest) = dirs.iter().find_map(|dir| slink_import::latest(from, dir)) {
        found.push(Source {
            title: latest
                .file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            detail: "Claude Code".into(),
            file: latest.file,
            harness: latest.harness.to_owned(),
            of_project: true,
            updated: latest.recency,
        });
    }
    if from.is_empty() {
        for capture in list_captures(&capture_dir()) {
            // A capture belongs here when it was recorded in one of the
            // project's directories, or says nowhere at all.
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
                title: if capture.name.is_empty() {
                    capture_id(&capture.file)
                } else {
                    capture.name.clone()
                },
                detail: format!("capture {}", capture_id(&capture.file)),
                updated: modified(&capture.file),
                file: capture.file,
                harness: String::new(),
                of_project: !recorded.is_empty(),
            });
        }
    }
    found.sort_by_key(|source| std::cmp::Reverse(source.updated));
    Ok(found)
}

/* ------------------------------------------------------------- the server */

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

fn asset(name: &str) -> &'static str {
    viewer::file(name)
        .and_then(|file| str::from_utf8(file.bytes).ok())
        .unwrap_or_default()
}

/// The chrome every page shares: the access script first, then the brand.
fn page(title: &str, body: &str) -> String {
    let favicon = STANDARD.encode(asset("favicon.svg"));
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<script src=\"/assets/local-access.js\"></script><title>{}</title><link rel=\"icon\" type=\"image/svg+xml\" href=\"data:image/svg+xml;base64,{favicon}\"><style>{CSS}{}</style><div class=\"wrap\"><header class=\"viewer-brand\"><a class=\"slink-brand slink-brand--compact\" href=\"/\" aria-label=\"session.link home\">{}<span class=\"slink-brand-name\">session<span class=\"slink-brand-dot\">.</span>link</span></a></header>{body}</div>",
        escape(title),
        asset("brand.css"),
        asset("mark.svg"),
    )
}

fn error_page(title: &str, headline: &str, detail: &str) -> String {
    page(
        title,
        &format!(
            "<p class=\"eyebrow\" style=\"margin:0 0 18px\"><a href=\"/\">← sessions</a> · local preview</p>\n     <h1 style=\"font-family:var(--serif);font-weight:500\">{headline}</h1>\n     {detail}"
        ),
    )
}

impl Viewer {
    /// The page for a saved preview, or `None` when there is no such file.
    fn preview_page(&self, id: &str) -> Option<Result<String, &'static str>> {
        let file = self.previews.join(format!("{id}.json"));
        let whole = fs::read_to_string(&file).ok()?;
        // The reading copy leaves out recorded tool output, which the reader
        // only shows once a step is opened; the whole document follows.
        let light = fs::read_to_string(self.previews.join(format!("{id}{READING_SUFFIX}")))
            .ok()
            .filter(|light| !light.is_empty());
        let full_url = light.is_some().then(|| format!("/api/document/{id}"));
        let text = light.unwrap_or(whole);
        let Ok(document) = slink_format::parse(&text) else {
            return Some(Err("the file on disk is not valid JSON"));
        };
        if !matches!(document["schema"].as_str(), Some("session/v0" | "run/v0")) {
            return Some(Err("the file on disk is not valid JSON"));
        }
        let name = document["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        let excerpt = !document["extensions"][EXCERPT].is_null();
        // Escaping `<` keeps a trace's own text inert inside the tag.
        let embedded = text.replace('<', "\\u003c");
        let publish = slink_format::encode(&json!({
            "hasKey": false,
            "file": display_path(&file),
            "endpoint": format!("/api/publish-preview/{id}"),
            "title": name,
        }));
        let full = full_url.map_or(String::new(), |url| {
            format!(
                "<script>window.__FULL__={}</script>",
                slink_format::encode(&Value::String(url))
            )
        });
        let (top, local) = if excerpt {
            (
                format!(
                    "<div class=\"top\">\n       <p class=\"eyebrow\" style=\"margin:0\"><a href=\"/\">← sessions</a></p>\n       <div class=\"pub\">\n         <button class=\"btn\" id=\"copy\" title=\"Copy this page's address on this computer. Publish to share with a colleague.\">Copy local URL</button>\n         <button class=\"btn\" id=\"dl\" title=\"save the run document as {id}.json\">Download excerpt</button>\n       </div>\n     </div>\n     <div class=\"result-row\"><span class=\"result\" id=\"out\"></span></div><p class=\"note\">Excerpt preview · only the selected material and the author note are included.</p>"
                ),
                String::new(),
            )
        } else {
            // The reading view owns its header: the way back, the title and
            // its one action.
            let config = slink_format::encode(&json!({
                "source": id,
                "project": self.project.to_string_lossy(),
                "title": "",
            }));
            (
                String::new(),
                format!("<script>window.__LOCAL__={config}</script>"),
            )
        };
        Some(Ok(page(
            name,
            &format!(
                "{top}<div id=\"root\"></div>\n     <script type=\"application/json\" id=\"run-data\">{embedded}</script><script>window.__RUN__=JSON.parse(document.getElementById(\"run-data\").textContent)</script>{full}{local}\n     <script>window.__PUB__={publish}</script>\n     <script src=\"/assets/viewer.js\"></script>\n     <script>\n       const copy=document.getElementById(\"copy\"),dl=document.getElementById(\"dl\");\n       if(copy)copy.onclick=async()=>{{\n         try{{await navigator.clipboard.writeText(location.href);copy.textContent=\"Copied\"}}\n         catch{{copy.textContent=\"Copy failed\"}}\n         setTimeout(()=>{{copy.textContent=\"Copy local URL\"}},1200);\n       }};\n       if(dl)dl.onclick=()=>{{\n         const u=URL.createObjectURL(new Blob([JSON.stringify(window.__RUN__)],{{type:\"application/json\"}}));\n         const a=document.createElement(\"a\");\n         a.href=u;a.download=\"{id}.json\";a.click();\n         setTimeout(()=>URL.revokeObjectURL(u),1000);\n       }};\n     </script>"
            ),
        )))
    }

    /// The sessions this run offers, each a button that saves a preview of
    /// it and goes there. The Go CLI's page groups and searches them; this
    /// one only lists them.
    fn index_page(&self) -> String {
        let name = self
            .project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rows: String = self
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                format!(
                    "<button class=\"card\" style=\"width:100%;text-align:left;cursor:pointer\" data-source=\"{index}\"><div class=\"t\">{}</div><div class=\"m\">{}</div></button>",
                    escape(&source.title),
                    escape(&source.detail)
                )
            })
            .collect();
        let list = if rows.is_empty() {
            "<p class=\"note\"><strong>No sessions for this project yet.</strong> Open a transcript with <code>slink view --session &lt;path&gt;</code>.</p>".to_owned()
        } else {
            rows
        };
        page(
            &format!("{name} · session.link"),
            &format!(
                "<div class=\"top\" style=\"margin-bottom:18px\"><h1 style=\"font-family:var(--serif);font-weight:500;margin:0\" title=\"{}\">{}</h1><button class=\"btn\" id=\"stop\">Stop viewer</button></div>\n    <p id=\"result\" class=\"note\" role=\"status\"></p>{list}\n    <script>\n      const result=document.getElementById('result');\n      document.querySelectorAll('[data-source]').forEach(row=>{{\n        row.onclick=async()=>{{\n          result.textContent='Opening…';\n          try{{const r=await fetch('/api/preview/'+row.dataset.source,{{method:'POST',headers:{{'x-slink':'1'}}}});const d=await r.json();if(!r.ok)throw new Error(d.error?.message||'Could not open this session.');location.assign(d.url);}}\n          catch(e){{result.textContent=e.message;}}\n        }};\n      }});\n      document.getElementById('stop').onclick=async()=>{{\n        try{{const r=await fetch('/api/stop',{{method:'POST',headers:{{'x-slink':'1'}}}});if(!r.ok)throw new Error();\n          document.querySelector('.wrap').innerHTML='<p class=\"note\"><strong>Viewer stopped.</strong> Your saved previews are still on disk. Run <code>slink view</code> to open it again.</p>';\n        }}catch{{result.textContent='Could not stop the viewer. Press Ctrl-C in the terminal that started it.';}}\n      }};\n    </script>",
                escape(&display_path(&self.project)),
                escape(&name),
            ),
        )
    }
}

fn send(status: StatusCode, kind: &'static str, body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    response
}

fn json_error(status: StatusCode, message: &str) -> Response {
    send(
        status,
        "application/json",
        slink_format::encode(&json!({"error": {"message": message}})),
    )
}

/// The same bytes, compared without stopping at the first that differs.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .fold(0, |differing, (x, y)| differing | (x ^ y))
            == 0
}

async fn handle(State(viewer): State<Arc<Viewer>>, request: Request) -> Response {
    let headers = request.headers();
    let header_text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    // A loopback bind alone does not stop a hostname that rebinds to it from
    // reading local sessions: the browser must use the host that was printed.
    let host = header_text("host");
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    if name != "127.0.0.1" && name != "localhost" {
        return send(
            StatusCode::FORBIDDEN,
            "text/plain; charset=utf-8",
            "use the local URL printed by slink view\n",
        );
    }
    let (get, post) = (
        request.method() == Method::GET,
        request.method() == Method::POST,
    );
    let path = request.uri().path();
    if get && path == "/assets/local-access.js" {
        return send(StatusCode::OK, "text/javascript", LOCAL_ACCESS_JS);
    }
    // The bundle is public code, with no session in it and no secret.
    if get && path == "/assets/viewer.js" {
        return send(
            StatusCode::OK,
            "text/javascript",
            viewer::file("viewer.js").map_or(&[][..], |file| file.bytes),
        );
    }
    if !same(
        header_text("x-slink-access").as_bytes(),
        viewer.access.as_bytes(),
    ) {
        // An ordinary navigation gets a public shell, which fetches the page
        // itself with the key from the fragment. Without the key, nothing
        // of a session or the list of them is ever sent.
        let page = path == "/" || path.starts_with("/p/");
        return if get && header_text("sec-fetch-mode") == "navigate" && page {
            send(StatusCode::OK, "text/html", ACCESS_SHELL)
        } else {
            send(
                StatusCode::FORBIDDEN,
                "text/plain; charset=utf-8",
                "local viewer access required\n",
            )
        };
    }
    // A custom header forces a preflight no page elsewhere gets answered,
    // and the origin must be this server.
    let origin = header_text("origin");
    let local_action =
        header_text("x-slink") == "1" && (origin.is_empty() || origin == format!("http://{host}"));
    let blocked = || json_error(StatusCode::FORBIDDEN, "cross-origin request blocked");
    let id_of = |prefix: &str| {
        path.strip_prefix(prefix).filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    };
    if path == "/" && get {
        return send(StatusCode::OK, "text/html", viewer.index_page());
    }
    if path == "/api/stop" && post {
        if !local_action {
            return blocked();
        }
        viewer.stop.notify_one();
        return send(StatusCode::OK, "application/json", "{\"ok\":true}");
    }
    if let Some(index) = id_of("/api/preview/").filter(|_| post) {
        if !local_action {
            return blocked();
        }
        let Some(source) = index
            .parse::<usize>()
            .ok()
            .and_then(|index| viewer.sources.get(index))
        else {
            return json_error(
                StatusCode::NOT_FOUND,
                "This session is not in the current list. Restart slink view to refresh it.",
            );
        };
        return match document(&source.file, &source.harness)
            .and_then(|text| save_preview(&viewer.previews, &text))
        {
            Ok(id) => send(
                StatusCode::OK,
                "application/json",
                slink_format::encode(&json!({"url": format!("/p/{id}")})),
            ),
            Err(error) => json_error(StatusCode::UNPROCESSABLE_ENTITY, &error),
        };
    }
    if let Some(id) = id_of("/api/document/").filter(|_| get) {
        return match fs::read(viewer.previews.join(format!("{id}.json"))) {
            Ok(raw) => send(StatusCode::OK, "application/json", raw),
            Err(_) => json_error(
                StatusCode::NOT_FOUND,
                "this session is no longer saved locally",
            ),
        };
    }
    if let Some(id) = id_of("/p/").filter(|_| get) {
        return match viewer.preview_page(id) {
            Some(Ok(body)) => send(StatusCode::OK, "text/html", body),
            Some(Err(why)) => send(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/html",
                error_page(
                    &format!("session {id} can't render"),
                    &format!(
                        "session <span class=\"hid\">{}</span> can't render",
                        escape(id)
                    ),
                    &format!(
                        "<p class=\"note\">{}</p>\n     <p class=\"note\"><a href=\"/\">back to the sessions</a></p>",
                        escape(why)
                    ),
                ),
            ),
            None => send(
                StatusCode::NOT_FOUND,
                "text/html",
                error_page(
                    &format!("no session {id} here"),
                    &format!("no session <span class=\"hid\">{}</span> here", escape(id)),
                    &format!(
                        "<p class=\"note\">nothing by that id in {}</p>\n     <p class=\"note\"><a href=\"/\">back to the sessions</a></p>",
                        escape(&display_path(&viewer.previews))
                    ),
                ),
            ),
        };
    }
    // Everything the Go viewer does through an account or the service.
    let hosted = [
        "/api/publish",
        "/api/login/",
        "/api/identity",
        "/api/named",
        "/api/compose/",
        "/api/title/",
        "/api/draft/",
        "/api/export/",
    ];
    if hosted.iter().any(|prefix| path.starts_with(prefix)) {
        return json_error(
            StatusCode::NOT_IMPLEMENTED,
            "slink-rs cannot publish or compose yet — nothing was uploaded. The Go slink does both.",
        );
    }
    send(
        StatusCode::NOT_FOUND,
        "text/html",
        error_page(
            "no such page",
            "no such page",
            "<p class=\"note\">everything here starts at <a href=\"/\">the sessions</a></p>",
        ),
    )
}
