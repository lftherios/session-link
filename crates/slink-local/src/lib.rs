//! The slink home on this machine: where it is, how its files are kept
//! private, the captures in it and the previews saved from them.
//!
//! Nothing here reaches the network. Every function that needs random bytes
//! or the time is handed them, so a test can say what they are; `unix_now`
//! is the one that reads the clock, for the callers to hand on.

mod captures;
mod drafts;
mod home;
mod preview;
pub mod share;
mod time;

use sha2::{Digest, Sha256};

pub use captures::{Capture, capture_id, list_captures, new_capture_path};
pub use drafts::{Drafts, SaveError, SavedView, ViewRecord, session_title_key};
pub use home::{
    capture_dir, create_private_dir, display_path, home, preview_dir, secure_home, write_private,
};
pub use preview::{READING_SUFFIX, remember_preview, reusable_preview, save_preview, source_stamp};
pub use time::{ago, ago_unix, stamp, unix_now};

/// The SHA-256 of `bytes` in lower-case hex: the name of a saved preview and
/// of a pending upload, and what a share's ciphertext is checked by.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Bytes as lower-case hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
