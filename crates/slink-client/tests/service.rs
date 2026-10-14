//! The client against a stand-in for the hosted service, on loopback: what
//! it sends, what it keeps, and what it does with answers that are lost,
//! wrong or pointed elsewhere. Go's tests of the same, in
//! go/internal/cli/encrypted_test.go and cli_test.go, are here as well.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slink_client::{delete, login, upload};
use slink_protocol::envelope;

const PLAIN: &str = r#"{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","name":"PRIVATE_TITLE_SENTINEL","spans":[{"id":"root","type":"agent","raw":{"unknown":"retained"}}]}"#;
const VALID: &str = r#"{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","spans":[{"id":"root","type":"agent"}]}"#;

/// What the stand-in was sent.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

type Answer = Box<dyn Fn(&Seen, usize) -> Response + Send + Sync>;

/// A service on loopback that records each request and answers with
/// `answer`, which is told how many came before.
async fn service(answer: Answer) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (record, answer) = (seen.clone(), Arc::new(answer));
    let app = axum::Router::new().fallback(move |request: Request| {
        let (record, answer) = (record.clone(), answer.clone());
        async move {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, 64 * 1024 * 1024).await.unwrap().to_vec();
            let request = Seen {
                method: parts.method.to_string(),
                path: parts.uri.to_string(),
                headers: parts
                    .headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.to_string(),
                            value.to_str().unwrap_or_default().to_owned(),
                        )
                    })
                    .collect(),
                body,
            };
            let count = {
                let mut all = record.lock().unwrap();
                all.push(request.clone());
                all.len() - 1
            };
            answer(&request, count)
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}"), seen)
}

fn reply(status: u16, body: impl Into<Body>) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(body.into())
        .unwrap()
}

fn header<'a>(seen: &'a Seen, name: &str) -> &'a str {
    seen.headers
        .iter()
        .find(|(key, _)| key == name)
        .map_or("", |(_, value)| value)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slink-client-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// An acknowledgement of the envelope in `seen`.
fn acknowledge(seen: &Seen, status: u16, extra: Value) -> Response {
    let mut body = json!({"id": "23456789abcdef", "sha256": sha256(&seen.body), "url": "https://untrusted.example/s/23456789abcdef"});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    reply(status, body.to_string())
}

