//! The local viewer's server: one saved preview at a time, on loopback, to
//! a browser that holds this run's access key.
//!
//! A page is given only to a request that carries the key, which is made
//! for each run and travels in the URL's fragment, where no server log,
//! proxy or other local port sees it. Composing an excerpt stays on this
//! machine too. Publishing one is the only thing that leaves it, as
//! ciphertext, and only on a request from a page this server gave out.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value, json};
use slink_client::identity::Input;
use slink_client::{Identity, login, resolve_target};
use slink_local::share::{Catalog, Draft, build_catalog, export};
use slink_local::{
    Drafts, READING_SUFFIX, SaveError, capture_dir, display_path, home, save_preview,
    session_title_key, unix_now,
};
use slink_protocol::misspelt;
use tokio::sync::{Mutex, Notify};

use crate::picker::{self, Row, escape};
use crate::publish::inspect;
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
/// How often a running viewer goes on granting shares for named people.
const RESUME_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

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
    /// Where publishing goes, and the key it had when this viewer started.
    pub target: String,
    pub api_key: String,
    /// A sign-in started from a page, while the person confirms it.
    pub login: Mutex<Option<Login>>,
}

/// A sign-in a page started.
pub struct Login {
    id: String,
    state: &'static str,
    url: String,
    user_code: String,
    error: String,
    waiting: Option<tokio::task::AbortHandle>,
}

impl Login {
    fn to_json(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("id".to_owned(), json!(self.id));
        fields.insert("state".to_owned(), json!(self.state));
        for (name, value) in [
            ("url", &self.url),
            ("user_code", &self.user_code),
            ("error", &self.error),
        ] {
            if !value.is_empty() {
                fields.insert(name.to_owned(), json!(value));
            }
        }
        fields
    }
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
    /// The key to publish with: the one saved now when it is for this
    /// viewer's server, since a sign-in in a terminal or a page can save
    /// one while the viewer runs, else the one it started with.
    fn api_key(&self) -> String {
        let (target, key) = resolve_target(&home(), "", "");
        if target == self.target && !key.is_empty() {
            key
        } else {
            self.api_key.clone()
        }
    }

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
            "hasKey": !self.api_key().is_empty(),
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
                    "<div class=\"top\">\n       <p class=\"eyebrow\" style=\"margin:0\"><a href=\"/\">← sessions</a></p>\n       <div class=\"pub\">\n         {edit}\n         <button class=\"btn\" id=\"copy\" title=\"Copy this page's address on this computer. Publish to share with a colleague.\">Copy local URL</button>\n         <button class=\"btn\" id=\"dl\" title=\"save the run document as {id}.json\">Download excerpt</button>\n         <span class=\"note\" title=\"{}\">Encrypted before upload to {}</span>\n         <div id=\"publish-control\"></div>\n       </div>\n     </div>\n     <div class=\"result-row\"><span class=\"result\" id=\"out\"></span></div><p class=\"note\">Excerpt preview · only the selected material and the author note are included. Publishing encrypts this excerpt locally.</p>",
                    escape(&self.target),
                    escape(host(&self.target)),
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
                    .map(|view| {
                        let mut link =
                            json!({"title": view.title, "url": format!("/p/{}", view.id)});
                        if let Some(published) = self.drafts.published(&view.id) {
                            link["published"] = published;
                        }
                        link
                    })
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

/// The host a server's URL names, as the page says where it publishes.
fn host(target: &str) -> &str {
    target
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .unwrap_or(target)
}

/// The people a publish request names, when it names any. Anything but no
/// body, `null`, or an object whose `recipients` is absent, `null` or a list
/// of text is not a publish request. Nor is `Recipients` or another casing,
/// which Go reads as the people and an exact reading would publish as a
/// link for anyone.
fn named(body: &[u8]) -> Result<Option<Vec<String>>, ()> {
    let Some(first) = serde_json::Deserializer::from_slice(body)
        .into_iter::<Value>()
        .next()
    else {
        return Ok(None);
    };
    match first.map_err(drop)? {
        Value::Null => Ok(None),
        Value::Object(fields) if fields.keys().any(|key| misspelt(key, &["recipients"])) => Err(()),
        Value::Object(fields) => match fields.get("recipients") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(people)) => people
                .iter()
                .map(|person| person.as_str().map(str::to_owned))
                .collect::<Option<_>>()
                .map(Some)
                .ok_or(()),
            Some(_) => Err(()),
        },
        _ => Err(()),
    }
}

