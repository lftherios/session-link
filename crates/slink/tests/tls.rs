//! The publishing client's TLS, apart from any command: rustls with ring
//! completes a handshake with a certificate it trusts and refuses one it does
//! not, and the operating system's trust store verifies the hosted service.

use std::net::SocketAddr;
use std::sync::Arc;

use rcgen::CertifiedKey;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn install_provider() {
    // An error means another test installed it first.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn certificate() -> Result<CertifiedKey<rcgen::KeyPair>, rcgen::Error> {
    rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
}

/// Answers one HTTPS request on loopback with the given certificate.
async fn serve_once(
    identity: &CertifiedKey<rcgen::KeyPair>,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        identity.signing_key.serialize_der(),
    ));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.der().clone()], key)?;
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        // A client that distrusts the certificate ends the handshake here.
        let mut stream = acceptor.accept(stream).await?;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await?);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await?;
        stream.shutdown().await
    });
    Ok(address)
}

fn client_trusting(
    identity: &CertifiedKey<rcgen::KeyPair>,
    server: SocketAddr,
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([reqwest::Certificate::from_der(identity.cert.der())?])
        .resolve("localhost", server)
        .build()
}

#[tokio::test]
async fn completes_a_handshake_with_a_certificate_it_trusts() -> Fallible {
    install_provider();
    let identity = certificate()?;
    let server = serve_once(&identity).await?;
    let response = client_trusting(&identity, server)?
        .get(format!("https://localhost:{}/", server.port()))
        .send()
        .await?;
    assert_eq!(response.text().await?, "ok");
    Ok(())
}

#[tokio::test]
async fn refuses_a_certificate_it_was_not_told_to_trust() -> Fallible {
    install_provider();
    let server = serve_once(&certificate()?).await?;
    let error = client_trusting(&certificate()?, server)?
        .get(format!("https://localhost:{}/", server.port()))
        .send()
        .await
        .expect_err("the server's certificate is not the trusted one");
    assert!(error.is_connect(), "{error:?}");
    Ok(())
}

#[tokio::test]
#[ignore = "reaches the network; CI runs it on every target"]
async fn the_system_trust_store_verifies_the_hosted_service() -> Fallible {
    install_provider();
    let response = reqwest::Client::builder()
        .build()?
        .get("https://session.link/api/health")
        .send()
        .await?;
    assert!(response.status().is_success(), "{}", response.status());
    Ok(())
}
