//! The hosted service as the slink CLI reaches it: signing in, publishing a
//! session as an encrypted link, and taking one offline. Each request goes
//! only to a server a key may be sent to, and follows no redirect.
//!
//! Device keys, the account's vault and named shares are not here yet;
//! docs/rust-client.md says what is.

pub mod config;
pub mod delete;
pub mod login;
mod origin;
pub mod publish;
mod transport;

pub use config::{Config, DEFAULT_SERVER, resolve_target};
pub use origin::{Refused, origin};
pub use publish::{Upload, share_id, upload};
pub use transport::cause;
