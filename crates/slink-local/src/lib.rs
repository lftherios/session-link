//! The slink home on this machine: where it is, how its files are kept
//! private, the captures in it and the previews saved from them.
//!
//! Nothing here reaches the network. Every function that needs the time or
//! random bytes is handed them, so a test can say what they are.

mod captures;
mod drafts;
mod home;
mod preview;
pub mod share;
mod time;

pub use captures::{Capture, capture_id, list_captures, new_capture_path};
pub use drafts::{Drafts, SaveError, SavedView, ViewRecord, session_title_key};
pub use home::{capture_dir, display_path, home, preview_dir, secure_home, write_private};
pub use preview::{READING_SUFFIX, remember_preview, reusable_preview, save_preview, source_stamp};
pub use time::{ago, stamp, unix_now};
