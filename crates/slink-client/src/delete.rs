//! Taking a published session offline. Only the account that published it
//! can, and the service answers 410 for it from then on.

use std::time::Duration;

use reqwest::StatusCode;

use crate::origin::origin;
use crate::publish::share_id;
use crate::transport::{cause, client, object};

/// What the service answered: its status and a few words, or 0 and why no
/// request was answered.
pub type Deleted = (u16, String);

/// Deletes an encrypted link share, `/s/<id>`. The link's fragment, with its
/// key, is never part of the request.
pub async fn share(server: &str, api_key: &str, id: &str) -> Deleted {
    if !share_id(id) {
        return (0, "invalid share id".to_owned());
    }
    match send(server, api_key, &format!("/api/shares/{id}")).await {
        Ok((status, _)) => (
            status.as_u16(),
            status.canonical_reason().unwrap_or_default().to_owned(),
        ),
        Err(why) => (0, why),
    }
}

/// Deletes a plaintext session published before links were encrypted,
/// `/r/<id>`, and says the service's own message for a failure.
pub async fn run(server: &str, api_key: &str, id: &str) -> Deleted {
    match send(server, api_key, &format!("/api/runs/{id}")).await {
        Ok((status, message)) => (status.as_u16(), message),
        Err(why) => (0, why),
    }
}

/// Sends the DELETE: to a server a key may go to, and never on to another.
async fn send(server: &str, api_key: &str, path: &str) -> Result<(StatusCode, String), String> {
    let server = origin(server)
        .map_err(|_| "use HTTPS for the server (HTTP is allowed only on localhost)".to_owned())?;
    let http = client(Duration::from_secs(30)).map_err(|error| error.to_string())?;
    let response = http
        .delete(format!("{server}{path}"))
        .header("authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|error| cause(&error))?;
    let status = response.status();
    let message = object(response, 64 * 1024)
        .await
        .and_then(|body| {
            body.get("error")?
                .get("message")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Ok((status, message))
}
