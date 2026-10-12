//! TLS for outgoing requests: rustls with ring, verified against the operating
//! system's trust store.

/// Makes ring the process's rustls provider. Call before building a client.
pub fn install() {
    // An error means a provider is already installed, which is the goal.
    let _ = rustls::crypto::ring::default_provider().install_default();
}
