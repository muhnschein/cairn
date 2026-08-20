//! Router behaviour against a stub catalog. No archive, no socket.

use std::sync::Arc;

use api::{
    ArchiveSummary, Catalog, CatalogError, EntryContent, Fault, Limits, Metadata, Method, Policy,
    Request, Response, Router, SharedBytes, Status, Suggestion,
};

const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";

struct Stub;

fn shared(bytes: &[u8]) -> SharedBytes {
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes.to_vec());
    let len = bytes.len();
    SharedBytes::new(data, 0, len)
}

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
            title: "Stub Archive".to_owned(),
            entry_count: 3,
            cluster_count: 1,
            main_page: Some("index.html".to_owned()),
            major_version: 6,
            minor_version: 1,
            content_namespace: 'C',
        })
    }

    fn metadata(&self, _uuid: &str) -> Result<Metadata, CatalogError> {
        Ok(Metadata {
            text: vec![("Title".into(), "Stub Archive".into())],
            binary: vec!["Illustration_48x48@1".into()],
        })
    }

    fn entry(&self, uuid: &str, path: &str) -> Result<EntryContent, CatalogError> {
        if uuid != UUID {
            return Err(CatalogError::NoSuchArchive);
        }
        match path {
            "index.html" => Ok(EntryContent {
                path: "index.html".into(),
                mime: "text/html".into(),
                body: shared(b"0123456789"),
            }),
            "evil.html" => Ok(EntryContent {
                // Both hostile inputs at once: the archive supplies a header
                // value and a path, the client supplies the request.
                path: "evil\r\nX-Injected: yes".into(),
                mime: "text/html\r\nX-Injected: yes".into(),
                body: shared(b"hi"),
            }),
            "empty.txt" => Ok(EntryContent {
                path: "empty.txt".into(),
                mime: "text/plain".into(),
                body: shared(b""),
            }),
            "broken.html" => Err(CatalogError::Corrupt),
            _ => Err(CatalogError::NoSuchEntry),
        }
    }

    fn random(&self, uuid: &str, pick: u64) -> Result<String, CatalogError> {
        if uuid != UUID {
            return Err(CatalogError::NoSuchArchive);
        }
        Ok(format!("random-{}.html", pick % 4))
    }

    fn suggest(
        &self,
        uuid: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>, CatalogError> {
        if uuid != UUID {
            return Err(CatalogError::NoSuchArchive);
        }
        Ok(["Apple", "Apricot", "Avocado"]
            .iter()
            .filter(|t| t.starts_with(prefix))
            .take(limit)
            .map(|t| Suggestion { title: (*t).to_owned(), path: format!("{}.html", t.to_lowercase()) })
            .collect())
    }
}

fn router(auth: Option<&str>) -> Router {
    Router::new(
        Arc::new(Stub),
        Limits::default(),
        Policy { auth_token: auth.map(str::to_owned), ..Policy::default() },
        Box::new(|| Status {
            version: "0.1.0".into(),
            uptime_seconds: 7,
            listener: "unix:/run/cairn/cairn.sock".into(),
            archive_count: 1,
            auth_required: false,
            sandbox: api::Sandbox {
                required: true,
                layers: vec![api::Layer {
                    name: "seccomp".into(),
                    state: "applied".into(),
                    detail: Some("kill".into()),
                }],
            },
            ..Status::default()
        }),
        42,
    )
}

fn get(r: &Router, target: &str) -> Response {
    request(r, "GET", target, &[])
}

