//! Round-trip and hostile-archive tests over crafted archives.

use testutil::{Builder, Compression};
use zimfmt::{Error, Layout, Target, Zim};

const LIMIT: usize = 1 << 20;

fn open(bytes: &[u8]) -> (Layout, ()) {
    (Layout::parse(bytes).expect("layout"), ())
}

#[test]
fn reads_a_sample_archive() {
    let bytes = testutil::sample().build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);

    assert_eq!(zim.entry_count(), 8); // seven entries plus the title listing
    assert_eq!(zim.header().content_namespace(), b'C');
    assert_eq!(zim.mime(0), Some(&b"text/html"[..]));
    assert!(zim.find(b'M', b"Title").unwrap().is_some());

    let idx = zim.find(b'C', b"index.html").unwrap().expect("index.html");
    let d = zim.dirent(idx).unwrap();
    assert_eq!(d.effective_title(), b"Main Page");
    let Target::Content { cluster, blob } = d.target else {
        panic!("not content")
    };
    let c = zim.cluster(cluster, LIMIT).unwrap();
    assert_eq!(c.blob(blob).unwrap(), b"<html><body>index</body></html>");

    assert!(zim.find(b'C', b"missing.html").unwrap().is_none());
    assert!(zim.find(b'Z', b"index.html").unwrap().is_none());
}

#[test]
fn follows_a_redirect() {
    let bytes = testutil::sample().build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    let from = zim.find(b'C', b"home.html").unwrap().unwrap();
    let to = zim.resolve(from).unwrap();
    assert_eq!(zim.dirent(to).unwrap().url, b"index.html");
}

#[test]
fn self_referential_redirect_stops() {
    let mut bytes = Builder::new()
        .redirect("loop.html", "Loop", "loop.html")
        .build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    let i = zim.find(b'C', b"loop.html").unwrap().unwrap();
    assert_eq!(zim.resolve(i), Err(Error::RedirectDepth));
    bytes.clear();
}

#[test]
fn long_redirect_chain_is_abandoned() {
    let mut b = Builder::new();
    for i in 0..10 {
        b = b.redirect(&format!("r{i}.html"), "R", &format!("r{}.html", i + 1));
    }
    let bytes = b.content("r10.html", "End", 0, b"end").build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    let i = zim.find(b'C', b"r0.html").unwrap().unwrap();
    assert_eq!(zim.resolve(i), Err(Error::RedirectDepth));
}

#[test]
fn every_compression_round_trips() {
    for c in [Compression::None, Compression::Xz, Compression::Zstd] {
        for extended in [false, true] {
            let bytes = testutil::sample().compression(c).extended(extended).build();
            let (layout, _) = open(&bytes);
            let zim = Zim::new(&bytes, &layout);
            let i = zim.find(b'C', b"notes.txt").unwrap().unwrap();
            let Target::Content { cluster, blob } = zim.dirent(i).unwrap().target else {
                panic!("not content")
            };
            let cl = zim.cluster(cluster, LIMIT).unwrap();
            assert_eq!(
                cl.blob(blob).unwrap(),
                b"plain notes",
                "{c:?} extended={extended}"
            );
        }
    }
}

#[test]
fn decompression_bound_is_enforced() {
    let big = vec![b'a'; 100_000];
    let bytes = Builder::new()
        .compression(Compression::Zstd)
        .content("big.txt", "Big", 0, &big)
        .build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    assert_eq!(zim.cluster(0, 4096), Err(Error::TooLarge { limit: 4096 }));
    assert!(zim.cluster(0, 1 << 20).is_ok());
}

#[test]
fn legacy_namespaces() {
    let bytes = Builder::new()
        .version(5, 0)
        .content("index.html", "Main", 0, b"legacy")
        .content_in(b'I', "logo.png", "Logo", 0, b"png")
        .build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    assert_eq!(zim.header().content_namespace(), b'A');
    assert!(zim.find(b'A', b"index.html").unwrap().is_some());
    assert!(zim.find(b'I', b"logo.png").unwrap().is_some());
}

