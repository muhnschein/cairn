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

/// Version reported by `--version` and `/v1/status`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
