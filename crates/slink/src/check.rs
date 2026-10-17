//! `slink-rs build-check` exercises what the build links in (bundled SQLite,
//! the embedded viewer, the HTTP server, the client every request to the
//! service is made with and its TLS, the format crate and the protocol crate)
//! in the binary that ships. It stays because nothing else runs that binary:
//! `cargo test` runs a test build, the release profile's LTO and stripped
//! symbols make another, and ring and x25519-dalek choose their code by
//! target. rust.yml runs it in the release binary on each of the six
//! targets' own runners. It needs no home, network or terminal, and it is
//! not a product command.

use std::error::Error;
use std::time::Duration;

use axum::http::header::CONTENT_TYPE;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use slink_local::sha256_hex;
use slink_protocol::encoding::{decode, decode_exact, encode};
use slink_protocol::{AgreementKey, Kind, Signed, envelope, unwrap, wrap};

use crate::flags::{Exit, Run};
use crate::{VERSION, viewer};

type Checked = Result<String, Box<dyn Error>>;

pub fn run() -> Run {
    println!("slink-rs {VERSION} ({})", env!("SLINK_TARGET"));
    let mut failed = false;
    for (name, result) in [
        ("sqlite", sqlite()),
        ("viewer", embedded()),
        ("http", http()),
        ("format", format()),
        ("protocol", protocol()),
    ] {
        match result {
            Ok(detail) => println!("ok    {name:<8} {detail}"),
            Err(err) => {
                failed = true;
                println!("FAIL  {name:<8} {err}");
            }
        }
    }
    if failed {
        Err(Exit {
            code: 1,
            message: String::new(),
        })
    } else {
        Ok(())
    }
}

fn sqlite() -> Checked {
    let db = rusqlite::Connection::open_in_memory()?;
    db.execute_batch(
        "create table part (id integer primary key, body text not null);
         insert into part (body) values ('a'), ('b');",
    )?;
    let (version, rows): (String, i64) = db.query_row(
        "select sqlite_version(), (select count(*) from part)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if rows != 2 {
        return Err(format!("wrote 2 rows and read {rows} back").into());
    }
    Ok(format!("bundled {version}, in-memory round trip"))
}

fn embedded() -> Checked {
    for file in viewer::FILES {
        if sha256_hex(file.bytes) != file.sha256 {
            return Err(format!("{} does not match the manifest", file.path).into());
        }
    }
    let bundle = viewer::file("viewer.js").ok_or("the artifact has no viewer.js")?;
    let manifest: serde_json::Value = serde_json::from_str(viewer::MANIFEST)?;
    let source = match (manifest["revision"].as_str(), manifest["dirty"].as_bool()) {
        (Some(revision), Some(false)) => format!("built at {revision}"),
        (Some(revision), _) => format!("built from changes on top of {revision}"),
        (None, _) => "built outside a checkout".into(),
    };
    Ok(format!(
        "{} files match the manifest; viewer.js is {} bytes, sha256 {}, {source}",
        viewer::FILES.len(),
        bundle.bytes.len(),
        bundle.sha256
    ))
}

fn http() -> Checked {
    let bundle = viewer::file("viewer.js").ok_or("the artifact has no viewer.js")?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(round_trip(bundle.bytes))
}

