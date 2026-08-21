//! cairnd: configuration, initialization order, and the serving loop.
//!
//! Init order is the security property: archives are opened and the listener is
//! bound while the process still can, workers are created, and only then does
//! the daemon confine itself. Nothing is served before that.

pub mod catalog;
pub mod config;
pub mod listener;
pub mod log;
pub mod server;

/// Release name reported by `--version` and `/v1/status`, defined once in
/// `api` so the daemon and its answers cannot disagree.
pub use api::VERSION;
