//! The local viewer's server: one saved preview at a time, on loopback, to
//! a browser that holds this run's access key.
//!
//! A page is given only to a request that carries the key, which is made
//! for each run and travels in the URL's fragment, where no server log,
//! proxy or other local port sees it. Composing an excerpt stays on this
//! machine too; publishing is not in this binary yet, so nothing here
//! uploads.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use slink_local::share::{Catalog, Draft, build_catalog, export};
use slink_local::{
    Drafts, READING_SUFFIX, SaveError, display_path, save_preview, session_title_key, unix_now,
};
use tokio::sync::Notify;

use crate::picker::{self, Row, escape};
use crate::view::Source;
use crate::viewer;

const CSS: &str = include_str!("assets/page.css");
const ACCESS_SHELL: &str = include_str!("assets/access-shell.html");
/// The script that carries the access key from the fragment to each request.
/// It is the Go CLI's, read from where that keeps it.
const LOCAL_ACCESS_JS: &str = include_str!("../../../go/internal/open/local-access.js");
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'self'; base-uri 'none'; object-src 'none'; form-action 'none'; frame-ancestors 'none'";
/// The name an excerpt carries its selection under.
const EXCERPT: &str = "session_link.share.v1";

pub struct Viewer {
    pub access: String,
    pub previews: PathBuf,
    pub drafts: Drafts,
    pub project: PathBuf,
    pub sources: Vec<Source>,
    /// What could not be listed, for the sessions page to say.
    pub warning: String,
    pub stop: Notify,
    /// Held while a draft's revision is read and the next one written.
    pub saving: std::sync::Mutex<()>,
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
            // An excerpt made here can be opened again in the page that made it.
            let edit = self.drafts.edit_source(id).map_or(String::new(), |source| {
                let view = if self
                    .drafts
                    .view_record(id)
                    .is_some_and(|record| record.draft.is_some())
                {
                    format!("?view={id}")
                } else {
                    String::new()
                };
                format!("<a class=\"btn\" href=\"/compose/{source}{view}\">Edit selection</a>")
            });
            (
                format!(
                    "<div class=\"top\">\n       <p class=\"eyebrow\" style=\"margin:0\"><a href=\"/\">← sessions</a></p>\n       <div class=\"pub\">\n         {edit}\n         <button class=\"btn\" id=\"copy\" title=\"Copy this page's address on this computer. Publish to share with a colleague.\">Copy local URL</button>\n         <button class=\"btn\" id=\"dl\" title=\"save the run document as {id}.json\">Download excerpt</button>\n       </div>\n     </div>\n     <div class=\"result-row\"><span class=\"result\" id=\"out\"></span></div><p class=\"note\">Excerpt preview · only the selected material and the author note are included.</p>"
                ),
                String::new(),
            )
        } else {
            // The reading view owns its header: the way back, the title and
            // its one action.
            let config = slink_format::encode(&json!({
                "source": id,
                "project": self.project.to_string_lossy(),
                "title": self.drafts.title_for(id, &title_key(&document)),
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

    /// A saved preview and what it offers for an excerpt.
    fn compose_source(&self, id: &str) -> Option<(Value, Catalog)> {
        let text = fs::read_to_string(self.previews.join(format!("{id}.json"))).ok()?;
        let run = slink_format::parse(&text).ok()?;
        let catalog = build_catalog(&run);
        Some((run, catalog))
    }

    /// The title, draft and export endpoints the compose page uses. All of
    /// it stays on this machine: an export is another saved preview.
    fn compose(
        &self,
        action: &str,
        id: &str,
        method: &Method,
        local_action: bool,
        query: &str,
        body: &[u8],
    ) -> Response {
        let reply = |status: StatusCode, value: Value| {
            send(
                status,
                "application/json",
                format!("{}\n", slink_format::encode(&value)),
            )
        };
        let fail = |status: StatusCode, message: &str| {
            reply(status, json!({"error": {"message": message}}))
        };
        let read = matches!(action, "compose" | "title") && method == Method::GET;
        let write = (matches!(action, "draft" | "title") && method == Method::PUT)
            || (action == "export" && method == Method::POST);
        if !read && !write {
            return fail(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        if !read && !local_action {
            return fail(StatusCode::FORBIDDEN, "cross-origin request blocked");
        }
        let Some((run, catalog)) = self.compose_source(id) else {
            return fail(
                StatusCode::NOT_FOUND,
                "the saved source preview is unavailable; open the session again with slink view",
            );
        };
        let key = title_key(&run);
        // One JSON document, with no field this code does not know.
        let sent = || -> Result<Value, String> {
            serde_json::from_slice::<Value>(body)
                .map_err(|error| format!("cannot read draft: {error}"))
        };
        match action {
            "title" if read => reply(
                StatusCode::OK,
                json!({"title": self.drafts.title_for(id, &key)}),
            ),
            "title" => {
                let title = match sent() {
                    Ok(Value::Object(fields)) if fields.keys().all(|name| name == "title") => {
                        fields
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim()
                            .to_owned()
                    }
                    Ok(_) => {
                        return fail(
                            StatusCode::BAD_REQUEST,
                            "cannot read draft: expected a title",
                        );
                    }
                    Err(error) => return fail(StatusCode::BAD_REQUEST, &error),
                };
                if title.is_empty() || title.chars().count() > 256 || title.contains(['\n', '\r']) {
                    return fail(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "use a title of 1–256 characters on one line",
                    );
                }
                // Under the session's own name too, so later snapshots and
                // the sessions page use it.
                match self.drafts.set_title(id, &key, &title) {
                    Ok(()) => reply(StatusCode::OK, json!({"title": title})),
                    Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
                }
            }
            "compose" => {
                let (revision, mut draft) = match self.drafts.read_draft(id) {
                    Ok(saved) => saved,
                    Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &error),
                };
                let view = query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("view="))
                    .unwrap_or_default();
                if !view.is_empty() {
                    match self
                        .drafts
                        .view_record(view)
                        .filter(|record| record.source == id)
                        .and_then(|record| record.draft)
                    {
                        Some(saved) => draft = saved,
                        None => {
                            return fail(
                                StatusCode::NOT_FOUND,
                                "the saved view is unavailable for this session",
                            );
                        }
                    }
                }
                let typed = self.drafts.title_for(id, &key);
                let title = if typed.is_empty() {
                    run["name"].as_str().unwrap_or_default().to_owned()
                } else {
                    typed
                };
                let views: Vec<Value> = self
                    .drafts
                    .saved_views(id)
                    .into_iter()
                    .map(|view| json!({"title": view.title, "url": format!("/p/{}", view.id)}))
                    .collect();
                reply(
                    StatusCode::OK,
                    json!({"catalog": catalog.to_json(), "source_title": title, "revision": revision, "draft": draft.to_json(), "views": views}),
                )
            }
            "draft" => {
                let (revision, draft) = match sent().and_then(|value| {
                    let fields = value.as_object().filter(|fields| {
                        fields
                            .keys()
                            .all(|name| matches!(name.as_str(), "revision" | "draft"))
                    });
                    let fields =
                        fields.ok_or("cannot read draft: expected a revision and a draft")?;
                    let revision = match fields.get("revision") {
                        None | Some(Value::Null) => 0,
                        Some(number) => number
                            .as_i64()
                            .ok_or("cannot read draft: revision must be a whole number")?,
                    };
                    let draft = match fields.get("draft") {
                        None | Some(Value::Null) => Draft::default(),
                        Some(draft) => Draft::from_json(draft)
                            .map_err(|error| format!("cannot read draft: {error}"))?,
                    };
                    Ok((revision, draft))
                }) {
                    Ok(next) => next,
                    Err(error) => return fail(StatusCode::BAD_REQUEST, &error),
                };
                if let Err(error) = draft.validate(&catalog, false) {
                    return fail(StatusCode::UNPROCESSABLE_ENTITY, &error);
                }
                let _saving = self
                    .saving
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match self.drafts.read_draft(id) {
                    Ok((current, _)) if current != revision => {
                        return fail(
                            StatusCode::CONFLICT,
                            "this draft changed in another tab; your edits are still here, but reload before saving further changes",
                        );
                    }
                    Ok(_) => {}
                    Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &error),
                }
                match self.drafts.write_draft(id, revision + 1, &draft) {
                    Ok(()) => reply(StatusCode::OK, json!({"revision": revision + 1})),
                    Err(SaveError::Taken) => fail(
                        StatusCode::CONFLICT,
                        "this draft changed in another viewer; reload before saving further changes",
                    ),
                    Err(SaveError::Failed(error)) => {
                        fail(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                    }
                }
            }
            _ => {
                let draft = match sent().and_then(|value| {
                    Draft::from_json(&value).map_err(|error| format!("cannot read draft: {error}"))
                }) {
                    Ok(draft) => draft,
                    Err(error) => return fail(StatusCode::BAD_REQUEST, &error),
                };
                let document = match export(&run, &catalog, &draft) {
                    Ok(document) => document,
                    Err(error) => return fail(StatusCode::UNPROCESSABLE_ENTITY, &error),
                };
                let exported = match save_preview(&self.previews, &slink_format::encode(&document))
                {
                    Ok(exported) => exported,
                    Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &error),
                };
                // How it was edited stays here, wholly outside the excerpt.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|since| i64::try_from(since.as_nanos()).ok())
                    .unwrap_or_else(|| unix_now() * 1_000_000_000);
                if let Err(error) = self.drafts.record_view(&exported, id, &draft, now) {
                    return fail(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
                }
                reply(
                    StatusCode::OK,
                    json!({"url": format!("/p/{exported}"), "document": document}),
                )
            }
        }
    }

    /// The sessions this run offers.
    fn index_page(&self) -> String {
        let name = self
            .project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rows: Vec<Row> = self
            .sources
            .iter()
            .map(|source| source.row.clone())
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_nanos()).ok())
            .unwrap_or(0);
        page(
            &format!("{name} · session.link"),
            &picker::body(&self.project, &rows, &self.warning, &self.drafts, now),
        )
    }
}

/// The name a title typed for this document's session is kept under.
fn title_key(document: &Value) -> String {
    session_title_key(
        document["source"]["harness"].as_str().unwrap_or_default(),
        document["metadata"]["session_id"]
            .as_str()
            .unwrap_or_default(),
    )
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

pub async fn handle(State(viewer): State<Arc<Viewer>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;
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
    let method = &parts.method;
    let (get, post) = (method == Method::GET, method == Method::POST);
    let path = parts.uri.path();
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
        let page = path == "/" || path.starts_with("/p/") || path.starts_with("/compose/");
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
    for action in ["compose", "title", "draft", "export"] {
        if let Some(id) = id_of(&format!("/api/{action}/")) {
            // A draft is small; anything larger is not one.
            let Ok(body) = axum::body::to_bytes(body, 2 * 1024 * 1024).await else {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "cannot read draft: the request is too large",
                );
            };
            return viewer.compose(
                action,
                id,
                method,
                local_action,
                parts.uri.query().unwrap_or_default(),
                &body,
            );
        }
    }
    if let Some(id) = id_of("/compose/").filter(|_| get) {
        return match viewer.compose_source(id) {
            Some(_) => send(
                StatusCode::OK,
                "text/html",
                page(
                    "Prepare an excerpt · session.link",
                    &format!(
                        "<div id=\"root\"></div><script>window.__COMPOSE__={}</script><script src=\"/assets/viewer.js\"></script>",
                        slink_format::encode(&json!({"source": id}))
                    ),
                ),
            ),
            None => send(
                StatusCode::NOT_FOUND,
                "text/html",
                error_page(
                    &format!("no session {id} here"),
                    &format!("no session <span class=\"hid\">{}</span> here", escape(id)),
                    "<p class=\"note\"><a href=\"/\">back to the sessions</a></p>",
                ),
            ),
        };
    }
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
        return match source.preview(&viewer.previews) {
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
    // Whether anyone is signed in is a question this binary can answer
    // truthfully: no one is, because it cannot sign anyone in yet.
    if path == "/api/login/status" && get {
        if !local_action {
            return blocked();
        }
        let server = std::env::var("SLINK_SERVER")
            .ok()
            .filter(|server| !server.is_empty());
        return send(
            StatusCode::OK,
            "application/json",
            slink_format::encode(&json!({
                "state": "idle",
                "signed_in": false,
                "server": server.as_deref().unwrap_or("https://session.link"),
            })),
        );
    }
    // Everything the Go viewer does through an account or the service.
    let hosted = ["/api/publish", "/api/login/", "/api/identity", "/api/named"];
    if hosted.iter().any(|prefix| path.starts_with(prefix)) {
        return json_error(
            StatusCode::NOT_IMPLEMENTED,
            "slink-rs cannot publish yet — nothing was uploaded. The Go slink publishes.",
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