#[tokio::test]
async fn a_lost_acknowledgement_is_recovered_with_the_same_ciphertext() {
    let home = home("lost");
    let pending_dir = home.join("pending-shares");
    let pending = pending_dir.clone();
    let (server, seen) = service(Box::new(move |request, count| {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/shares");
        assert_eq!(header(request, "authorization"), "Bearer rk_test");
        assert_eq!(
            header(request, "content-type"),
            "application/vnd.session-link.encrypted"
        );
        assert!(
            envelope::validate(&request.body).is_ok(),
            "upload is not an envelope"
        );
        assert!(
            !request
                .body
                .windows(22)
                .any(|w| w == b"PRIVATE_TITLE_SENTINEL")
        );
        // The key is recoverable before the service has a byte.
        assert_eq!(files(&pending).len(), 1, "pending receipts");
        if count == 0 {
            return reply(503, "");
        }
        acknowledge(request, 201, json!({"account_id": "acct_1"}))
    }))
    .await;

    let first = upload(&home, PLAIN, &server, "rk_test").await;
    assert!(!first.ok, "an unacknowledged upload succeeded: {first:?}");
    assert_eq!(first.status, 503);
    assert_eq!(first.body["error"]["code"], "upload_failed");
    // The same text to the same server, written with a trailing slash.
    let second = upload(&home, PLAIN, &format!("{server}/"), "rk_test").await;
    assert!(second.ok, "{second:?}");
    assert_eq!(second.status, 201);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].body, seen[1].body,
        "the retry sent other ciphertext"
    );

    // The link is on the configured server, whatever the answer said.
    let url = second.body["url"].as_str().unwrap();
    let key = url
        .strip_prefix(&format!("{server}/s/23456789abcdef#key="))
        .expect("the link is on the configured server");
    assert_eq!(
        envelope::decrypt(&seen[1].body, key).unwrap(),
        PLAIN.as_bytes()
    );
    assert_eq!(second.body["backup"], "setup");
    for request in &seen {
        assert!(
            !request.path.contains(key)
                && !request.headers.iter().any(|(_, value)| value.contains(key))
        );
    }

    assert!(files(&pending_dir).is_empty(), "the pending receipt stayed");
    let receipts = files(&home.join("shares"));
    assert_eq!(receipts.len(), 1);
    let name = receipts[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        name,
        format!("{}-23456789abcdef.json", sha256(&seen[1].body))
    );
    let receipt: Value = serde_json::from_str(&fs::read_to_string(&receipts[0]).unwrap()).unwrap();
    assert_eq!(receipt["account"], "acct_1");
    assert_eq!(receipt["server"], server);
    assert_eq!(receipt["url"], url);
    assert!(receipt.get("envelope").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&receipts[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn unsafe_input_never_leaves_and_an_old_server_gets_no_plaintext() {
    let home = home("unsafe");
    let (server, seen) = service(Box::new(|request, _| {
        assert_eq!(request.path, "/api/shares", "plaintext fallback");
        reply(404, "")
    }))
    .await;
    let secret = r#"{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","spans":[{"id":"r","type":"agent","raw":{"text":"sk-abcdefghijklmnop1234"}}]}"#;
    for (text, code) in [
        ("{}", "invalid_session"),
        ("not json", "invalid_session"),
        (secret, "secrets_detected"),
    ] {
        let refused = upload(&home, text, &server, "rk_test").await;
        assert_eq!((refused.ok, refused.status), (false, 422));
        assert_eq!(refused.body["error"]["code"], code);
    }
    let anonymous = upload(&home, VALID, &server, "").await;
    assert_eq!(
        (anonymous.status, anonymous.body["error"]["code"].as_str()),
        (401, Some("auth_required"))
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "unsafe input left the client"
    );
    assert!(
        !home.join("pending-shares").exists(),
        "a key was made for what was not sent"
    );

    let old = upload(&home, VALID, &server, "rk_test").await;
    assert_eq!((old.ok, old.status), (false, 404));
    assert_eq!(seen.lock().unwrap().len(), 1);
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn a_key_goes_only_where_it_may_and_no_redirect_is_followed() {
    let home = home("origin");
    for (server, message) in [
        (
            "http://session.link",
            "use HTTPS for the server (HTTP is allowed only on localhost)",
        ),
        ("https://user@session.link", "invalid server URL"),
        ("https://session.link?x", "invalid server URL"),
        ("session.link", "invalid server URL"),
    ] {
        let refused = upload(&home, VALID, server, "rk_test").await;
        assert_eq!(
            (refused.status, refused.body["error"]["message"].as_str()),
            (0, Some(message)),
            "{server}"
        );
    }
    let (elsewhere, followed) = service(Box::new(|_, _| reply(500, ""))).await;
    let target = format!("{elsewhere}/api/shares");
    let (server, _) = service(Box::new(move |_, _| {
        Response::builder()
            .status(307)
            .header("location", target.as_str())
            .body(Body::empty())
            .unwrap()
    }))
    .await;
    let redirected = upload(&home, VALID, &server, "rk_test").await;
    assert_eq!((redirected.ok, redirected.status), (false, 307));
    assert_eq!(
        delete::share(&server, "rk_test", "23456789abcdef").await.0,
        307
    );
    assert!(
        followed.lock().unwrap().is_empty(),
        "a redirect was followed"
    );
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn an_answer_that_does_not_acknowledge_this_envelope_is_not_a_link() {
    let home = home("wrong");
    let (server, _) = service(Box::new(|request, count| match count {
        0 => reply(
            201,
            json!({"id": "23456789abcdef", "sha256": "00"}).to_string(),
        ),
        1 => reply(
            201,
            json!({"id": "../../x", "sha256": sha256(&request.body)}).to_string(),
        ),
        2 => reply(201, "[1]"),
        3 => reply(
            502,
            "{\"error\":{\"code\":\"bad\",\"message\":\"upstream\"}} trailing",
        ),
        _ => acknowledge(request, 200, json!({"deduplicated": true})),
    }))
    .await;
    for _ in 0..3 {
        let wrong = upload(&home, VALID, &server, "rk_test").await;
        assert_eq!(
            (wrong.ok, wrong.body["error"]["code"].as_str()),
            (false, Some("invalid_response"))
        );
    }
    let failed = upload(&home, VALID, &server, "rk_test").await;
    assert_eq!(
        (failed.status, failed.body["error"]["message"].as_str()),
        (502, Some("upstream"))
    );
    // Every attempt was the same envelope, and the last one is the link.
    let done = upload(&home, VALID, &server, "rk_test").await;
    assert!(
        done.ok && done.status == 200 && done.body["deduplicated"] == true,
        "{done:?}"
    );
    assert_eq!(files(&home.join("shares")).len(), 1);
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn a_pending_receipt_that_does_not_open_to_this_session_is_not_sent() {
    let home = home("tampered");
    let (server, seen) = service(Box::new(|_, _| reply(503, ""))).await;
    assert!(!upload(&home, VALID, &server, "rk_test").await.ok);
    let pending = files(&home.join("pending-shares"));
    assert_eq!(pending.len(), 1);
    let mut receipt: Value =
        serde_json::from_str(&fs::read_to_string(&pending[0]).unwrap()).unwrap();
    receipt["key"] = json!("A".repeat(43));
    fs::write(&pending[0], receipt.to_string()).unwrap();
    let refused = upload(&home, VALID, &server, "rk_test").await;
    assert_eq!(
        refused.body["error"]["message"],
        "pending encryption receipt failed verification"
    );
    fs::write(&pending[0], "{").unwrap();
    let unreadable = upload(&home, VALID, &server, "rk_test").await;
    assert_eq!(
        unreadable.body["error"]["message"],
        "cannot read the pending encryption receipt"
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn sign_in_waits_for_the_grant_and_saves_it_for_the_configured_server() {
    let home = home("login");
    let code = "A".repeat(43);
    let poll = format!("/api/auth/cli/{code}");
    let (server, seen) = service(Box::new(move |request, count| {
        if request.method == "POST" && request.path == "/api/auth/cli" {
            return reply(
                200,
                json!({"code": "A".repeat(43), "user_code": "AB-12", "url": "https://x/cli/c1"})
                    .to_string(),
            );
        }
        assert_eq!(request.path, poll);
        if count < 2 {
            return reply(202, "");
        }
        reply(
            200,
            json!({"key": "rk_granted", "login": "tester", "uid": "u1"}).to_string(),
        )
    }))
    .await;
    let attempt = login::begin(&format!("{server}/"), "viewer").await.unwrap();
    assert_eq!(attempt.user_code, "AB-12");
    assert_eq!(attempt.url, format!("{server}/cli/{code}"));
    assert!(
        !format!("{attempt:?}").contains(&code),
        "the grant code is printed"
    );
    let first = seen.lock().unwrap()[0].clone();
    assert_eq!(first.body, br#"{"source":"viewer"}"#);
    assert_eq!(header(&first, "content-type"), "application/json");

    let done = login::wait(&home, &attempt, Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(done.login, "tester");
    let saved = slink_client::config::read(&home);
    assert_eq!(
        (
            saved.api_key.as_str(),
            saved.server.as_str(),
            saved.user_id.as_str()
        ),
        ("rk_granted", server.as_str(), "u1")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&done.config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn sign_in_says_why_it_stopped() {
    let home = home("login-failed");
    let (server, _) = service(Box::new(|request, _| match request.path.as_str() {
        "/api/auth/cli" => reply(
            200,
            json!({"code": "A".repeat(43), "user_code": "X"}).to_string(),
        ),
        _ => reply(404, ""),
    }))
    .await;
    let attempt = login::begin(&server, "terminal").await.unwrap();
    let expired = login::wait(&home, &attempt, Duration::from_millis(1)).await;
    assert_eq!(expired.unwrap_err(), "sign-in expired; try again");
    assert!(!home.join("config.json").exists());

    let (refusing, _) = service(Box::new(|_, _| reply(503, "<html>"))).await;
    assert_eq!(
        login::begin(&refusing, "terminal").await.unwrap_err(),
        "cannot start sign-in (HTTP 503); check that email or GitHub login is configured"
    );
    let (odd, _) = service(Box::new(|_, _| {
        reply(
            200,
            json!({"code": format!("{}B", "A".repeat(42)), "user_code": "X"}).to_string(),
        )
    }))
    .await;
    assert_eq!(
        login::begin(&odd, "terminal").await.unwrap_err(),
        "invalid sign-in response"
    );
    assert_eq!(
        login::begin("http://session.link", "terminal")
            .await
            .unwrap_err(),
        "sign-in requires an HTTPS server (or localhost)"
    );
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn a_pasted_key_is_saved_and_clears_who_signed_in() {
    let home = home("paste");
    slink_client::config::write(
        &home,
        &slink_client::Config {
            user_id: "u1".to_owned(),
            api_key: "rk_old".to_owned(),
            server: "https://a.test".to_owned(),
            login: "tester".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(
        login::with_key(&home, "nope", "").unwrap_err(),
        "that doesn't look like an API key (rk_…)"
    );
    login::with_key(&home, "rk_new", "").unwrap();
    let saved = slink_client::config::read(&home);
    assert_eq!(
        saved,
        slink_client::Config {
            api_key: "rk_new".to_owned(),
            server: "https://a.test".to_owned(),
            ..Default::default()
        }
    );
    login::with_key(&home, "rk_new", "https://b.test").unwrap();
    assert_eq!(slink_client::config::read(&home).server, "https://b.test");
    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn delete_sends_the_key_and_reads_the_answer() {
    let (server, seen) = service(Box::new(|request, _| match request.path.as_str() {
        "/api/shares/23456789abcdef" => reply(204, ""),
        "/api/runs/abc" => reply(403, r#"{"error":{"message":"not yours"}}"#),
        _ => reply(404, ""),
    }))
    .await;
    assert_eq!(
        delete::share(&server, "rk_test", "23456789abcdef").await,
        (204, "No Content".to_owned())
    );
    assert_eq!(
        delete::run(&server, "rk_test", "abc").await,
        (403, "not yours".to_owned())
    );
    assert_eq!(
        delete::share(&server, "rk_test", "../x").await,
        (0, "invalid share id".to_owned())
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|request| request.method == "DELETE"
        && header(request, "authorization") == "Bearer rk_test"));
    let closed = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    assert_eq!(
        delete::share(&format!("http://{address}"), "rk_test", "23456789abcdef").await,
        (0, "connection refused".to_owned())
    );
    assert_eq!(
        delete::run("http://session.link", "rk_test", "abc").await.0,
        0
    );
}
