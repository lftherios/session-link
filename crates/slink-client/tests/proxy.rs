//! Which requests go through a proxy named in the environment: as with Go's
//! client, one to HTTPS does, and one to this machine never does, so an API
//! key sent to a local server over plain HTTP stays on the machine. Each
//! upload runs in a process of its own, so the variables reach no other
//! test.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

const VALID: &str = r#"{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","spans":[{"id":"root","type":"agent"}]}"#;

/// The head of the first request the proxy got while `server` was
/// published to with `variable` naming it, or nothing.
fn proxied(variable: &str, server: &str) -> String {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let port = proxy.local_addr().unwrap().port();
    let home = std::env::temp_dir().join(format!(
        "slink-client-proxy-{}-{variable}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    let mut child = Command::new(std::env::current_exe().unwrap());
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        child.env_remove(name);
    }
    let mut child = child
        .args(["--exact", "upload", "--ignored", "--nocapture"])
        .env(variable, format!("http://127.0.0.1:{port}"))
        .env("SLINK_PROXY_SERVER", server)
        .env("SLINK_PROXY_HOME", &home)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut head = String::new();
    while Instant::now() < deadline {
        match proxy.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                while !bytes.windows(4).any(|end| end == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => bytes.extend_from_slice(&chunk[..read]),
                    }
                }
                head = String::from_utf8_lossy(&bytes).into_owned();
                let _ = stream.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("{error}"),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&home);
    head
}

#[test]
fn a_server_on_this_machine_is_never_reached_through_a_proxy() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://127.0.0.1:{}", closed.local_addr().unwrap().port());
    drop(closed);
    for variable in ["HTTP_PROXY", "ALL_PROXY", "HTTPS_PROXY"] {
        let head = proxied(variable, &server);
        assert!(head.is_empty(), "{variable} was used:\n{head}");
    }
}

#[test]
fn https_goes_through_the_proxy_https_proxy_names() {
    let head = proxied("HTTPS_PROXY", "https://session.example.test");
    assert!(
        head.starts_with("CONNECT session.example.test:443 "),
        "{head}"
    );
    assert!(!head.contains("rk_test"), "{head}");
    // Go reads neither of these for HTTPS.
    for variable in ["HTTP_PROXY", "ALL_PROXY"] {
        let head = proxied(variable, "https://session.example.test");
        assert!(head.is_empty(), "{variable} was used:\n{head}");
    }
}

#[tokio::test]
#[ignore = "run by the tests above, with a proxy in its environment"]
async fn upload() {
    let server = std::env::var("SLINK_PROXY_SERVER").unwrap();
    let home = std::path::PathBuf::from(std::env::var("SLINK_PROXY_HOME").unwrap());
    let upload = slink_client::upload(&home, VALID, &server, "rk_test").await;
    println!("{upload:?}");
}
