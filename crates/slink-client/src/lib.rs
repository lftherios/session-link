//! The hosted service as the slink CLI reaches it: signing in, publishing a
//! session as an encrypted link or for named people, taking one offline, and
//! this machine's part in the account's devices and the vault that backs its
//! shares up. Each request goes only to a server a key may be sent to, and
//! follows no redirect.

pub mod config;
pub mod delete;
mod device;
pub mod identity;
pub mod login;
pub mod named;
mod origin;
pub mod publish;
mod transport;
mod vault;

pub use config::{Config, DEFAULT_SERVER, resolve_target};
pub use identity::Identity;
pub use publish::{Upload, upload};
pub use transport::client;
