//! Excerpts. A person shares part of a session, not the session: they pick
//! pieces of what it holds, and a new document is built from those pieces
//! and nothing else.
//!
//! The catalog lists what a session offers for picking, as text. The export
//! makes the document, by a positive list of what may be in it, so that the
//! source's raw payloads, history, metadata and anything this code does not
//! know about cannot ride along.

mod catalog;
mod export;
#[cfg(test)]
mod tests;

pub use catalog::{Catalog, Unit, build_catalog};
pub use export::{Draft, Selection, export};

/// The name an excerpt carries what it is under, in a document's extensions.
pub const EXTENSION: &str = "session_link.share.v1";

/// Lower case as Go's `strings.ToLower` gives it: each character by itself,
/// with no rule that looks at its neighbours.
pub(crate) fn lower(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            // The one letter whose lower case is two characters; Go writes
            // the first.
            '\u{130}' => 'i',
            other => other.to_lowercase().next().unwrap_or(other),
        })
        .collect()
}
