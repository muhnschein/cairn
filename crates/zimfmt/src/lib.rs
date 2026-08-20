//! ZIM archive parser, written from the openZIM specification.
//!
//! Every input here is untrusted: offsets are checked against the file length,
//! indices against their declared counts, and decompressed output against a
//! caller-supplied bound. Nothing in this crate performs I/O.

#![forbid(unsafe_code)]

mod bytes;
pub mod cluster;
pub mod decompress;
pub mod dirent;
mod error;
pub mod header;
mod layout;
mod uuid;
mod zim;

pub use cluster::Cluster;
pub use dirent::{Dirent, Target};
pub use error::{Error, Result};
pub use header::{Header, MAGIC};
pub use layout::Layout;
pub use uuid::Uuid;
pub use zim::{MAX_REDIRECT_HOPS, Zim};
