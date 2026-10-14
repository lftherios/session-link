//! Publishing a session as a link: checked, encrypted here, and uploaded as
//! ciphertext. The content key stays on this machine and in the link's
//! fragment; no request carries it, and nothing is ever sent in the clear,
//! whatever the server says.
//!
//! The key is saved before any byte is sent. An upload whose answer is lost
//! is sent again, as the same ciphertext, when the same session is published
//! to the same server, and the service answers with the same link.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use slink_local::write_private;
use slink_protocol::envelope::{self, MAX_PLAINTEXT};

use crate::identity::Identity;
use crate::origin::{Refused, origin};
use crate::transport::{client, object};

/// What an upload came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub ok: bool,
    /// The service's status, or 0 when nothing answered or nothing was sent.
    pub status: u16,
    /// The service's answer, with `url` and `backup` added on success; on
    /// failure, an `error` with a `code` and a `message`.
    pub body: Map<String, Value>,
}

fn error(status: u16, code: &str, message: &str) -> Upload {
    let Value::Object(body) = json!({"error": {"code": code, "message": message}}) else {
        unreachable!("an object is written");
    };
    Upload {
        ok: false,
        status,
        body,
    }
}

/// The id of a link share: 14 characters of the service's alphabet.
pub fn share_id(id: &str) -> bool {
    id.len() == 14
        && id
            .bytes()
            .all(|byte| b"23456789abcdefghjkmnpqrstuvwxyz".contains(&byte))
}

/// A key this machine keeps for a share: the receipt of a finished upload,
/// or, with its envelope, one still to be acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Receipt {
    account: String,
    server: String,
    key: String,
    sha256: String,
    url: String,
    envelope: Vec<u8>,
}

impl Receipt {
    /// As Go's `json.Encoder` writes the same receipt.
    fn encode(&self) -> String {
        let text = |value: &str| slink_format::encode(&Value::String(value.to_owned()));
        let mut fields = Vec::new();
        if !self.account.is_empty() {
            fields.push(format!("\"account\":{}", text(&self.account)));
        }
        fields.push(format!("\"server\":{}", text(&self.server)));
        fields.push(format!("\"key\":{}", text(&self.key)));
        fields.push(format!("\"sha256\":{}", text(&self.sha256)));
        if !self.url.is_empty() {
            fields.push(format!("\"url\":{}", text(&self.url)));
        }
        if !self.envelope.is_empty() {
            fields.push(format!(
                "\"envelope\":{}",
                text(&STANDARD.encode(&self.envelope))
            ));
        }
        format!("{{{}}}\n", fields.join(","))
    }

