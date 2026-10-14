//! What the account's vault holds, under the vault key: the links this
//! account's devices published, the incoming-share keys once sharing with
//! named people is on, and the named shares a device is still granting.
//!
//! The vault is read as Go's client reads it and held to the same rules: a
//! receipt is for this server and this account, an incoming-share key opens
//! to the public key it is filed under, and a named share is one Go would
//! accept for this identity. A vault that breaks one is refused whole, so a
//! device never writes back less than it was given.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use slink_local::write_private;
use slink_protocol::encoding::{decode_exact, digest, encode};
use slink_protocol::{AgreementKey, State, Verified};

use crate::identity::{Error, refused};
use crate::named::{Outbox, read_outbox};
use crate::publish::share_id;

/// The most links, and named shares, a vault holds.
const MAX_ENTRIES: usize = 10_000;
/// The most incoming-share keys, one for each rotation.
const MAX_INBOXES: usize = 512;
/// The longest vault, before it is sealed.
pub const MAX_PLAINTEXT: usize = 4 * 1024 * 1024;

/// A link's key as the vault keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub server: String,
    pub account: String,
    pub key: String,
    pub sha256: String,
    pub url: String,
}

impl Receipt {
    /// From an object whose fields are text; a field that is missing or null
    /// is empty, as Go reads it.
    fn read(value: &Value) -> Option<Receipt> {
        let fields = value.as_object()?;
        let text = |name: &str| match fields.get(name) {
            None | Some(Value::Null) => Some(String::new()),
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => None,
        };
        Some(Receipt {
            server: text("server")?,
            account: text("account")?,
            key: text("key")?,
            sha256: text("sha256")?,
            url: text("url")?,
        })
    }

    /// As Go's `json.Marshal` writes the same receipt.
    pub fn encode(&self) -> String {
        let text = |value: &str| slink_format::encode(&Value::String(value.to_owned()));
        format!(
            "{{\"server\":{},\"account\":{},\"key\":{},\"sha256\":{},\"url\":{}}}",
            text(&self.server),
            text(&self.account),
            text(&self.key),
            text(&self.sha256),
            text(&self.url)
        )
    }

    /// Whether this is a whole link this account published on `server`: its
    /// key, the hash of its ciphertext, and the link made on that server.
    fn valid(&self, server: &str, account: &str) -> bool {
        let link = || {
            let rest = self.url.strip_prefix(server)?.strip_prefix("/s/")?;
            let (id, fragment) = rest.split_once('#')?;
            Some(share_id(id) && fragment.strip_prefix("key=") == Some(self.key.as_str()))
        };
        self.server == server
            && self.account == account
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && decode_exact::<32>(&self.key).is_ok()
            && link() == Some(true)
    }
}

/// The vault's contents.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contents {
    /// 1, or 2 once the account has an incoming-share key.
    pub version: u64,
    pub shares: Vec<Receipt>,
    /// Incoming-share private keys by their public keys, base64url.
    pub inboxes: BTreeMap<String, String>,
    /// Named shares still being granted, one for each ciphertext.
    pub named: Vec<Outbox>,
}

fn invalid() -> Error {
    refused("invalid key vault")
}

impl Contents {
    /// The vault of `verified`, opened with `key`, for this server.
    pub fn open(key: &[u8; 32], verified: &Verified, server: &str) -> Result<Contents, Error> {
        let raw = verified
            .open_vault(key)
            .map_err(|_| refused("cannot decrypt encrypted key vault"))?;
        if raw.len() > MAX_PLAINTEXT {
            return Err(invalid());
        }
        let value: Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        let data = Contents::read(&value).ok_or_else(invalid)?;
        if !matches!(data.version, 1 | 2) || data.shares.len() > MAX_ENTRIES {
            return Err(invalid());
        }
        let state = &verified.state;
        match &state.inbox {
            Some(inbox) => {
                if data.version != 2 || data.inboxes.len() > MAX_INBOXES {
                    return Err(refused("update slink to read this key vault"));
                }
                if !data.inboxes.contains_key(inbox) {
                    return Err(refused("incoming-share key is missing"));
                }
                for (public, private) in &data.inboxes {
                    let private = decode_exact::<32>(private)?;
                    if AgreementKey::from_bytes(private).public() != *public {
                        return Err(refused("invalid incoming-share key"));
                    }
                }
            }
            None if data.version != 1 || !data.inboxes.is_empty() => {
                return Err(refused("unbound incoming-share keys"));
            }
            None => {}
        }
        if data.named.len() > MAX_ENTRIES {
            return Err(refused("too many private shares"));
        }
        if !data
            .named
            .iter()
            .all(|item| item.valid(server, &state.account, &state.root))
        {
            return Err(refused("invalid private sharing backup"));
        }
        if !data
            .shares
            .iter()
            .all(|receipt| receipt.valid(server, &state.account))
        {
            return Err(refused("invalid share key in vault"));
        }
        Ok(data)
    }