/// A page's answer for a failed action on keys: the message, and the
/// status Go's client gives it.
fn identity_error(error: &slink_client::identity::Error) -> Response {
    let mut failure = json!({"message": error.to_string()});
    // The settings page offers to discard the unfinished setup.
    if let Some(code) = error.code() {
        failure["code"] = json!(code);
    }
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
    send(
        status,
        "application/json",
        slink_format::encode(&json!({"error": failure})),
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

impl Viewer {
    /// Publishes a saved preview, or with `preview` false a capture, as an
    /// encrypted link, once it is a valid session with no credential in it.
    async fn publish(&self, id: &str, preview: bool, body: &[u8]) -> Response {
        let reply = |status: StatusCode, value: Value| {
            send(status, "application/json", slink_format::encode(&value))
        };
        let Ok(named) = named(body) else {
            return json_error(StatusCode::BAD_REQUEST, "invalid publish request");
        };
        let file = if preview {
            self.previews.join(format!("{id}.json"))
        } else {
            capture_dir().join(format!("{id}.json"))
        };
        let inspection = match inspect(&file) {
            Ok(inspection) => inspection,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
        };
        if !inspection.errors.is_empty() {
            return reply(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error": {"message": "not a valid session/v0 document", "details": inspection.errors}}),
            );
        }
        if !inspection.secrets.is_empty() {
            let whole = std::path::absolute(&file).unwrap_or_else(|_| file.clone());
            let hits: Vec<Value> = inspection
                .secrets
                .iter()
                .map(|hit| json!({"pattern": hit.pattern, "preview": hit.preview}))
                .collect();
            return reply(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error": {
                    "code": "secrets_detected",
                    "message": "publish blocked — credentials detected; redact the local file first",
                    "path": display_path(&whole),
                    "details": hits,
                }}),
            );
        }
        if let Some(people) = named {
            let identity = Identity::new(&home(), &self.target, &self.api_key());
            let shared = match identity.publish_named(&inspection.text, &people).await {
                Ok(shared) => shared,
                Err(error) => return identity_error(&error),
            };
            eprintln!("Published: {}", shared.url);
            if preview {
                let count = shared.recipients.len();
                let _ = self
                    .drafts
                    .record_published(id, &shared.url, count, now_ms());
            }
            let recipients: Vec<Value> = shared.recipients.iter().map(|r| r.to_json()).collect();
            return reply(
                StatusCode::OK,
                json!({"id": shared.id, "url": shared.url, "recipients": recipients, "warning": shared.error}),
            );
        }
        let upload =
            slink_client::upload(&home(), &inspection.text, &self.target, &self.api_key()).await;
        if !upload.ok {
            // No answer, or nothing sent, is a gateway's failure to the page.
            let status = StatusCode::from_u16(upload.status).unwrap_or(StatusCode::BAD_GATEWAY);
            return reply(status, Value::Object(upload.body));
        }
        let url = upload.body["url"].as_str().unwrap_or_default();
        eprintln!("Published: {url}");
        if preview {
            let _ = self.drafts.record_published(id, url, 0, now_ms());
        }
        reply(
            StatusCode::OK,
            json!({
                "url": url,
                "deduplicated": upload.body.get("deduplicated").cloned().unwrap_or(Value::Null),
                "backup": upload.body.get("backup").cloned().unwrap_or(Value::Null),
            }),
        )
    }
}