/// Serves the bundle on loopback and fetches it with the client every
/// request to the service is made with.
async fn round_trip(bundle: &'static [u8]) -> Checked {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let app = axum::Router::new().route(
        "/viewer.js",
        axum::routing::get(move || async move {
            ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], bundle)
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    // Building the client builds its TLS configuration, so this also proves
    // that rustls, ring and the platform verifier initialize on this target.
    let client = slink_client::client(Duration::from_secs(30))?;
    let body = client
        .get(format!("http://{address}/viewer.js"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    server.abort();
    if body.as_ref() != bundle {
        return Err(format!("fetched {} bytes that are not the bundle", body.len()).into());
    }
    Ok("served and fetched viewer.js over loopback; TLS client built".into())
}

/// The schema compiles and decides, and the credential patterns compile and
/// find, in this binary on this target.
fn format() -> Checked {
    const DOCUMENT: &str = r#"{"schema":"session/v0","created_at":"2026-07-19T10:00:00Z","spans":[{"id":"root","type":"agent"}]}"#;
    let document = slink_format::parse(DOCUMENT)?;
    let errors = slink_format::validate(&document);
    if !errors.is_empty() {
        return Err(format!("a valid document was refused: {}", errors[0]).into());
    }
    let undated = slink_format::parse(&DOCUMENT.replace("T10:00:00Z", ""))?;
    let errors = slink_format::validate(&undated);
    if !errors.iter().any(|error| error.starts_with("/created_at ")) {
        return Err(format!("a date for created_at was not refused there: {errors:?}").into());
    }
    // Assembled here so that no source file holds text shaped like a key.
    let key = format!("sk-{}", "a".repeat(16));
    let hits = slink_format::scan_for_secrets(&format!("é{key}"));
    if hits.len() != 1 || hits[0].pattern != "provider_api_key" {
        return Err(format!("the scan found {hits:?} where one provider key was").into());
    }
    Ok("schema compiled and applied; credential patterns compiled and applied".into())
}

/// Envelopes sealed by Web Crypto and by Go open, the shared signed
/// statements get their verdicts, and a key is wrapped as `node:crypto`
/// wrapped it, in this binary on this target. AES-GCM, Ed25519 and HKDF are
/// ring's, with assembly chosen for each target; X25519 is x25519-dalek's,
/// which chooses its arithmetic by target too.
fn protocol() -> Checked {
    const ENVELOPES: [(&str, &str); 2] = [
        (
            "Web Crypto",
            include_str!("../../../testdata/encryption/webcrypto.json"),
        ),
        ("Go", include_str!("../../../testdata/encryption/go.json")),
    ];
    const STATEMENTS: &str = include_str!("../../../testdata/protocol/statements.json");

    for (source, fixture) in ENVELOPES {
        let fixture: serde_json::Value = serde_json::from_str(fixture)?;
        let text = |field: &str| fixture[field].as_str().unwrap_or_default();
        let mut data = STANDARD.decode(text("envelope"))?;
        if envelope::decrypt(&data, text("key"))? != text("plaintext").as_bytes() {
            return Err(format!("the {source} envelope opened to another document").into());
        }
        let last = data.len() - 1;
        data[last] ^= 1;
        if envelope::decrypt(&data, text("key")).is_ok() {
            return Err(format!("the {source} envelope opened with its tag changed").into());
        }
    }

    let statements: Vec<serde_json::Value> = serde_json::from_str(STATEMENTS)?;
    for case in &statements {
        let text = |field: &str| case[field].as_str().unwrap_or_default();
        let kind = Kind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text("kind"))
            .ok_or("a statement of a kind this build does not know")?;
        let signed = Signed {
            payload: text("payload").to_owned(),
            signature: text("signature").to_owned(),
        };
        let verified = signed.verify(kind, text("public")).is_ok();
        if verified != (text("expect") == "verified") {
            return Err(format!("{}: verified is {verified}", text("name")).into());
        }
    }
    rewrap()?;
    Ok(format!(
        "{} envelopes opened and refused once changed; {} signed statements judged; a key wrapped and unwrapped",
        ENVELOPES.len(),
        statements.len()
    ))
}

/// The case `a wrapper` of `testdata/protocol/keys.json`: made again from
/// the keys and the nonce it was made with, and opened by its recipient only.
fn rewrap() -> Result<(), Box<dyn Error>> {
    const RECIPIENT: &str = "52dQH0e0i0aKrXjg8fxUEe74SaWY2VHvvTx9PCuNeCU";
    const EPHEMERAL: &str = "U5QdQRFrd-eA4BzmTAnbGYpbORyEf-Vp3CCATUxfC9U";
    const CONTEXT: &str = "usr_fixture/HbBq0ZHqcXnnu3PxoLwlwZKKszsqToq59Yjo7snN6ps/1";
    const KEY: &str = "imYdxhOQcz0t-QL6bI1BLD11yTb0egLz1KUPSSAAJ-w";
    const WRAPPER: &str = "yUv7n_8pMcnoPrIsOU1un0xPqci_yZNy7ojZAK1MVBg.NozgFU2NSqACJR2DV2EHhHgV1PYPlCd5lLXs_tTxTz4KYxQ4b92z6anu7okEh2ZQ9Io_ExXLlBE9uelh";

    let recipient = AgreementKey::from_bytes(decode_exact(RECIPIENT)?);
    let ephemeral = AgreementKey::from_bytes(decode_exact(EPHEMERAL)?);
    let key: [u8; 32] = decode_exact(KEY)?;
    let (_, sealed) = WRAPPER.split_once('.').ok_or("the wrapper has no dot")?;
    let nonce = *decode(sealed)?
        .first_chunk()
        .ok_or("the box has no nonce")?;
    let made = wrap(&recipient.public(), &key, CONTEXT, &ephemeral, nonce)?;
    if made != WRAPPER {
        return Err(format!("the wrapper was made as {made}").into());
    }
    if unwrap(&recipient, &made, CONTEXT)? != key {
        return Err("the wrapper opened to another key".into());
    }
    if unwrap(&ephemeral, &made, CONTEXT).is_ok() {
        return Err("the wrapper opened for a key it was not made for".into());
    }
    // A point of small order agrees with every key on the same bytes.
    if wrap(&encode(&[0; 32]), &key, CONTEXT, &ephemeral, nonce).is_ok() {
        return Err("a key was wrapped to a point of small order".into());
    }
    Ok(())
}