    /// The fields Go's client reads, with the types it reads them as.
    fn read(value: &Value) -> Option<Contents> {
        let fields = value.as_object()?;
        let list = |name: &str| match fields.get(name) {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(items)) => Some(items.clone()),
            Some(_) => None,
        };
        let version = match fields.get("version") {
            None | Some(Value::Null) => 0,
            Some(number) => number.as_u64()?,
        };
        let shares = list("shares")?
            .iter()
            .map(Receipt::read)
            .collect::<Option<Vec<_>>>()?;
        let named = list("named")?
            .iter()
            .map(Outbox::read)
            .collect::<Option<Vec<_>>>()?;
        let inboxes = match fields.get("inboxes") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(keys)) => keys
                .iter()
                .map(|(public, private)| Some((public.clone(), private.as_str()?.to_owned())))
                .collect::<Option<_>>()?,
            Some(_) => return None,
        };
        Some(Contents {
            version,
            shares,
            inboxes,
            named,
        })
    }

    /// The contents as they are sealed: Go's fields, the empty ones left out
    /// as Go leaves them out.
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = Vec::new();
        if !self.named.is_empty() {
            let named: Vec<String> = self.named.iter().map(Outbox::encode).collect();
            fields.push(format!("\"named\":[{}]", named.join(",")));
        }
        if !self.inboxes.is_empty() {
            let inboxes = self
                .inboxes
                .iter()
                .map(|(public, private)| (public.clone(), Value::String(private.clone())))
                .collect();
            fields.push(format!(
                "\"inboxes\":{}",
                slink_format::encode(&Value::Object(inboxes))
            ));
        }
        fields.push(format!("\"version\":{}", self.version));
        let shares: Vec<String> = self.shares.iter().map(Receipt::encode).collect();
        fields.push(format!("\"shares\":[{}]", shares.join(",")));
        format!("{{{}}}", fields.join(",")).into_bytes()
    }

    /// Whether the named shares can be written for `state`: as many as a
    /// vault holds, in a vault that has the keys they need, each one Go would
    /// accept.
    pub fn named_fit(&self, server: &str, state: &State) -> bool {
        self.named.len() <= MAX_ENTRIES
            && (self.named.is_empty() || self.version == 2)
            && self
                .named
                .iter()
                .all(|item| item.valid(server, &state.account, &state.root))
    }

    /// The vault with every link and named share this machine keeps for the
    /// account added: the receipts in `home`'s `shares` that are whole links
    /// on `server` for this account, and the shares in its `named-shares`
    /// made under this root key. Any other file there stays local and
    /// blocks nothing.
    pub fn merge(
        mut self,
        home: &Path,
        server: &str,
        account: &str,
        root: &str,
    ) -> Result<Contents, Error> {
        let dir = home.join("shares");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries.collect::<io::Result<Vec<_>>>().map_err(io_error)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(io_error(error)),
        };
        let mut by_url: BTreeMap<String, Receipt> = self
            .shares
            .drain(..)
            .map(|receipt| (receipt.url.clone(), receipt))
            .collect();
        for entry in entries {
            let name = entry.file_name();
            if entry.file_type().map_err(io_error)?.is_dir()
                || !name.to_string_lossy().ends_with(".json")
            {
                continue;
            }
            let raw = fs::read(entry.path()).map_err(io_error)?;
            let Some(receipt) = serde_json::from_slice::<Value>(&raw)
                .ok()
                .as_ref()
                .and_then(Receipt::read)
            else {
                continue;
            };
            if receipt.valid(server, account) {
                by_url.insert(receipt.url.clone(), receipt);
            }
        }
        if self.version == 0 {
            self.version = 1;
        }
        // One entry for each ciphertext, this machine's over the vault's, in
        // the order Go writes them.
        let saved = read_outbox(home, server, account, root)?;
        let mut named = BTreeMap::new();
        for item in self.named.drain(..).chain(saved) {
            named.insert(item.sha256.clone(), item);
        }
        self.named = named.into_values().collect();
        self.shares = by_url.into_values().collect();
        if self.shares.len() > MAX_ENTRIES {
            return Err(refused("key vault is full"));
        }
        Ok(self)
    }

    /// Writes each link into `home`'s `shares`, and each named share this
    /// machine does not have into `named-shares`, so that a device that
    /// joins the account keeps the links and goes on granting. A file that
    /// already holds the receipt as it would be written is left alone, and
    /// a named share already here is this machine's to keep.
    pub fn import(&self, home: &Path) -> Result<(), Error> {
        for receipt in &self.shares {
            let name = digest(
                format!("{}\0{}\0{}", receipt.server, receipt.account, receipt.url).as_bytes(),
            );
            let file = home.join("shares").join(format!("{name}.json"));
            let text = receipt.encode();
            if fs::read(&file).is_ok_and(|saved| saved == text.as_bytes()) && private(&file) {
                continue;
            }
            write_private(&file, text.as_bytes()).map_err(io_error)?;
        }
        for item in &self.named {
            // The hash was checked as 64 hex digits when the vault opened.
            let file = home
                .join("named-shares")
                .join(format!("{}.json", item.sha256));
            if !file.exists() {
                write_private(&file, item.encode().as_bytes()).map_err(io_error)?;
            }
        }
        Ok(())
    }

    /// A new incoming-share key for `state`, kept here and named there. The
    /// old one stays, for the grants already made to it.
    pub fn rotate_inbox(&mut self, state: &mut State, private: [u8; 32]) {
        let key = AgreementKey::from_bytes(private);
        let public = key.public();
        self.inboxes.insert(public.clone(), encode(key.bytes()));
        state.inbox = Some(public);
        self.version = 2;
    }
}

