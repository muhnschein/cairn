//! cairn's HTTP surface.
//!
//! Raw request bytes in, responses out. This crate opens nothing, spawns
//! nothing, and has no dependencies: the client is hostile and the archive is
//! hostile, and both meet here.

#![forbid(unsafe_code)]

mod catalog;
mod fault;
mod json;
mod limits;
pub mod percent;
pub mod range;
mod request;
mod response;
mod router;
mod status;
pub mod token;

pub use catalog::{ArchiveSummary, Catalog, CatalogError, EntryContent, Metadata, Suggestion};
pub use fault::Fault;
pub use json::Json;
pub use limits::{Limits, RateLimiter};
pub use request::{Method, ParseError, Request};
pub use response::{Payload, Response, SharedBytes, reason};
pub use router::{Policy, Router, is_canonical_uuid};
pub use status::{Cache, Connections, Layer, Sandbox, Status};

/// Version reported by `/v1/status` and the `Server` header.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Map a parse failure onto the fault the client should see.
pub fn fault_for_parse_error(e: ParseError) -> Fault {
    match e {
        ParseError::Incomplete => Fault::BadRequest,
        ParseError::TooLong("request line") => Fault::UriTooLong,
        ParseError::TooLong(_) => Fault::HeadersTooLarge,
        ParseError::Malformed(_) => Fault::BadRequest,
        ParseError::UnsupportedVersion => Fault::VersionNotSupported,
        ParseError::BodyNotAllowed => Fault::BodyNotAllowed,
    }
}
