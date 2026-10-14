//! `slink login`, `slink logout` and `slink delete`: the account a session is
//! published under, and taking a published session offline.

use std::io::{self, Write};
use std::time::Duration;

use slink_client::login::{self, LoggedIn};
use slink_client::{config, delete, resolve_target};
use slink_local::home;

use crate::flags::{Exit, Parsed, Run};
use crate::publish::{answer, block_on, interactive, transport_cause, yes};
use crate::text::quoted;
use crate::view::launch;

pub fn login(parsed: &Parsed) -> Run {
    let (key, server) = (parsed.text("key"), parsed.text("server"));
    if !key.is_empty() {
        let saved = login::with_key(&home(), key, server).map_err(Exit::failure)?;
        eprintln!("✓ saved to {}", saved.config.display());
        return Ok(());
    }
    let target = resolve_target(&home(), server, "").0;
    let signed = browser_login(&target)?;
    eprintln!(
        "✓ logged in as @{} to {target} — saved to {}",
        signed.login,
        signed.config.display()
    );
    if !signed.previous_server.is_empty() && signed.previous_server != target {
        eprintln!(
            "  note: default push target changed from {} to {target}",
            signed.previous_server
        );
    }
    Ok(())
}

/// Signs in through a browser: the code is shown here and confirmed there.
pub fn browser_login(server: &str) -> Result<LoggedIn, Exit> {
    block_on(async {
        let attempt = login::begin(server, "terminal").await?;
        eprintln!(
            "Opening {}\n  confirm this code in the browser: {}",
            attempt.url, attempt.user_code
        );
        eprintln!("  if the browser doesn't open, visit the URL yourself");
        // The URL is printed, so a browser that does not open is no failure.
        let _ = launch(&attempt.url);
        login::wait(&home(), &attempt, Duration::from_secs(2)).await
    })?
    .map_err(Exit::failure)
}

pub fn logout(_parsed: &Parsed) -> Run {
    let mut saved = config::read(&home());
    if saved.api_key.is_empty() {
        eprintln!("not signed in");
        return Ok(());
    }
    let was = std::mem::take(&mut saved.login);
    saved.api_key.clear();
    config::write(&home(), &saved).map_err(|error| Exit::failure(error.to_string()))?;
    if was.is_empty() {
        eprintln!("✓ signed out — the key was removed from this machine");
    } else {
        eprintln!("✓ signed out @{was} — the key was removed from this machine");
    }
    Ok(())
}

/// The id in a published URL as a browser copies it, with its query,
/// fragment and trailing slash, and whether it names an encrypted share.
fn published(reference: &str) -> Option<(String, bool)> {
    let encrypted = reference.contains("/s/") || reference.starts_with("s/");
    let mut id = reference;
    for separator in ['?', '#'] {
        if let Some(index) = id.find(separator) {
            id = &id[..index];
        }
    }
    for route in ["/r/", "/s/"] {
        if let Some(index) = id.rfind(route) {
            id = &id[index + 3..];
        }
    }
    let id = id.strip_prefix("s/").unwrap_or(id);
    let id = id.strip_suffix('/').unwrap_or(id);
    (!id.is_empty() && !id.contains(['/', '?', '#'])).then(|| (id.to_owned(), encrypted))
}

pub fn delete(parsed: &Parsed) -> Run {
    let reference = parsed.arg(0);
    if reference.is_empty() {
        return Err(Exit::failure(
            "which one? pass the published URL or its id: slink delete <url|id>",
        ));
    }
    let Some((id, mut encrypted)) = published(reference) else {
        return Err(Exit::failure(format!(
            "{} doesn't look like a published URL or id",
            quoted(reference)
        )));
    };
    let (target, key) = resolve_target(&home(), parsed.text("server"), parsed.text("key"));
    if key.is_empty() {
        return Err(Exit::failure(
            "not signed in — run: slink login  (deleting needs the account that published it)",
        ));
    }
    if !parsed.bool("yes") {
        // A script must never wait on a question nobody sees.
        if !interactive() {
            return Err(Exit::failure(
                "not a TTY — pass --yes to delete from scripts",
            ));
        }
        eprintln!("about to take published session {id} on {target} offline");
        eprintln!("  the link stops working immediately");
        eprint!("\nTake it offline? [y/N] ");
        let _ = io::stderr().flush();
        if !yes(&answer()) {
            return Err(Exit::failure("aborted — still online"));
        }
    }
    let (status, body) = block_on(async {
        if encrypted {
            return delete::share(&target, &key, &id).await;
        }
        // A bare id may be either kind: a session published before links
        // were encrypted is tried first, as Go tries it.
        let (status, body) = delete::run(&target, &key, &id).await;
        if status != 404 {
            return (status, body);
        }
        let (status, body) = delete::share(&target, &key, &id).await;
        encrypted = status != 404;
        (status, body)
    })?;
    match status {
        200..=299 | 410 => {
            eprintln!("✓ taken offline — the page now shows a deletion notice");
            let route = if encrypted { "s" } else { "r" };
            eprintln!("    url:  {target}/{route}/{id}");
            if !encrypted {
                eprintln!("\n  note: publishing the identical session again would revive this URL");
            }
            Ok(())
        }
        0 => Err(Exit::failure(format!(
            "✗ can't reach {target} ({})\n    nothing was deleted",
            transport_cause(&body)
        ))),
        401 => Err(Exit::failure("✗ not signed in — run: slink login")),
        403 => Err(Exit::failure(
            "✗ that session was published by a different account",
        )),
        404 => Err(Exit::failure(format!(
            "✗ no published session {id} on {target}"
        ))),
        status => Err(Exit::failure(format!(
            "✗ delete failed (HTTP {status}): {body}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::published;

    #[test]
    fn a_published_reference_is_read_as_a_browser_copies_it() {
        for (reference, want) in [
            (
                "https://session.link/s/23456789abcdef#key=x",
                Some(("23456789abcdef", true)),
            ),
            (
                "https://session.link/r/9f3kx2mvq7wtd4/?ref=1",
                Some(("9f3kx2mvq7wtd4", false)),
            ),
            ("s/23456789abcdef", Some(("23456789abcdef", true))),
            ("23456789abcdef", Some(("23456789abcdef", false))),
            ("https://session.link/", None),
            ("a/b", None),
            ("#x", None),
        ] {
            assert_eq!(
                published(reference),
                want.map(|(id, encrypted)| (id.to_owned(), encrypted)),
                "{reference}"
            );
        }
    }
}