#[cfg(unix)]
fn private(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(file).is_ok_and(|meta| meta.permissions().mode() & 0o777 == 0o600)
}

#[cfg(not(unix))]
fn private(_file: &Path) -> bool {
    true
}

fn io_error(error: io::Error) -> Error {
    refused(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(server: &str) -> Receipt {
        let key = encode(&[1; 32]);
        Receipt {
            server: server.to_owned(),
            account: "usr_a".to_owned(),
            key: key.clone(),
            sha256: "a".repeat(64),
            url: format!("{server}/s/23456789abcdef#key={key}"),
        }
    }

    #[test]
    fn a_receipt_is_a_whole_link_on_its_server_for_its_account() {
        let server = "https://session.link";
        let good = receipt(server);
        assert!(good.valid(server, "usr_a"));
        assert!(!good.valid(server, "usr_b"));
        assert!(!good.valid("https://other.example", "usr_a"));
        for broken in [
            Receipt {
                sha256: "A".repeat(64),
                ..good.clone()
            },
            Receipt {
                key: "short".to_owned(),
                ..good.clone()
            },
            Receipt {
                url: good.url.replace("#key=", "#k="),
                ..good.clone()
            },
            Receipt {
                url: good.url.replace("/s/", "/n/"),
                ..good.clone()
            },
            Receipt {
                url: good.url.replace("23456789abcdef", "23456789abcde1"),
                ..good.clone()
            },
            Receipt {
                url: good.url.replace(server, "https://session.link.evil"),
                ..good.clone()
            },
        ] {
            assert!(!broken.valid(server, "usr_a"), "{broken:?}");
        }
        assert_eq!(
            good.encode(),
            format!(
                "{{\"server\":\"{server}\",\"account\":\"usr_a\",\"key\":\"{}\",\"sha256\":\"{}\",\"url\":\"{}\"}}",
                good.key, good.sha256, good.url
            )
        );
    }

    #[test]
    fn contents_are_written_as_go_writes_them() {
        let mut data = Contents {
            version: 1,
            shares: vec![receipt("https://h")],
            ..Contents::default()
        };
        let text = String::from_utf8(data.encode()).unwrap();
        assert!(text.starts_with("{\"version\":1,\"shares\":[{\"server\":\"https://h\""));
        assert_eq!(
            Contents::read(&serde_json::from_str(&text).unwrap()),
            Some(data.clone())
        );
        data.version = 2;
        data.inboxes.insert("pub".to_owned(), "priv".to_owned());
        let text = String::from_utf8(data.encode()).unwrap();
        assert!(text.starts_with("{\"inboxes\":{\"pub\":\"priv\"},\"version\":2,"));
        // Types Go cannot read are refused; what is missing is empty.
        for bad in [
            r#"[]"#,
            r#"{"version":"1"}"#,
            r#"{"version":1.5}"#,
            r#"{"shares":{}}"#,
            r#"{"shares":[1]}"#,
            r#"{"shares":[{"url":1}]}"#,
            r#"{"named":[1]}"#,
            r#"{"inboxes":{"a":1}}"#,
        ] {
            assert_eq!(
                Contents::read(&serde_json::from_str(bad).unwrap()),
                None,
                "{bad}"
            );
        }
        assert_eq!(
            Contents::read(&serde_json::from_str("{}").unwrap()),
            Some(Contents::default())
        );
    }
}
