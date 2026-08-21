//! Catalog behaviour over crafted archives.

// a panic in a test is the failure report.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use archive::{Catalog, Limits, LookupError};
use testutil::{Builder, Compression, TempDir};

fn catalog(dir: &TempDir) -> Catalog {
    Catalog::open_dir(dir.path(), Limits::default()).expect("open catalog")
}

fn uuid_of(c: &Catalog) -> String {
    c.archives()[0].uuid().to_string()
}

#[test]
fn opens_a_directory_and_reports_identity() {
    let dir = TempDir::new("open");
    dir.write("sample.zim", &testutil::sample().build());
    dir.write("notes.txt", b"not an archive");
    let c = catalog(&dir);

    assert_eq!(c.archives().len(), 1);
    let s = c.archives()[0].summary();
    assert_eq!(s.title, "Sample Archive");
    assert_eq!(s.main_page.as_deref(), Some("index.html"));
    assert_eq!(s.content_namespace, 'C');
    // The UUID, not the filename, is the identity.
    assert_eq!(s.uuid.to_string(), uuid_of(&c));
}

#[test]
fn duplicate_uuid_names_both_files() {
    let dir = TempDir::new("dup");
    dir.write("a.zim", &testutil::sample().build());
    dir.write("b.zim", &testutil::sample().build());
    let err = Catalog::open_dir(dir.path(), Limits::default()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("a.zim") && msg.contains("b.zim"), "{msg}");
}

#[test]
fn identity_survives_a_rename() {
    let dir = TempDir::new("rename");
    dir.write("first-name.zim", &testutil::sample().build());
    let before = uuid_of(&catalog(&dir));
    std::fs::rename(
        dir.path().join("first-name.zim"),
        dir.path().join("second-name.zim"),
    )
    .unwrap();
    assert_eq!(before, uuid_of(&catalog(&dir)));
}

#[test]
fn serves_entries_from_every_compression() {
    for comp in [Compression::None, Compression::Xz, Compression::Zstd] {
        let dir = TempDir::new("entries");
        dir.write("s.zim", &testutil::sample().compression(comp).build());
        let c = catalog(&dir);
        let uuid = uuid_of(&c);

        let e = c.entry(&uuid, "index.html").expect("index");
        assert_eq!(e.blob.as_slice(), b"<html><body>index</body></html>");
        assert_eq!(e.mime, "text/html");
        assert_eq!(e.path, "index.html");

        let e = c.entry(&uuid, "logo.png").expect("logo");
        assert_eq!(e.mime, "image/png");
        assert_eq!(e.blob.len(), 8);
    }
}

#[test]
fn redirects_report_the_resolved_path() {
    let dir = TempDir::new("redirect");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    let e = c.entry(&uuid_of(&c), "home.html").unwrap();
    assert_eq!(e.path, "index.html");
    assert_eq!(e.blob.as_slice(), b"<html><body>index</body></html>");
}

#[test]
fn cluster_cache_serves_the_second_read() {
    let dir = TempDir::new("cache");
    dir.write(
        "s.zim",
        &testutil::sample().compression(Compression::Zstd).build(),
    );
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    c.entry(&uuid, "index.html").unwrap();
    let after_first = c.cache_stats();
    c.entry(&uuid, "notes.txt").unwrap();
    let after_second = c.cache_stats();
    assert_eq!(after_first.misses, 1);
    assert_eq!(after_second.hits, 1, "second entry shares the cluster");
    assert_eq!(after_second.misses, 1);
}

#[test]
fn uncompressed_entries_are_served_from_the_mapping() {
    let dir = TempDir::new("mapped");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    c.entry(&uuid_of(&c), "index.html").unwrap();
    assert_eq!(
        c.cache_stats().entries,
        0,
        "plain clusters do not enter the cache"
    );
}

