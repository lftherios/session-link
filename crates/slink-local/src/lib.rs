//! The slink home on this machine: where it is, how its files are kept
//! private, the captures in it and the previews saved from them.
//!
//! Nothing here reaches the network. Every function that needs the time or
//! random bytes is handed them, so a test can say what they are.

mod captures;
mod home;
mod preview;
mod time;

pub use captures::{Capture, capture_id, list_captures, new_capture_path};
pub use home::{capture_dir, display_path, home, preview_dir, secure_home, write_private};
pub use preview::{READING_SUFFIX, save_preview};
pub use time::{ago, stamp, unix_now};
