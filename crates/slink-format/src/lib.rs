//! The `session/v0` format in Rust: reading a document's text, validation
//! against the canonical schema, the invariants the schema cannot express,
//! and the credential scan.
//!
//! The schema and the pattern list are embedded from `packages/format`, where
//! they are authoritative, so this crate holds no copy of either. Nothing here
//! touches the network or the filesystem.

mod datetime;
mod parse;
mod scan;
mod time;
mod validate;
mod write;

pub use parse::parse;
pub use scan::{Hit, MAX_HITS, scan_for_secrets};
pub use time::instant;
pub use validate::{MAX_SCHEMA_ERRORS, SCHEMA, invariant_errors, validate};
pub use write::{compact, encode};
