//! Fuzz target B: the client is hostile.
//!
//! Raw socket bytes: malformed request lines, absurd header counts, oversized
//! URIs, embedded NUL and CR, double-encoded and over-long percent sequences,
//! degenerate `Range` values, confusables in the uuid and in `q`. Parsing and
//! routing both run; the archive side is a stub so only the request path is
//! under test.

#![no_main]

use std::sync::Arc;

use api::{
    ArchiveSummary, Catalog, CatalogError, EntryContent, Limits, Metadata, Policy, Request, Router,
    SharedBytes, Status, Suggestion,
};
use libfuzzer_sys::fuzz_target;

const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";

struct Stub;

impl Catalog for Stub {
    fn archives(&self) -> Vec<ArchiveSummary> {
        vec![self.summary(UUID).unwrap()]
    }

    fn summary(&self, uuid: &str) -> Result<ArchiveSummary, CatalogError> {
        if uuid != UUID {
            return Err(CatalogError::NoSuchArchive);
        }
        Ok(ArchiveSummary {
            uuid: UUID.to_owned(),
            title: "Stub\u{7f}\r\n".to_owned(),
            entry_count: 1,
            cluster_count: 1,
            main_page: Some("index.html".to_owned()),
            major_version: 6,
            minor_version: 1,
            content_namespace: 'C',
            suggest: true,
        })
    }

    fn metadata(&self, _uuid: &str) -> Result<Metadata, CatalogError> {
        Ok(Metadata { text: vec![("Title".into(), "a\r\nb".into())], binary: vec!["I".into()] })
    }

    fn entry(&self, uuid: &str, path: &str) -> Result<EntryContent, CatalogError> {
        if uuid != UUID {
            return Err(CatalogError::NoSuchArchive);
        }
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(vec![b'x'; 64]);
        Ok(EntryContent {
            // Hostile archive data on the response side of a hostile request.
            path: format!("{path}\r\nX-Injected: yes"),
            mime: "text/html\r\nX-Injected: yes".to_owned(),
            body: SharedBytes::new(data, 0, 64),
        })
    }

    fn random(&self, _uuid: &str, pick: u64) -> Result<String, CatalogError> {
        Ok(format!("random-{pick}"))
    }

    fn suggest(&self, _uuid: &str, prefix: &str, limit: usize) -> Result<Vec<Suggestion>, CatalogError> {
        Ok((0..limit.min(4))
            .map(|i| Suggestion { title: format!("{prefix}{i}"), path: format!("{i}.html") })
            .collect())
    }
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok((request, consumed)) = Request::parse(data, &limits) else { return };
    assert!(consumed <= data.len());

    let router = Router::new(
        Arc::new(Stub),
        limits,
        Policy::default(),
        Box::new(Status::default),
        0x5eed,
    );
    let response = router.handle(&request);
    let head = response.head_bytes();

    // Whatever came in, the head must stay a well-formed set of header lines.
    let text = String::from_utf8(head).expect("response head is ascii");
    assert!(text.starts_with("HTTP/1.1 "));
    assert!(text.ends_with("\r\n\r\n"));
    for line in text.split("\r\n").skip(1).filter(|l| !l.is_empty()) {
        assert!(line.contains(": "), "stray head line {line:?}");
        assert!(!line.starts_with("X-Injected"), "header injection: {line:?}");
    }
});
