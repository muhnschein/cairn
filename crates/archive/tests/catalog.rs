//! Catalog behaviour over crafted archives.

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
    std::fs::rename(dir.path().join("first-name.zim"), dir.path().join("second-name.zim"))
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
    dir.write("s.zim", &testutil::sample().compression(Compression::Zstd).build());
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
    assert_eq!(c.cache_stats().entries, 0, "plain clusters do not enter the cache");
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
    assert_eq!(c.entry(&uuid, "index.html").unwrap().blob.as_slice(), b"legacy");
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
    assert_eq!(c.entry(&uuid_of(&c), "I/logo.png").unwrap().blob.as_slice(), b"content");
}

#[test]
fn missing_things_are_distinguishable() {
    let dir = TempDir::new("missing");
    dir.write("s.zim", &testutil::sample().build());
    let c = catalog(&dir);
    let uuid = uuid_of(&c);
    assert!(matches!(c.entry(&uuid, "nope.html"), Err(LookupError::NoSuchEntry)));
    assert!(matches!(c.entry("not-a-uuid", "index.html"), Err(LookupError::NoSuchArchive)));
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
    assert_eq!(s.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Apple", "Apricot"]);
    assert_eq!(s[0].path, "a.html");
    assert_eq!(c.suggest(&uuid, "Ap", 1).unwrap().len(), 1, "limit is respected");
    assert!(c.suggest(&uuid, "ap", 10).unwrap().is_empty(), "prefix match is byte exact");
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
    assert!(m.text.iter().any(|(k, v)| k == "Title" && v == "Sample Archive"));
    assert!(m.text.iter().any(|(k, _)| k == "Description"));
    assert!(m.binary.iter().any(|k| k == "Illustration_48x48@1"));
}

#[test]
fn an_empty_directory_is_an_empty_catalog() {
    let dir = TempDir::new("empty");
    let c = catalog(&dir);
    assert!(c.archives().is_empty());
    assert!(matches!(c.summary("00000000-0000-0000-0000-000000000000"), Err(LookupError::NoSuchArchive)));
}