    fn decode(text: &str) -> Option<Receipt> {
        let saved = slink_format::parse(text).ok()?;
        let field = |name: &str| match &saved[name] {
            Value::Null => Some(String::new()),
            Value::String(text) => Some(text.clone()),
            _ => None,
        };
        let envelope = field("envelope")?;
        Some(Receipt {
            account: field("account")?,
            server: field("server")?,
            key: field("key")?,
            sha256: field("sha256")?,
            url: field("url")?,
            envelope: STANDARD.decode(envelope).ok()?,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The pending receipt for this text on this server, when there is one: the
/// first by name of those that start with `prefix`.
fn pending_file(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|rest| rest.ends_with(".json"))
        })
        .collect();
    names.sort();
    names.first().map(|name| dir.join(name))
}

/// A new envelope for `text`, under a key made for it alone.
fn seal(text: &str) -> Result<(Vec<u8>, String), String> {
    let random = SystemRandom::new();
    let (mut key, mut nonce) = ([0; 32], [0; 12]);
    random
        .fill(&mut key)
        .and_then(|()| random.fill(&mut nonce))
        .map_err(|_| "no randomness for the content key".to_owned())?;
    let data =
        envelope::encrypt(text.as_bytes(), &key, nonce).map_err(|error| error.to_string())?;
    Ok((data, slink_protocol::encoding::encode(&key)))
}

/// Publishes `text` to `server` as an encrypted link, with the key of the
/// account `api_key` names. `home` keeps the content key, pending until the
/// service acknowledges the upload and then as the share's receipt.
pub async fn upload(home: &Path, text: &str, server: &str, api_key: &str) -> Upload {
    let fail = |code: &str, message: &str| error(0, code, message);
    let server = match origin(server) {
        Ok(server) => server,
        Err(Refused::Invalid) => return fail("invalid_server", "invalid server URL"),
        Err(Refused::Insecure) => {
            return fail(
                "invalid_server",
                "use HTTPS for the server (HTTP is allowed only on localhost)",
            );
        }
    };
    if api_key.is_empty() {
        return error(401, "auth_required", "run slink login before publishing");
    }
    if text.len() > MAX_PLAINTEXT {
        return error(413, "too_large", "session exceeds the 25 MiB limit");
    }
    let valid = slink_format::parse(text)
        .is_ok_and(|document| slink_format::validate(&document).is_empty());
    if !valid {
        return error(422, "invalid_session", "not a valid session document");
    }
    if !slink_format::scan_for_secrets(text).is_empty() {
        return error(
            422,
            "secrets_detected",
            "redact credentials before publishing",
        );
    }

    let pending = home.join("pending-shares");
    let prefix = hex(&Sha256::digest(format!("{server}\0{text}").as_bytes()));
    let mut pending_path = pending.join(format!("{prefix}.json"));
    if let Some(found) = pending_file(&pending, &prefix) {
        pending_path = found;
    }
    let mut receipt = match fs::read_to_string(&pending_path) {
        Ok(saved) => {
            let Some(receipt) = Receipt::decode(&saved) else {
                return fail("local_keys", "cannot read the pending encryption receipt");
            };
            let opened = envelope::decrypt(&receipt.envelope, &receipt.key);
            if receipt.server != server || opened.ok().as_deref() != Some(text.as_bytes()) {
                return fail(
                    "local_keys",
                    "pending encryption receipt failed verification",
                );
            }
            receipt
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return fail("local_keys", "cannot read local encryption keys");
        }
        Err(_) => {
            let (data, key) = match seal(text) {
                Ok(sealed) => sealed,
                Err(message) => return fail("encryption_failed", &message),
            };
            let receipt = Receipt {
                account: String::new(),
                server: server.clone(),
                key,
                sha256: hex(&Sha256::digest(&data)),
                url: String::new(),
                envelope: data,
            };
            pending_path = pending.join(format!("{prefix}-{}.json", receipt.sha256));
            if write_private(&pending_path, receipt.encode().as_bytes()).is_err() {
                return fail(
                    "local_keys",
                    "cannot save the encryption key; nothing was uploaded",
                );
            }
            receipt
        }
    };

    let Ok(http) = client(Duration::from_secs(5 * 60)) else {
        return fail("invalid_server", "cannot build the HTTP client");
    };
    let sent = http
        .post(format!("{server}/api/shares"))
        .header("content-type", "application/vnd.session-link.encrypted")
        .header("authorization", format!("Bearer {api_key}"))
        .body(receipt.envelope.clone())
        .send()
        .await;
    let Ok(response) = sent else {
        return fail(
            "unreachable",
            "upload was not acknowledged; retry to recover it with the saved local key",
        );
    };
    let status = response.status().as_u16();
    let answer = object(response, 64 * 1024).await;
    if !(200..300).contains(&status) {
        let Some(body) = answer else {
            return error(
                status,
                "upload_failed",
                "encrypted upload failed; no plaintext fallback was attempted",
            );
        };
        return Upload {
            ok: false,
            status,
            body,
        };
    }
    let acknowledged = answer.filter(|body| {
        body.get("id").and_then(Value::as_str).is_some_and(share_id)
            && body.get("sha256").and_then(Value::as_str) == Some(receipt.sha256.as_str())
    });
    let Some(mut body) = acknowledged else {
        return fail(
            "invalid_response",
            "server did not acknowledge the expected encrypted document; retry to recover the share",
        );
    };
    let id = body["id"].as_str().unwrap_or_default().to_owned();
    // The link is made on the configured server: an answer cannot send the
    // key to another site's JavaScript.
    receipt.url = format!("{server}/s/{id}#key={}", receipt.key);
    receipt.envelope.clear();
    receipt.account = body
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let saved = home
        .join("shares")
        .join(format!("{}-{id}.json", receipt.sha256));
    if write_private(&saved, receipt.encode().as_bytes()).is_err() {
        return fail(
            "local_keys",
            "upload succeeded; its key remains in pending-shares. Retry to save the complete link",
        );
    }
    let _ = fs::remove_file(&pending_path);
    body.insert("url".to_owned(), Value::String(receipt.url));
    let backup = Identity::new(home, &server, api_key)
        .backup(&receipt.account)
        .await;
    body.insert("backup".to_owned(), Value::String(backup.to_owned()));
    Upload {
        ok: true,
        status,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_is_written_as_go_writes_it_and_read_back() {
        let receipt = Receipt {
            account: String::new(),
            server: "http://127.0.0.1:1".to_owned(),
            key: "k".to_owned(),
            sha256: "ab".to_owned(),
            url: String::new(),
            envelope: vec![0xfb, 0xff],
        };
        let text = receipt.encode();
        assert_eq!(
            text,
            "{\"server\":\"http://127.0.0.1:1\",\"key\":\"k\",\"sha256\":\"ab\",\"envelope\":\"+/8=\"}\n"
        );
        assert_eq!(Receipt::decode(&text), Some(receipt));
        assert_eq!(Receipt::decode("{\"server\":1}"), None);
    }

    #[test]
    fn share_ids_are_the_services_alphabet() {
        assert!(share_id("23456789abcdef"));
        assert!(!share_id("23456789abcde"));
        assert!(!share_id("23456789abcdei"));
        assert!(!share_id("1234567890abcd"));
    }
}
