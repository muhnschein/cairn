//! Adapter from the archive layer to what the HTTP layer asks for.
//!
//! This is the only place the two meet: `api` has no dependency that can open
//! a file, and `archive` has no idea what HTTP is.

use api::{ArchiveSummary, Catalog, CatalogError, EntryContent, Metadata, SharedBytes, Suggestion};
use archive::LookupError;

/// Wraps an open catalog for the router.
#[derive(Debug)]
pub struct Archives {
    inner: archive::Catalog,
}

impl Archives {
    /// Wrap an open catalog.
    pub fn new(inner: archive::Catalog) -> Archives {
        Archives { inner }
    }

    /// The wrapped catalog, for status counters and startup logging.
    pub fn inner(&self) -> &archive::Catalog {
        &self.inner
    }
}

// Used point-free as `map_err(map)`, which needs an owned argument.
#[allow(clippy::needless_pass_by_value)]
fn map(e: LookupError) -> CatalogError {
    match e {
        LookupError::NoSuchArchive => CatalogError::NoSuchArchive,
        LookupError::NoSuchEntry => CatalogError::NoSuchEntry,
        LookupError::Corrupt(_) => CatalogError::Corrupt,
    }
}

fn summary_of(s: archive::Summary) -> ArchiveSummary {
    ArchiveSummary {
        uuid: s.uuid.to_string(),
        title: s.title,
        entry_count: u64::from(s.entry_count),
        cluster_count: u64::from(s.cluster_count),
        main_page: s.main_page,
        major_version: s.major_version,
        minor_version: s.minor_version,
        content_namespace: s.content_namespace,
        suggest: s.has_title_index,
    }
}

impl Catalog for Archives {
    fn archives(&self) -> Vec<ArchiveSummary> {
        self.inner
            .archives()
            .iter()
            .map(|a| summary_of(a.summary()))
            .collect()
    }

    fn summary(&self, uuid: &str) -> Result<ArchiveSummary, CatalogError> {
        self.inner.summary(uuid).map(summary_of).map_err(map)
    }

    fn metadata(&self, uuid: &str) -> Result<Metadata, CatalogError> {
        let m = self.inner.metadata(uuid).map_err(map)?;
        Ok(Metadata {
            text: m.text,
            binary: m.binary,
        })
    }

    fn entry(&self, uuid: &str, path: &str) -> Result<EntryContent, CatalogError> {
        let e = self.inner.entry(uuid, path).map_err(map)?;
        Ok(EntryContent {
            path: e.path,
            mime: e.mime,
            body: SharedBytes::new(e.blob.data, e.blob.start, e.blob.end),
        })
    }

    fn random(&self, uuid: &str, pick: u64) -> Result<String, CatalogError> {
        self.inner.random(uuid, pick).map_err(map)
    }

    fn suggest(
        &self,
        uuid: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>, CatalogError> {
        Ok(self
            .inner
            .suggest(uuid, prefix, limit)
            .map_err(map)?
            .into_iter()
            .map(|s| Suggestion {
                title: s.title,
                path: s.path,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two layers have separate error types on purpose; this is the only
    /// place they meet, and a wrong arm here turns a missing entry into a 503.
    #[test]
    fn lookup_failures_map_onto_the_fault_the_client_sees() {
        assert_eq!(map(LookupError::NoSuchArchive), CatalogError::NoSuchArchive);
        assert_eq!(map(LookupError::NoSuchEntry), CatalogError::NoSuchEntry);
        assert_eq!(
            map(LookupError::Corrupt(archive::zimfmt::Error::MimeList)),
            CatalogError::Corrupt,
            "a malformed archive is 503 for that archive, never a 500"
        );
    }
}