#[test]
fn legacy_namespace_paths_resolve() {
    let dir = TempDir::new("legacy");
    dir.write(
        "s.zim",
        &Builder::new()
            .version(5, 0)
            .content("index.html", "Main", 0, b"legacy")
            .content_in(b'I', "logo.png", "Logo", 0, b"png")
            .build(),
    );
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    assert_eq!(
        c.entry(&uuid, "index.html").unwrap().blob.as_slice(),
        b"legacy"
    );
    let e = c.entry(&uuid, "I/logo.png").unwrap();
    assert_eq!(e.blob.as_slice(), b"png");
    assert_eq!(e.path, "I/logo.png");
}

#[test]
fn content_namespace_wins_over_an_explicit_prefix() {
    let dir = TempDir::new("shadow");
    dir.write(
        "s.zim",
        &Builder::new()
            .content("I/logo.png", "Content entry", 0, b"content")
            .content_in(b'I', "logo.png", "Namespace entry", 0, b"namespace")
            .build(),
    );
    let c = catalog(&dir);
    assert_eq!(
        c.entry(&uuid_of(&c), "I/logo.png").unwrap().blob.as_slice(),
        b"content"
    );
}

#[test]
fn missing_things_are_distinguishable() {
    let dir = TempDir::new("missing");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    assert!(matches!(
        c.entry(&uuid, "nope.html"),
        Err(LookupError::NoSuchEntry)
    ));
    assert!(matches!(
        c.entry("not-a-uuid", "index.html"),
        Err(LookupError::NoSuchArchive)
    ));
    assert!(matches!(
        c.entry("00000000-0000-0000-0000-000000000000", "index.html"),
        Err(LookupError::NoSuchArchive)
    ));
}

#[test]
fn suggestions_are_title_prefix_and_bounded() {
    let dir = TempDir::new("suggest");
    dir.write(
        "s.zim",
        &Builder::new()
            .content("a.html", "Apple", 0, b"a")
            .content("b.html", "Apricot", 0, b"b")
            .content("c.html", "Banana", 0, b"c")
            .build(),
    );
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    let s = c.suggest(&uuid, "Ap", 10).unwrap();
    assert_eq!(
        s.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
        ["Apple", "Apricot"]
    );
    assert_eq!(s[0].path, "a.html");
    assert_eq!(
        c.suggest(&uuid, "Ap", 1).unwrap().len(),
        1,
        "limit is respected"
    );
    assert!(
        c.suggest(&uuid, "ap", 10).unwrap().is_empty(),
        "prefix match is byte exact"
    );
    assert!(c.suggest(&uuid, "", 10).unwrap().len() == 3);
}

#[test]
fn random_stays_inside_the_content_namespace() {
    let dir = TempDir::new("random");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    let content = ["index.html", "logo.png", "notes.txt"];
    for seed in 0..64u64 {
        let p = c.random(&uuid, seed).unwrap();
        assert!(content.contains(&p.as_str()), "random returned {p}");
    }
}

#[test]
fn metadata_splits_text_from_binary() {
    let dir = TempDir::new("meta");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    let m = c.metadata(&uuid_of(&c)).unwrap();
    assert!(
        m.text
            .iter()
            .any(|(k, v)| k == "Title" && v == "Sample Archive")
    );
    assert!(m.text.iter().any(|(k, _)| k == "Description"));
    assert!(m.binary.iter().any(|k| k == "Illustration_48x48@1"));
}

#[test]
fn an_empty_directory_is_an_empty_catalog() {
    let dir = TempDir::new("empty");
    let c = catalog(&dir);
    assert!(c.archives().is_empty());
    assert!(matches!(
        c.summary("00000000-0000-0000-0000-000000000000"),
        Err(LookupError::NoSuchArchive)
    ));
}

