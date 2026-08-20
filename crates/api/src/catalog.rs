//! What the HTTP layer needs from whatever holds the archives.
//!
//! Deliberately narrow: an entry handle, some counts, and three lookups. No
//! ZIM concept crosses this line.

use crate::response::SharedBytes;

/// Why a lookup failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// No archive with that id is open.
    NoSuchArchive,
    /// No entry at that path.
    NoSuchEntry,
    /// The archive region backing this answer is malformed: 503, not 500.
    Corrupt,
}

/// One archive in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSummary {
    pub uuid: String,
    pub title: String,
    pub entry_count: u64,
    pub cluster_count: u64,
    pub main_page: Option<String>,
    pub major_version: u16,
    pub minor_version: u16,
    pub content_namespace: char,
    /// Whether `/suggest` can answer for this archive at all.
    pub suggest: bool,
}

/// An entry's content and what it resolved to.
#[derive(Debug, Clone)]
pub struct EntryContent {
    /// Path after redirect following.
    pub path: String,
    /// MIME type as stored in the archive. Unvalidated on purpose.
    pub mime: String,
    /// Entry bytes.
    pub body: SharedBytes,
}

/// A title-prefix suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub title: String,
    pub path: String,
}

/// Archive metadata, split by whether it is text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub text: Vec<(String, String)>,
    pub binary: Vec<String>,
}

/// The archives a router serves.
pub trait Catalog: Send + Sync {
    /// Every open archive.
    fn archives(&self) -> Vec<ArchiveSummary>;

    /// One archive.
    fn summary(&self, uuid: &str) -> Result<ArchiveSummary, CatalogError>;

    /// One archive's `M` namespace.
    fn metadata(&self, uuid: &str) -> Result<Metadata, CatalogError>;

    /// One entry, redirects followed.
    fn entry(&self, uuid: &str, path: &str) -> Result<EntryContent, CatalogError>;

    /// A random content path, selected using `pick`.
    fn random(&self, uuid: &str, pick: u64) -> Result<String, CatalogError>;

    /// Title-prefix suggestions, at most `limit` of them.
    fn suggest(
        &self,
        uuid: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>, CatalogError>;
}
