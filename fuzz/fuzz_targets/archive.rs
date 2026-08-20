//! Fuzz target A: the archive is hostile.
//!
//! Truncated headers, offsets past EOF, MIME tables without a terminator,
//! control bytes in MIME strings, descending cluster offsets, self-referential
//! redirects, decompression bombs, counts inconsistent with the pointer lists.
//! Nothing here may panic; every answer must be a `Result`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use zimfmt::{Layout, Target, Zim};

/// Same ceiling the daemon uses by default, so a bomb fails the same way.
const CLUSTER_LIMIT: usize = 32 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let Ok(layout) = Layout::parse(data) else { return };
    let zim = Zim::new(data, &layout);

    // Walking the whole archive is bounded by the declared counts, which the
    // layout already checked against the file length.
    let entries = zim.entry_count().min(4096);
    for i in 0..entries {
        let Ok(dirent) = zim.dirent(i) else { continue };
        let _ = zim.mime(dirent.mime_index);
        let _ = dirent.effective_title();
        let _ = zim.resolve(i);
        if let Target::Content { cluster, blob } = dirent.target
            && let Ok(c) = zim.cluster(cluster, CLUSTER_LIMIT)
        {
            let _ = c.blob(blob);
            let _ = c.blob(c.blob_count());
        }
    }

    if let Ok(Some(titles)) = zim.title_index() {
        for p in 0..titles.count().min(256) {
            if let Ok(index) = zim.title_entry(&titles, p) {
                let _ = zim.dirent(index);
            }
        }
        for ns in [b'C', b'A', 0xff] {
            let _ = zim.title_lower_bound(&titles, ns, b"a");
        }
    }

    for cluster in 0..zim.cluster_count().min(256) {
        let _ = zim.cluster_shape(cluster);
        let _ = zim.cluster(cluster, 64 * 1024);
    }

    // Lookups over whatever ordering the file claims to have.
    for ns in [b'C', b'A', b'M', b'-', 0xff] {
        let _ = zim.find(ns, b"index.html");
        let _ = zim.find(ns, b"");
        let _ = zim.url_lower_bound(ns, b"a");
    }
});