#[test]
fn suggestions_come_from_the_listing_entry_in_a_modern_archive() {
    // The default builder layout is what current libzim writes: a sentinel in
    // the header, the ordering in X/listing/titleOrdered/v1.
    let dir = TempDir::new("modern");
    dir.write(
        "s.zim",
        &Builder::new()
            .compression(Compression::Zstd)
            .content("a.html", "Apple", 0, b"a")
            .content("b.html", "Apricot", 0, b"b")
            .content("c.html", "Banana", 0, b"c")
            .build(),
    );
    let c = catalog(&dir);
    let uuid = uuid_of(&c);

    assert!(c.archives()[0].summary().has_title_index);
    let s = c.suggest(&uuid, "Ap", 10).unwrap();
    assert_eq!(
        s.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
        ["Apple", "Apricot"]
    );
    assert_eq!(s[0].path, "a.html");
}

#[test]
fn an_archive_without_a_title_ordering_says_so_and_suggests_nothing() {
    let mut bytes = Builder::new()
        .legacy_title_index()
        .content("a.html", "Apple", 0, b"a")
        .build();
    // The sentinel with no listing entry: legal, and nothing to order by.
    bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes());

    let dir = TempDir::new("no-titles");
    dir.write("s.zim", &bytes);
    let c = catalog(&dir);
    let uuid = uuid_of(&c);

    assert!(!c.archives()[0].summary().has_title_index);
    assert!(c.suggest(&uuid, "A", 10).unwrap().is_empty());
    // Everything else still works.
    assert_eq!(c.entry(&uuid, "a.html").unwrap().blob.as_slice(), b"a");
}

#[test]
fn a_redirect_chain_is_abandoned_rather_than_walked() {
    // SCOPE §5.1: chains are followed to a fixed small depth. A chain longer
    // than that is a crafted archive, not a deep site.
    let hops = usize::try_from(zimfmt::MAX_REDIRECT_HOPS).unwrap();
    let mut b = Builder::new().content("end.html", "End", 0, b"arrived");
    // r0 -> r1 -> ... -> r{n} -> end.html, one hop longer than allowed.
    for i in 0..=hops {
        let to = if i == hops {
            "end.html".to_owned()
        } else {
            format!("r{}.html", i + 1)
        };
        b = b.redirect(&format!("r{i}.html"), &format!("R{i}"), &to);
    }
    let dir = TempDir::new("redirect-depth");
    dir.write("chain.zim", &b.build());
    let c = catalog(&dir);
    let uuid = uuid_of(&c);

    // The far end of the chain is still reachable from close enough in.
    let near = c.entry(&uuid, &format!("r{hops}.html")).expect("one hop");
    assert_eq!(near.path, "end.html");

    match c.entry(&uuid, "r0.html") {
        Err(LookupError::Corrupt(_)) => {}
        other => panic!("a chain past the limit must be refused, got {other:?}"),
    }
}

#[test]
fn a_redirect_to_itself_does_not_loop() {
    let dir = TempDir::new("redirect-self");
    dir.write(
        "loop.zim",
        &Builder::new()
            .content("real.html", "Real", 0, b"x")
            .redirect("self.html", "Self", "self.html")
            .build(),
    );
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    match c.entry(&uuid, "self.html") {
        Err(LookupError::Corrupt(_)) => {}
        other => panic!("a self-redirect must be refused, got {other:?}"),
    }
    // The rest of the archive still serves.
    assert_eq!(c.entry(&uuid, "real.html").expect("real").path, "real.html");
}

#[test]
fn metadata_stops_at_the_entry_limit() {
    let mut b = Builder::new().content("index.html", "Index", 0, b"x");
    for i in 0..12 {
        b = b.content_in(b'M', &format!("Key{i:02}"), "", 2, b"value");
    }
    let dir = TempDir::new("meta-count");
    dir.write("many.zim", &b.build());

    let limits = Limits {
        max_metadata_entries: 5,
        ..Limits::default()
    };
    let c = Catalog::open_dir(dir.path(), limits).expect("open");
    let uuid = c.archives()[0].uuid().to_string();
    let m = c.metadata(&uuid).expect("metadata");
    assert_eq!(
        m.text.len() + m.binary.len(),
        5,
        "a scan bounded by count, not by what the archive declares"
    );
}