#[test]
fn title_order_supports_prefix_search() {
    let bytes = Builder::new()
        .content("a.html", "Apple", 0, b"a")
        .content("b.html", "Apricot", 0, b"b")
        .content("c.html", "Banana", 0, b"c")
        .build();
    let (layout, _) = open(&bytes);
    let zim = Zim::new(&bytes, &layout);
    let index = zim.title_index().unwrap().expect("a title ordering");
    let start = zim.title_lower_bound(&index, b'C', b"Ap").unwrap();
    let titles: Vec<Vec<u8>> = (start..index.count())
        .map(|p| {
            zim.dirent(zim.title_entry(&index, p).unwrap())
                .unwrap()
                .effective_title()
                .to_vec()
        })
        .take_while(|t| t.starts_with(b"Ap"))
        .collect();
    assert_eq!(titles, vec![b"Apple".to_vec(), b"Apricot".to_vec()]);
}

#[test]
fn header_rejects_junk() {
    assert!(matches!(Layout::parse(&[]), Err(Error::Truncated { .. })));
    assert!(matches!(Layout::parse(&[0u8; 80]), Err(Error::BadMagic(0))));

    let mut bytes = testutil::sample().build();
    bytes[8..24].copy_from_slice(&[0u8; 16]);
    assert_eq!(Layout::parse(&bytes), Err(Error::NilUuid));

    let mut bytes = testutil::sample().build();
    bytes[4..6].copy_from_slice(&9u16.to_le_bytes());
    assert!(matches!(
        Layout::parse(&bytes),
        Err(Error::UnsupportedVersion { .. })
    ));
}

#[test]
fn pointer_lists_must_fit_in_the_file() {
    // The URL and cluster pointer lists have no sentinel: a position that far
    // out is a malformed archive.
    for field in [32usize, 48] {
        let mut bytes = testutil::sample().build();
        bytes[field..field + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(
            matches!(Layout::parse(&bytes), Err(Error::Region { .. })),
            "field at {field}"
        );
    }

    // The title pointer list does: -1 means the ordering lives in an entry.
    let mut bytes = testutil::sample().legacy_title_index().build();
    bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    let layout = Layout::parse(&bytes).expect("the sentinel is not a defect");
    assert!(!layout.header().has_title_index());

    // Any other out-of-range position still is one.
    let mut bytes = testutil::sample().legacy_title_index().build();
    bytes[40..48].copy_from_slice(&(u64::MAX - 1).to_le_bytes());
    assert!(matches!(Layout::parse(&bytes), Err(Error::Region { .. })));
}

#[test]
fn truncation_at_every_length_is_handled() {
    let full = testutil::sample().compression(Compression::Zstd).build();
    for n in 0..full.len() {
        let bytes = &full[..n];
        let Ok(layout) = Layout::parse(bytes) else {
            continue;
        };
        let zim = Zim::new(bytes, &layout);
        for i in 0..zim.entry_count().min(64) {
            if let Ok(d) = zim.dirent(i)
                && let Target::Content { cluster, blob } = d.target
                && let Ok(c) = zim.cluster(cluster, LIMIT)
            {
                let _ = c.blob(blob);
            }
            let _ = zim.resolve(i);
        }
        if let Ok(Some(index)) = zim.title_index() {
            for p in 0..index.count().min(64) {
                let _ = zim.title_entry(&index, p);
            }
            let _ = zim.title_lower_bound(&index, b'C', b"M");
        }
        let _ = zim.find(b'C', b"index.html");
    }
}

#[test]
fn cluster_pointers_out_of_order_are_refused() {
    let bytes = testutil::sample().blobs_per_cluster(1).build();
    let layout = Layout::parse(&bytes).unwrap();
    let base = layout.header().cluster_ptr_pos as usize;
    let mut bytes = bytes.clone();
    let third = u64::from_le_bytes(bytes[base + 16..base + 24].try_into().unwrap());
    bytes[base + 8..base + 16].copy_from_slice(&(third + 1).to_le_bytes());
    let layout = Layout::parse(&bytes).unwrap();
    let zim = Zim::new(&bytes, &layout);
    assert_eq!(
        zim.cluster_raw(1),
        Err(Error::Cluster("cluster extent out of order"))
    );
}

#[test]
fn entry_and_cluster_indices_are_checked() {
    let bytes = testutil::sample().build();
    let layout = Layout::parse(&bytes).unwrap();
    let zim = Zim::new(&bytes, &layout);
    assert_eq!(zim.dirent(9999), Err(Error::EntryIndex(9999)));
    assert_eq!(zim.cluster_raw(9999), Err(Error::ClusterIndex(9999)));
    let index = zim.title_index().unwrap().expect("a title ordering");
    assert_eq!(zim.title_entry(&index, 9999), Err(Error::EntryIndex(9999)));
}

#[test]
fn a_decoder_that_panics_is_still_an_error() {
    // The committed seed carries an xz footer that overflows inside lzma-rs.
    // Found by fuzz target A; the parser must answer, not unwind.
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/seeds/archive/xz-crash.zim"),
    )
    .expect("crash seed");
    let layout = Layout::parse(&bytes).expect("the archive itself is well formed");
    let zim = Zim::new(&bytes, &layout);
    let mut errors = 0;
    for cluster in 0..zim.cluster_count() {
        if zim.cluster(cluster, LIMIT).is_err() {
            errors += 1;
        }
    }
    assert!(errors > 0, "the crafted cluster should fail to decode");
}