fn request(r: &Router, method: &str, target: &str, headers: &[(&str, &str)]) -> Response {
    let mut raw = format!("{method} {target} HTTP/1.1\r\nHost: cairn\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("\r\n");
    let (req, _) = Request::parse(raw.as_bytes(), &Limits::default()).expect("parse");
    r.handle(&req)
}

fn body(r: &Response) -> String {
    String::from_utf8_lossy(r.payload.as_slice()).into_owned()
}

fn header<'a>(r: &'a Response, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

#[test]
fn status_reports_the_sandbox() {
    let r = get(&router(None), "/v1/status");
    assert_eq!(r.status, 200);
    let b = body(&r);
    assert!(b.contains(r#""version":"0.1.0""#), "{b}");
    assert!(b.contains(r#""name":"seccomp","state":"applied","detail":"kill""#), "{b}");
    assert!(b.contains(r#""required":true"#), "{b}");
    assert!(b.contains(r#""limits":{"#), "{b}");
}

#[test]
fn lists_archives() {
    let r = get(&router(None), "/v1/archives");
    assert_eq!(r.status, 200);
    let b = body(&r);
    assert!(b.contains(&format!(r#""uuid":"{UUID}""#)), "{b}");
    assert!(b.contains(r#""format_version":"6.1""#), "{b}");
    assert!(b.contains(r#""main_page":"index.html""#), "{b}");
}

#[test]
fn archive_detail_includes_metadata() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}"));
    assert_eq!(r.status, 200);
    let b = body(&r);
    assert!(b.contains(r#""metadata":{"Title":"Stub Archive"}"#), "{b}");
    assert!(b.contains(r#""binary_metadata":["Illustration_48x48@1"]"#), "{b}");
}

#[test]
fn serves_entry_content_with_the_documented_headers() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}/entry/index.html"));
    assert_eq!(r.status, 200);
    assert_eq!(body(&r), "0123456789");
    assert_eq!(header(&r, "content-type"), Some("text/html"));
    assert_eq!(header(&r, "x-cairn-archive"), Some(UUID));
    assert_eq!(header(&r, "x-cairn-path"), Some("index.html"));
    assert_eq!(header(&r, "accept-ranges"), Some("bytes"));
    assert_eq!(header(&r, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&r, "cross-origin-resource-policy"), Some("same-origin"));
    assert_eq!(header(&r, "content-security-policy"), Some("default-src 'none'; sandbox"));
}

#[test]
fn archive_data_cannot_inject_a_header() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}/entry/evil.html"));
    assert_eq!(r.status, 200);
    assert_eq!(header(&r, "content-type"), Some("application/octet-stream"));
    assert_eq!(header(&r, "x-cairn-path"), Some("evil%0D%0AX-Injected:%20yes"));
    let head = String::from_utf8(r.head_bytes()).unwrap();
    // The CRLF is encoded, so the payload stays inside one header value and
    // no new header line appears.
    for line in head.split("\r\n").skip(1).filter(|l| !l.is_empty()) {
        assert!(!line.starts_with("X-Injected"), "injected header: {line:?}");
        assert!(line.contains(": "), "stray line {line:?}");
    }
}

#[test]
fn head_keeps_the_length_and_drops_the_body() {
    let r = request(&router(None), "HEAD", &format!("/v1/archives/{UUID}/entry/index.html"), &[]);
    assert_eq!(r.status, 200);
    assert!(!r.send_body);
    assert_eq!(r.payload.len(), 10);
    assert!(String::from_utf8(r.head_bytes()).unwrap().contains("Content-Length: 10"));
}

#[test]
fn single_ranges_are_served() {
    let target = format!("/v1/archives/{UUID}/entry/index.html");
    let r = request(&router(None), "GET", &target, &[("Range", "bytes=2-4")]);
    assert_eq!(r.status, 206);
    assert_eq!(body(&r), "234");
    assert_eq!(header(&r, "content-range"), Some("bytes 2-4/10"));

    let r = request(&router(None), "GET", &target, &[("Range", "bytes=-3")]);
    assert_eq!(r.status, 206);
    assert_eq!(body(&r), "789");
}

#[test]
fn multipart_and_unsatisfiable_ranges_are_refused() {
    let target = format!("/v1/archives/{UUID}/entry/index.html");
    for value in ["bytes=0-1,4-5", "bytes=50-60"] {
        let r = request(&router(None), "GET", &target, &[("Range", value)]);
        assert_eq!(r.status, 416, "{value}");
        assert_eq!(header(&r, "content-range"), Some("bytes */10"));
        assert!(body(&r).contains("range_not_satisfiable"));
    }
}

#[test]
fn empty_entries_are_served() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}/entry/empty.txt"));
    assert_eq!(r.status, 200);
    assert_eq!(r.payload.len(), 0);
}

#[test]
fn a_malformed_archive_is_503_not_500() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}/entry/broken.html"));
    assert_eq!(r.status, 503);
    assert!(body(&r).contains("archive_unavailable"));
}

#[test]
fn missing_things_are_404() {
    let missing = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    for target in [
        format!("/v1/archives/{UUID}/entry/nope.html"),
        format!("/v1/archives/{missing}/entry/index.html"),
        format!("/v1/archives/{missing}"),
        "/v1/nothing".to_owned(),
        "/".to_owned(),
        format!("/v1/archives/{UUID}/"),
        format!("/v1/archives/{UUID}/entry/"),
        format!("/v1/archives/{UUID}/unknown"),
    ] {
        let r = get(&router(None), &target);
        assert_eq!(r.status, 404, "{target}");
        assert!(body(&r).contains("not_found"), "{target}");
    }
}

#[test]
fn non_canonical_uuids_are_400() {
    for uuid in [
        "01234567-89AB-CDEF-0123-456789ABCDEF",
        "0123456789abcdef0123456789abcdef",
        "{01234567-89ab-cdef-0123-456789abcdef}",
        "%3001234567-89ab-cdef-0123-456789abcde",
    ] {
        let r = get(&router(None), &format!("/v1/archives/{uuid}"));
        assert_eq!(r.status, 400, "{uuid}");
        assert!(body(&r).contains("bad_uuid"), "{uuid}");
    }
}

#[test]
fn other_methods_are_405() {
    let r = request(&router(None), "DELETE", "/v1/status", &[]);
    assert_eq!(r.status, 405);
    assert_eq!(header(&r, "allow"), Some("GET, HEAD"));
}

#[test]
fn bad_paths_are_rejected_before_lookup() {
    let base = format!("/v1/archives/{UUID}/entry");
    for path in ["%C0%AF", "%00", "%zz", "%2"] {
        let r = get(&router(None), &format!("{base}/{path}"));
        assert_eq!(r.status, 400, "{path}");
        assert!(body(&r).contains("bad_path"), "{path}");
    }
    let long = "a".repeat(2000);
    let r = get(&router(None), &format!("{base}/{long}"));
    assert_eq!(r.status, 414);
}

#[test]
fn suggestions_are_bounded() {
    let base = format!("/v1/archives/{UUID}/suggest");
    let r = get(&router(None), &format!("{base}?q=Ap"));
    assert_eq!(r.status, 200);
    let b = body(&r);
    assert!(b.contains(r#""title":"Apple","path":"apple.html""#), "{b}");
    assert!(b.contains("Apricot"), "{b}");
    assert!(!b.contains("Avocado"), "{b}");

    let r = get(&router(None), &format!("{base}?q=A&limit=1"));
    assert_eq!(body(&r).matches("title").count(), 1);

    // Missing q, oversized q, and junk limits are all refused.
    for target in [
        base.clone(),
        format!("{base}?limit=2"),
        format!("{base}?q={}", "a".repeat(200)),
        format!("{base}?q=a&limit=x"),
        format!("{base}?q=%zz"),
    ] {
        let r = get(&router(None), &target);
        assert_eq!(r.status, 400, "{target}");
        assert!(body(&r).contains("bad_query"), "{target}");
    }
}

#[test]
fn random_returns_a_path() {
    let r = get(&router(None), &format!("/v1/archives/{UUID}/random"));
    assert_eq!(r.status, 200);
    assert!(body(&r).contains(r#""path":"random-"#));
}

#[test]
fn auth_is_checked_before_routing() {
    let r = router(Some("s3cret"));
    // Without a token nothing is distinguishable: not the archive list, not a
    // missing archive, not a missing entry.
    for target in [
        "/v1/status",
        "/v1/archives",
        "/v1/archives/ffffffff-ffff-ffff-ffff-ffffffffffff",
        "/v1/nothing",
    ] {
        let resp = get(&r, target);
        assert_eq!(resp.status, 401, "{target}");
        assert_eq!(header(&resp, "www-authenticate"), Some("Bearer"));
    }

    let ok = request(&r, "GET", "/v1/status", &[("Authorization", "Bearer s3cret")]);
    assert_eq!(ok.status, 200);

    for value in ["Bearer wrong", "Bearer s3cre", "Bearer s3cretx", "s3cret", "Basic s3cret"] {
        let resp = request(&r, "GET", "/v1/status", &[("Authorization", value)]);
        assert_eq!(resp.status, 401, "{value}");
    }
}

#[test]
fn error_bodies_never_echo_the_request() {
    let r = get(&router(None), "/v1/archives/not-a-uuid-at-all-really-nope-x/entry/secret");
    assert_eq!(r.status, 400);
    let b = body(&r);
    assert!(!b.contains("secret"), "{b}");
    assert!(!b.contains("not-a-uuid"), "{b}");
    assert_eq!(b, String::from_utf8(Fault::BadUuid.body()).unwrap());
}

#[test]
fn head_of_every_response_is_well_formed() {
    let targets = [
        "/v1/status".to_owned(),
        "/v1/archives".to_owned(),
        format!("/v1/archives/{UUID}"),
        format!("/v1/archives/{UUID}/entry/index.html"),
        format!("/v1/archives/{UUID}/entry/evil.html"),
        format!("/v1/archives/{UUID}/random"),
        format!("/v1/archives/{UUID}/suggest?q=A"),
        "/v1/missing".to_owned(),
    ];
    for target in targets {
        let r = get(&router(None), &target);
        let head = String::from_utf8(r.head_bytes()).expect("head is ascii");
        assert!(head.starts_with("HTTP/1.1 "), "{target}");
        assert!(head.ends_with("\r\n\r\n"), "{target}");
        assert!(head.contains("Content-Length: "), "{target}");
        assert!(!head[9..].contains("HTTP/1.1"), "{target}: header injection");
    }
}

#[test]
fn method_is_carried_from_the_request() {
    let (req, _) = Request::parse(
        b"HEAD /v1/status HTTP/1.1\r\nHost: c\r\n\r\n",
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(req.method, Method::Head);
    let r = router(None).handle(&req);
    assert!(!r.send_body);
}
