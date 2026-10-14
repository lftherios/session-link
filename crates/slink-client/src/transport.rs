//! The HTTP client every request to the service is made with, and how its
//! answers and failures are read.

use std::io;
use std::time::Duration;

use reqwest::{Client, Response, redirect};
use serde_json::{Map, Value};

/// A client that waits at most `timeout` for a whole exchange and never
/// follows a redirect: a key sent to one server is not sent on to another,
/// and an answer that points elsewhere is read as the answer.
pub fn client(timeout: Duration) -> reqwest::Result<Client> {
    // An error means a provider is already installed, which is the goal.
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder()
        .timeout(timeout)
        .redirect(redirect::Policy::none())
        .build()
}

/// The first JSON value of at most `limit` bytes of the answer, when it is an
/// object. More bytes than that are not read, and a value cut off by the
/// limit is not one.
pub async fn object(mut response: Response, limit: usize) -> Option<Map<String, Value>> {
    let mut body = Vec::new();
    while body.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    body.truncate(limit);
    match serde_json::Deserializer::from_slice(&body)
        .into_iter::<Value>()
        .next()
    {
        Some(Ok(Value::Object(object))) => Some(object),
        _ => None,
    }
}

/// Why a request got no answer, in the words Go's client uses for the
/// common causes, else in the words of the deepest error.
pub fn cause(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "i/o timeout".to_owned();
    }
    let mut deepest: &dyn std::error::Error = error;
    let mut dns = false;
    while let Some(source) = deepest.source() {
        dns |= source.to_string().contains("dns error");
        deepest = source;
    }
    if dns {
        return "no such host".to_owned();
    }
    if let Some(io) = deepest.downcast_ref::<io::Error>() {
        match io.kind() {
            io::ErrorKind::ConnectionRefused => return "connection refused".to_owned(),
            io::ErrorKind::ConnectionReset => return "connection reset by peer".to_owned(),
            io::ErrorKind::TimedOut => return "i/o timeout".to_owned(),
            _ => {}
        }
    }
    deepest.to_string()
}
