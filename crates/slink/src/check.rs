//! `slink-rs build-check` exercises what the build links in (bundled SQLite,
//! the embedded viewer, the HTTP server and client, TLS, the format crate) so
//! that a release binary can be proven on each target. It is scaffolding for the build spike
//! and goes when real commands reach the same code.

use std::error::Error;
use std::fmt::Write as _;
use std::process::ExitCode;

use axum::http::header::CONTENT_TYPE;
use sha2::{Digest, Sha256};

use crate::{VERSION, tls, viewer};

type Checked = Result<String, Box<dyn Error>>;

pub fn run() -> ExitCode {
    println!("slink-rs {VERSION} ({})", env!("SLINK_TARGET"));
    let mut failed = false;
    for (name, result) in [
        ("sqlite", sqlite()),
        ("viewer", embedded()),
        ("http", http()),
        ("format", format()),
    ] {
        match result {
            Ok(detail) => println!("ok    {name:<7} {detail}"),
            Err(err) => {
                failed = true;
                println!("FAIL  {name:<7} {err}");
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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
        if hex(&Sha256::digest(file.bytes)) != file.sha256 {
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

/// Serves the bundle on loopback and fetches it with the publishing client.
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
    tls::install();
    let client = reqwest::Client::builder().no_proxy().build()?;
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a string");
        out
    })
}
