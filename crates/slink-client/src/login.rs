//! Signing in: an API key pasted, or one the service grants once a person
//! confirms a code in a browser. Either way the key is saved in the home's
//! `config.json` and nowhere else.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use slink_protocol::misspelt;

use crate::config::{self, Config};
use crate::origin::origin;
use crate::transport::{client, object};

/// How a sign-in ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedIn {
    pub config: PathBuf,
    /// The handle the account signed in with; empty for a pasted key.
    pub login: String,
    /// The server the saved sign-in was for before this one.
    pub previous_server: String,
}

/// Saves a key someone pasted: from CI, or minted by an operator. It is for
/// `server` when that is given, else for the server already saved.
pub fn with_key(home: &Path, key: &str, server: &str) -> Result<LoggedIn, String> {
    if !key.starts_with("rk_") {
        return Err("that doesn't look like an API key (rk_…)".to_owned());
    }
    let mut saved = config::read(home);
    saved.api_key = key.to_owned();
    saved.login.clear();
    saved.user_id.clear();
    if !server.is_empty() {
        saved.server = server.to_owned();
    }
    let file = config::write(home, &saved).map_err(|error| error.to_string())?;
    Ok(LoggedIn {
        config: file,
        login: String::new(),
        previous_server: String::new(),
    })
}

/// A sign-in under way. The code is a grant for a few minutes, so it is
/// never shown: `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq)]
pub struct Attempt {
    code: String,
    /// What the person compares in the browser.
    pub user_code: String,
    /// Where the person confirms it, on the configured server.
    pub url: String,
    server: String,
}

impl fmt::Debug for Attempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attempt")
            .field("user_code", &self.user_code)
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

/// Whether `code` is 32 bytes as base64url writes them, and only so.
fn grant_code(code: &str) -> bool {
    code.len() == 43
        && slink_protocol::encoding::decode_exact::<32>(code)
            .is_ok_and(|bytes| slink_protocol::encoding::encode(&bytes) == code)
}

/// Asks `server` for a sign-in, naming what asked: `terminal` or `viewer`.
pub async fn begin(server: &str, source: &str) -> Result<Attempt, String> {
    let server =
        origin(server).map_err(|_| "sign-in requires an HTTPS server (or localhost)".to_owned())?;
    let http = client(Duration::from_secs(30)).map_err(|error| error.to_string())?;
    let response = http
        .post(format!("{server}/api/auth/cli"))
        .header("content-type", "application/json")
        .body(slink_format::encode(&json!({"source": source})))
        .send()
        .await
        .map_err(|_| "cannot reach the sign-in server".to_owned())?;
    let status = response.status().as_u16();
    let answer = object(response, 8192).await.unwrap_or_default();
    let text = |name: &str| answer.get(name).and_then(Value::as_str).unwrap_or_default();
    let (code, user_code) = (text("code"), text("user_code"));
    if status != 200 || code.len() != 43 || user_code.is_empty() {
        return Err(format!(
            "cannot start sign-in (HTTP {status}); check that email or GitHub login is configured"
        ));
    }
    if !grant_code(code) {
        return Err("invalid sign-in response".to_owned());
    }
    // The page is on the configured server: an answer's own URL is not
    // followed.
    Ok(Attempt {
        url: format!("{server}/cli/{code}"),
        code: code.to_owned(),
        user_code: user_code.to_owned(),
        server,
    })
}

/// Waits for the person to confirm `attempt`, asking every `interval` for at
/// most ten minutes, and saves the key the service grants.
pub async fn wait(home: &Path, attempt: &Attempt, interval: Duration) -> Result<LoggedIn, String> {
    let expired = || "sign-in expired or was cancelled; try again".to_owned();
    tokio::time::timeout(Duration::from_secs(10 * 60), poll(home, attempt, interval))
        .await
        .unwrap_or_else(|_| Err(expired()))
}

async fn poll(home: &Path, attempt: &Attempt, interval: Duration) -> Result<LoggedIn, String> {
    let http = client(Duration::from_secs(30)).map_err(|error| error.to_string())?;
    loop {
        tokio::time::sleep(interval).await;
        let Ok(response) = http
            .get(format!("{}/api/auth/cli/{}", attempt.server, attempt.code))
            .send()
            .await
        else {
            continue;
        };
        let status = response.status().as_u16();
        let grant = object(response, 8192).await;
        match status {
            202 => continue,
            404 => return Err("sign-in expired; try again".to_owned()),
            _ => {}
        }
        // Go reads each field as text, with any casing of its name; a grant
        // it cannot read that way is refused.
        let fields = ["key", "login", "uid"];
        let grant = grant.filter(|grant| {
            grant.iter().all(|(name, value)| {
                !misspelt(name, &fields)
                    && (!fields.contains(&name.as_str()) || value.is_string() || value.is_null())
            })
        });
        let text = |name: &str| {
            grant
                .as_ref()
                .and_then(|grant| grant.get(name))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let key = text("key");
        if grant.is_none() || status != 200 || !key.starts_with("rk_") {
            return Err(format!("sign-in failed (HTTP {status})"));
        }
        let mut saved = config::read(home);
        let previous_server = saved.server.clone();
        saved = Config {
            api_key: key,
            server: attempt.server.clone(),
            login: text("login"),
            user_id: text("uid"),
        };
        let file = config::write(home, &saved).map_err(|error| error.to_string())?;
        return Ok(LoggedIn {
            config: file,
            login: saved.login,
            previous_server,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::grant_code;

    #[test]
    fn a_grant_code_is_canonical_base64url() {
        assert!(grant_code(&"A".repeat(43)));
        // The last character carries two bits that must be zero.
        assert!(!grant_code(&format!("{}B", "A".repeat(42))));
        assert!(!grant_code(&"A".repeat(42)));
        assert!(!grant_code(&format!("{}=", "A".repeat(42))));
    }
}