#[test]
fn a_modern_archive_orders_titles_from_the_listing_entry() {
    // What current libzim writes: the header field is a sentinel and the
    // ordering is the blob of X/listing/titleOrdered/v1.
    let bytes = Builder::new()
        .content("a.html", "Apple", 0, b"a")
        .content("b.html", "Banana", 0, b"b")
        .content("c.html", "Cherry", 0, b"c")
        .build();
    let layout = Layout::parse(&bytes).expect("opens");
    let zim = Zim::new(&bytes, &layout);

    assert!(
        !zim.header().has_title_index(),
        "the header carries a sentinel"
    );
    assert!(zim.find(b'X', zimfmt::TITLE_LISTING_V1).unwrap().is_some());

    let index = zim
        .title_index()
        .unwrap()
        .expect("ordering from the listing");
    assert_eq!(index.count(), 3, "front articles only");
    let titles: Vec<Vec<u8>> = (0..index.count())
        .map(|p| {
            zim.dirent(zim.title_entry(&index, p).unwrap())
                .unwrap()
                .effective_title()
                .to_vec()
        })
        .collect();
    assert_eq!(
        titles,
        [b"Apple".to_vec(), b"Banana".to_vec(), b"Cherry".to_vec()]
    );

    let start = zim.title_lower_bound(&index, b'C', b"B").unwrap();
    assert_eq!(
        zim.title_entry(&index, start).unwrap(),
        zim.find(b'C', b"b.html").unwrap().unwrap()
    );
}

#[test]
fn a_legacy_archive_orders_titles_from_the_header() {
    let bytes = Builder::new()
        .legacy_title_index()
        .version(5, 0)
        .content("a.html", "Apple", 0, b"a")
        .content("b.html", "Banana", 0, b"b")
        .build();
    let layout = Layout::parse(&bytes).expect("opens");
    let zim = Zim::new(&bytes, &layout);

    assert!(zim.header().has_title_index());
    assert!(zim.find(b'X', zimfmt::TITLE_LISTING_V1).unwrap().is_none());
    let index = zim
        .title_index()
        .unwrap()
        .expect("ordering from the header");
    assert_eq!(index.count(), zim.entry_count());
}

#[test]
fn a_sentinel_without_a_listing_leaves_no_ordering() {
    let mut bytes = Builder::new()
        .legacy_title_index()
        .content("a.html", "Apple", 0, b"a")
        .build();
    bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    let layout = Layout::parse(&bytes).expect("opens anyway");
    let zim = Zim::new(&bytes, &layout);
    assert_eq!(zim.title_index().unwrap(), None, "nothing to suggest from");
}