/// What a settings page sent: the first JSON value of the body, with the
/// action and its fields as text. A field that is missing or null is empty.
fn identity_input(body: &[u8]) -> Option<(String, Input)> {
    let value = serde_json::Deserializer::from_slice(body)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    if !value.is_object() && !value.is_null() {
        return None;
    }
    let text = |name: &str| match &value[name] {
        Value::Null => Some(String::new()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    };
    let input = Input {
        name: text("name")?,
        id: text("id")?,
        code: text("code")?,
        recovery_key: text("recovery_key")?,
    };
    Some((text("action")?, input))
}

impl Viewer {
    /// This machine's devices for the account the viewer publishes to.
    /// The keys stay here; the page sees the status, and the recovery key
    /// when it is asked to show it.
    async fn identity(&self, action: &str, input: &Input) -> Response {
        let identity = Identity::new(&home(), &self.target, &self.api_key());
        match identity.handle(action, input).await {
            Ok(status) => send(
                StatusCode::OK,
                "application/json",
                slink_format::encode(&status.to_json()),
            ),
            Err(error) => identity_error(&error),
        }
    }

    /// The shares for named people this machine is granting, for the
    /// "Shared with people" page, after revoking one person's access when
    /// the page asks to.
    async fn named(&self, body: &[u8]) -> Response {
        let input = serde_json::Deserializer::from_slice(body)
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok)
            .filter(|value| value.is_object() || value.is_null())
            .and_then(|value| {
                let text = |name: &str| match &value[name] {
                    Value::Null => Some(String::new()),
                    Value::String(text) => Some(text.clone()),
                    _ => None,
                };
                Some((text("action")?, text("id")?, text("invite_id")?))
            });
        let Some((action, id, invitation)) = input else {
            return json_error(StatusCode::BAD_REQUEST, "invalid request");
        };
        let revoke = match action.as_str() {
            "status" => "",
            // A revocation missing either id is not a listing.
            "revoke" if id.is_empty() || invitation.is_empty() => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "share and invitation ids are required",
                );
            }
            "revoke" => &id,
            _ => return json_error(StatusCode::BAD_REQUEST, "unknown sharing action"),
        };
        let identity = Identity::new(&home(), &self.target, &self.api_key());
        match identity.named_shares(revoke, &invitation, true).await {
            Ok(shares) => {
                let shares: Vec<Value> = shares.iter().map(|shared| shared.to_json()).collect();
                send(
                    StatusCode::OK,
                    "application/json",
                    slink_format::encode(&json!({"shares": shares})),
                )
            }
            Err(error) => identity_error(&error),
        }
    }
}

/// Goes on granting shares for named people while the viewer runs: a person
/// who accepts gets their key at the next pass. Only an invitation this
/// machine made, and a claim proven with its secret, releases a key.
pub async fn resume_named(viewer: Arc<Viewer>) {
    let mut every =
        tokio::time::interval_at(tokio::time::Instant::now() + RESUME_EVERY, RESUME_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let identity = Identity::new(&home(), &viewer.target, &viewer.api_key());
        let _ = identity.named_shares("", "", false).await;
    }
}

