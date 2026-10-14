//! Which servers a key is sent to. An API key, and the link a content key is
//! put in, go only to HTTPS, or to plain HTTP on this machine.
//!
//! The rule is Go's (`UploadRun` and `BeginLogin`): a URL with a host, no
//! user, query or fragment, whose scheme is `https`, or `http` with the host
//! `localhost` or a loopback address. Requests are made to the text as given,
//! trailing slashes removed, so the text is held to more than Go holds it to:
//! nothing in it may be read one way here and another way by the HTTP
//! client. docs/rust-client.md lists what that refuses that Go accepts.

use std::net::{IpAddr, Ipv6Addr};

/// Why a server is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Not a URL with a host, or one with a user, a query or a fragment.
    Invalid,
    /// Neither HTTPS nor plain HTTP on this machine.
    Insecure,
}

/// `server` without its trailing slashes, when a key may be sent to it.
pub fn origin(server: &str) -> Result<String, Refused> {
    let invalid = Refused::Invalid;
    // Go refuses control characters; a space, a backslash, a query or a
    // fragment, even an empty one, are refused here as well.
    if server
        .bytes()
        .any(|byte| byte <= b' ' || byte >= 0x7f || matches!(byte, b'\\' | b'?' | b'#'))
    {
        return Err(invalid);
    }
    let (scheme, rest) = server.split_once(':').ok_or(invalid)?;
    let mut letters = scheme.chars();
    if !letters.next().is_some_and(|c| c.is_ascii_alphabetic())
        || !letters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Err(invalid);
    }
    let rest = rest.strip_prefix("//").ok_or(invalid)?;
    let authority = rest.split('/').next().unwrap_or_default();
    // A user, even an empty one, is a URL Go refuses.
    if authority.contains('@') {
        return Err(invalid);
    }
    let host = hostname(authority).ok_or(invalid)?;
    let allowed = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => host == "localhost" || loopback(host),
        _ => false,
    };
    if !allowed {
        return Err(Refused::Insecure);
    }
    Ok(server.trim_end_matches('/').to_owned())
}

/// The host without its port or brackets. Only letters, digits, `-`, `.`,
/// `_` and `~` make a name, and a port is digits.
fn hostname(authority: &str) -> Option<&str> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => {
            let (inside, after) = rest.split_once(']')?;
            inside.parse::<Ipv6Addr>().ok()?;
            let port = if after.is_empty() {
                after
            } else {
                after.strip_prefix(':')?
            };
            (inside, port)
        }
        None => {
            let (host, port) = authority.rsplit_once(':').unwrap_or((authority, ""));
            let name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~');
            if !host.chars().all(name) {
                return None;
            }
            (host, port)
        }
    };
    (!host.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())).then_some(host)
}

/// Go's `net.ParseIP(host).IsLoopback()`: 127.0.0.0/8, `::1`, and IPv4's
/// loopback written as IPv6.
fn loopback(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => address.is_loopback(),
        Ok(IpAddr::V6(address)) => {
            address.is_loopback() || address.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Refused, origin};

    #[test]
    fn https_anywhere_and_http_only_here() {
        for (server, want) in [
            ("https://session.link", Ok("https://session.link")),
            ("https://session.link/", Ok("https://session.link")),
            ("https://session.link//", Ok("https://session.link")),
            ("HTTPS://session.link", Ok("HTTPS://session.link")),
            (
                "https://session.link:8443/base/",
                Ok("https://session.link:8443/base"),
            ),
            ("http://localhost:3838", Ok("http://localhost:3838")),
            ("http://127.0.0.1:3838/", Ok("http://127.0.0.1:3838")),
            ("http://127.8.9.10", Ok("http://127.8.9.10")),
            ("http://[::1]:3838", Ok("http://[::1]:3838")),
            ("http://[::ffff:127.0.0.1]", Ok("http://[::ffff:127.0.0.1]")),
            ("http://session.link", Err(Refused::Insecure)),
            ("http://LOCALHOST", Err(Refused::Insecure)),
            ("http://10.0.0.1", Err(Refused::Insecure)),
            ("http://[::2]", Err(Refused::Insecure)),
            ("http://127.0.0.1.example.com", Err(Refused::Insecure)),
            ("ftp://session.link", Err(Refused::Insecure)),
            ("session.link", Err(Refused::Invalid)),
            ("https:session.link", Err(Refused::Invalid)),
            ("https:///session.link", Err(Refused::Invalid)),
            ("https://", Err(Refused::Invalid)),
            ("https://:443", Err(Refused::Invalid)),
            ("https://user@session.link", Err(Refused::Invalid)),
            ("https://@session.link", Err(Refused::Invalid)),
            ("https://session.link?x=1", Err(Refused::Invalid)),
            ("https://session.link/?", Err(Refused::Invalid)),
            ("https://session.link#x", Err(Refused::Invalid)),
            ("https://session.link/#", Err(Refused::Invalid)),
            ("https://session.link:port", Err(Refused::Invalid)),
            ("https://session.link\\.evil.test", Err(Refused::Invalid)),
            ("https://session .link", Err(Refused::Invalid)),
            ("https://session.link/a b", Err(Refused::Invalid)),
            ("https://session.link\n", Err(Refused::Invalid)),
            ("https://séssion.link", Err(Refused::Invalid)),
            ("https://[::1", Err(Refused::Invalid)),
            ("https://[::1]x", Err(Refused::Invalid)),
            ("https://[nope]", Err(Refused::Invalid)),
            ("", Err(Refused::Invalid)),
        ] {
            assert_eq!(
                origin(server).as_deref().map_err(|refused| *refused),
                want,
                "{server:?}"
            );
        }
    }
}
