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

/// Release name reported by `/v1/status` and `--version`: `YYYY.0M`, with a
/// counter appended for a second release in one month.
///
/// Written out because semver cannot spell the leading zero in `08`; the
/// manifest carries the same release as `2026.8.0`.
pub const VERSION: &str = "2026.08";

/// Map a parse failure onto the fault the client should see.
pub fn fault_for_parse_error(e: ParseError) -> Fault {
    match e {
        ParseError::TooLong("request line") => Fault::UriTooLong,
        ParseError::TooLong(_) => Fault::HeadersTooLarge,
        ParseError::Incomplete | ParseError::Malformed(_) => Fault::BadRequest,
        ParseError::UnsupportedVersion => Fault::VersionNotSupported,
        ParseError::BodyNotAllowed => Fault::BodyNotAllowed,
    }
}

#[cfg(test)]
mod tests {
    use super::VERSION;

    /// Nothing but this connects the two spellings: a bump that touched only
    /// `Cargo.toml` would ship a daemon reporting last month's release.
    #[test]
    fn version_matches_the_manifest() {
        let major = env!("CARGO_PKG_VERSION_MAJOR");
        let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
        let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
        let expected = if patch == 0 {
            format!("{major}.{minor:02}")
        } else {
            format!("{major}.{minor:02}.{patch}")
        };
        assert_eq!(VERSION, expected);
    }

    /// The scheme, not this release: `2026.8` sorts wrong, `2026.13` is not a
    /// month.
    #[test]
    fn version_is_a_calendar_date() {
        let (year, rest) = VERSION.split_once('.').unwrap();
        let month = rest.split_once('.').map_or(rest, |(m, _)| m);
        assert_eq!(year.len(), 4, "the year is spelled in full: {VERSION}");
        assert_eq!(month.len(), 2, "the month is zero-padded: {VERSION}");
        assert!(year.parse::<u32>().unwrap() >= 2026);
        assert!(
            (1..=12).contains(&month.parse::<u32>().unwrap()),
            "{VERSION}"
        );
    }
}