#[test]
fn a_metadata_value_over_the_byte_limit_is_reported_as_binary() {
    // Not dropped: the operator should see the key exists. Not decoded into
    // memory either, which is the point of the bound.
    let dir = TempDir::new("meta-bytes");
    dir.write(
        "big.zim",
        &Builder::new()
            .content("index.html", "Index", 0, b"x")
            .content_in(b'M', "Title", "", 2, b"Small")
            .content_in(b'M', "Long", "", 2, &vec![b'a'; 4096])
            .build(),
    );
    let limits = Limits {
        max_metadata_bytes: 64,
        ..Limits::default()
    };
    let c = Catalog::open_dir(dir.path(), limits).expect("open");
    let uuid = c.archives()[0].uuid().to_string();
    let m = c.metadata(&uuid).expect("metadata");

    assert!(m.text.iter().any(|(k, v)| k == "Title" && v == "Small"));
    assert!(m.binary.iter().any(|k| k == "Long"), "{m:?}");
    assert!(!m.text.iter().any(|(k, _)| k == "Long"), "{m:?}");
}

#[test]
fn an_archive_with_a_nil_uuid_is_refused_at_open() {
    // SCOPE §5.3: identity is the uuid, so an archive without one has no id
    // the API could address it by.
    let dir = TempDir::new("nil-uuid");
    dir.write("nil.zim", &testutil::sample().uuid([0u8; 16]).build());
    let err = Catalog::open_dir(dir.path(), Limits::default()).unwrap_err();
    assert!(err.to_string().contains("nil.zim"), "{err}");
}

#[test]
fn a_file_that_is_not_an_archive_is_refused_by_name() {
    let dir = TempDir::new("not-zim");
    dir.write(
        "broken.zim",
        b"this is not a ZIM file, but it is named like one",
    );
    let err = Catalog::open_dir(dir.path(), Limits::default()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("broken.zim"), "{msg}");
}

#[test]
fn a_truncation_that_cuts_a_declared_region_is_refused_at_open() {
    // The header and the pointer lists are checked against the file length
    // when the archive is opened, so this never reaches a request.
    let full = testutil::sample().build();
    let dir = TempDir::new("truncated-head");
    dir.write("cut.zim", &full[..64]);
    let err = Catalog::open_dir(dir.path(), Limits::default()).unwrap_err();
    assert!(err.to_string().contains("cut.zim"), "{err}");
}

#[test]
fn a_truncation_that_only_cuts_cluster_bytes_never_yields_wrong_bytes() {
    // Cluster contents are checked when read, not at open, so this archive
    // opens. What matters is that no entry comes back with the wrong bytes:
    // every read either returns exactly what was stored, or fails.
    let full = testutil::sample().build();
    let whole = TempDir::new("truncated-whole");
    whole.write("full.zim", &full);
    let reference = catalog(&whole);
    let ref_uuid = uuid_of(&reference);
    let paths = ["index.html", "logo.png", "notes.txt"];
    let expected: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| {
            let e = reference.entry(&ref_uuid, p).expect("reference entry");
            e.blob.as_slice().to_vec()
        })
        .collect();

    for cut in [1, 8, 64] {
        let dir = TempDir::new(&format!("truncated-tail-{cut}"));
        dir.write("cut.zim", &full[..full.len() - cut]);
        let Ok(c) = Catalog::open_dir(dir.path(), Limits::default()) else {
            continue; // refused at open is also a correct answer
        };
        let uuid = c.archives()[0].uuid().to_string();
        for (path, want) in paths.iter().zip(&expected) {
            match c.entry(&uuid, path) {
                Ok(e) => assert_eq!(
                    e.blob.as_slice(),
                    &want[..],
                    "{path} served the wrong bytes after a {cut}-byte truncation"
                ),
                Err(LookupError::Corrupt(_) | LookupError::NoSuchEntry) => {}
                Err(other) => panic!("{path}: unexpected {other:?}"),
            }
        }
    }
}