/// The sign-in a page can start, follow and cancel. The key the service
/// grants goes into the home's config and never to the page.
async fn login_api(viewer: Arc<Viewer>, action: &str, method: &Method) -> Response {
    let reply = |value: Map<String, Value>| {
        send(
            StatusCode::OK,
            "application/json",
            format!("{}\n", slink_format::encode(&Value::Object(value))),
        )
    };
    let with_account = |mut fields: Map<String, Value>| {
        fields.insert("signed_in".to_owned(), json!(!viewer.api_key().is_empty()));
        fields.insert("server".to_owned(), json!(viewer.target));
        fields
    };
    let mut current = viewer.login.lock().await;
    match (action, method) {
        ("status", &Method::GET) => match current.as_ref() {
            Some(login) => reply(with_account(login.to_json())),
            None => {
                let mut idle = Map::new();
                idle.insert("state".to_owned(), json!("idle"));
                reply(with_account(idle))
            }
        },
        ("cancel", &Method::POST) => {
            if let Some(waiting) = current.take().and_then(|login| login.waiting) {
                waiting.abort();
            }
            send(StatusCode::OK, "application/json", "{\"ok\":true}")
        }
        ("start", &Method::POST) => {
            if let Some(login) = current.as_ref().filter(|login| login.state == "pending") {
                return reply(with_account(login.to_json()));
            }
            let attempt = match login::begin(&viewer.target, "viewer").await {
                Ok(attempt) => attempt,
                Err(message) => {
                    return send(
                        StatusCode::BAD_GATEWAY,
                        "application/json",
                        format!(
                            "{}\n",
                            slink_format::encode(&json!({"error": {"message": message}}))
                        ),
                    );
                }
            };
            let mut token = [0; 24];
            if SystemRandom::new().fill(&mut token).is_err() {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "no randomness for the sign-in",
                );
            }
            let id: String = token.iter().map(|byte| format!("{byte:02x}")).collect();
            let (url, user_code) = (attempt.url.clone(), attempt.user_code.clone());
            let (watching, mine) = (viewer.clone(), id.clone());
            let waiting = tokio::spawn(async move {
                let done = login::wait(&home(), &attempt, std::time::Duration::from_secs(2)).await;
                let mut current = watching.login.lock().await;
                let Some(login) = current.as_mut().filter(|login| login.id == mine) else {
                    return;
                };
                login.url.clear();
                login.user_code.clear();
                match done {
                    Ok(_) => login.state = "complete",
                    Err(message) => {
                        login.state = "error";
                        login.error = message;
                    }
                }
            });
            let started = Login {
                id,
                state: "pending",
                url,
                user_code,
                error: String::new(),
                waiting: Some(waiting.abort_handle()),
            };
            let body = started.to_json();
            *current = Some(started);
            reply(body)
        }
        _ => send(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "404 page not found\n",
        ),
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
        let page = matches!(path, "/" | "/settings" | "/shared")
            || path.starts_with("/p/")
            || path.starts_with("/compose/");
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
    // The account pages are the bundle's; what they ask of this server is
    // below.
    if get && (path == "/settings" || path == "/shared") {
        let (title, flag) = if path == "/settings" {
            ("Recovery and devices", "__IDENTITY__")
        } else {
            ("Shared with people", "__NAMED_OUTBOX__")
        };
        return send(
            StatusCode::OK,
            "text/html",
            page(
                title,
                &format!(
                    "<a href=\"/\">← Sessions</a><div id=\"root\"></div><script>window.{flag}=true</script><script src=\"/assets/viewer.js\"></script>"
                ),
            ),
        );
    }
    for (prefix, preview) in [("/api/publish-preview/", true), ("/api/publish/", false)] {
        if let Some(id) = id_of(prefix).filter(|_| post) {
            if !local_action {
                return json_error(StatusCode::FORBIDDEN, "cross-origin publish blocked");
            }
            let Ok(body) = axum::body::to_bytes(body, 4096).await else {
                return json_error(StatusCode::BAD_REQUEST, "invalid publish request");
            };
            return viewer.publish(id, preview, &body).await;
        }
    }
    if let Some(action) = path.strip_prefix("/api/login/") {
        if !local_action {
            return blocked();
        }
        return login_api(viewer.clone(), action, method).await;
    }
    if path == "/api/identity" {
        if !local_action {
            return blocked();
        }
        if !post {
            return json_error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
        }
        let input = match axum::body::to_bytes(body, 4096).await {
            Ok(body) => identity_input(&body),
            Err(_) => None,
        };
        let Some((action, input)) = input else {
            return json_error(StatusCode::BAD_REQUEST, "invalid request");
        };
        return viewer.identity(&action, &input).await;
    }
    if path == "/api/named" {
        if !post || !local_action {
            return json_error(StatusCode::FORBIDDEN, "local action required");
        }
        let Ok(body) = axum::body::to_bytes(body, 4096).await else {
            return json_error(StatusCode::BAD_REQUEST, "invalid request");
        };
        return viewer.named(&body).await;
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
