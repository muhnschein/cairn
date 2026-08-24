//! `make smoke`: a real daemon and the real CLI over a crafted archive.

// a panic in a test is the failure report.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::{Daemon, SAMPLE_UUID, ZSTD_UUID, parse_replies, parse_reply};

fn entry(uuid: &str, path: &str) -> String {
    format!("/v1/archives/{uuid}/entry/{path}")
}

#[test]
fn status_reports_what_was_applied() {
    let d = Daemon::start("smoke-status", "sandbox = best-effort\n");
    let r = d.get("/v1/status");
    assert_eq!(r.status, 200);
    let body = r.text();
    assert!(body.contains(r#""version":"#), "{body}");
    assert!(body.contains(r#""name":"seccomp""#), "{body}");
    assert!(body.contains(r#""name":"landlock""#), "{body}");
    assert!(body.contains(r#""name":"no_new_privs""#), "{body}");
    assert!(body.contains(r#""listener":"unix:"#), "{body}");
    assert_eq!(r.header("content-type"), Some("application/json"));
}

#[test]
fn serves_every_documented_endpoint() {
    let d = Daemon::with_archives(
        "smoke-endpoints",
        "",
        &[
            ("plain.zim", testutil::sample().build()),
            (
                "zstd.zim",
                testutil::sample()
                    .uuid(*b"cairn-test-zstd1")
                    .compression(testutil::Compression::Zstd)
                    .build(),
            ),
        ],
    );

    let r = d.get("/v1/archives");
    assert_eq!(r.status, 200);
    assert!(r.text().contains(SAMPLE_UUID));
    assert!(r.text().contains(ZSTD_UUID));

    let r = d.get(&format!("/v1/archives/{SAMPLE_UUID}"));
    assert_eq!(r.status, 200);
    assert!(
        r.text().contains(r#""Title":"Sample Archive""#),
        "{}",
        r.text()
    );

    let r = d.get(&entry(SAMPLE_UUID, "index.html"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"<html><body>index</body></html>");
    assert_eq!(r.header("content-type"), Some("text/html"));
    assert_eq!(r.header("x-cairn-archive"), Some(SAMPLE_UUID));
    assert_eq!(r.header("x-cairn-path"), Some("index.html"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        r.header("cross-origin-resource-policy"),
        Some("same-origin")
    );
    assert!(r.header("content-security-policy").is_some());

    // A redirect resolves and says what it resolved to.
    let r = d.get(&entry(SAMPLE_UUID, "home.html"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-cairn-path"), Some("index.html"));

    // Compressed clusters come back byte-identical.
    let r = d.get(&entry(ZSTD_UUID, "notes.txt"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"plain notes");

    let r = d.request("HEAD", &entry(SAMPLE_UUID, "index.html"), "");
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    assert!(r.head.contains("Content-Length: 31"), "{}", r.head);

    let r = d.get(&format!("/v1/archives/{SAMPLE_UUID}/suggest?q=Ma"));
    assert_eq!(r.status, 200);
    assert!(r.text().contains(r#""title":"Main Page""#), "{}", r.text());

    // The listing entry is what a modern archive orders titles by, and the
    // listing says whether an archive can answer at all.
    let r = d.get("/v1/archives");
    assert!(r.text().contains(r#""suggest":true"#), "{}", r.text());

    let r = d.get(&format!("/v1/archives/{SAMPLE_UUID}/random"));
    assert_eq!(r.status, 200);
    assert!(r.text().contains(r#""path":"#));
}

#[test]
fn ranges_are_served_and_bounded() {
    let d = Daemon::start("smoke-range", "");
    let target = entry(SAMPLE_UUID, "index.html");

    let r = d.request("GET", &target, "Range: bytes=0-5\r\n");
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"<html>");
    assert_eq!(r.header("content-range"), Some("bytes 0-5/31"));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));

    let r = d.request("GET", &target, "Range: bytes=-4\r\n");
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"tml>", "suffix range");
    assert_eq!(r.header("content-range"), Some("bytes 27-30/31"));

    let r = d.request("GET", &target, "Range: bytes=100-200\r\n");
    assert_eq!(r.status, 416);
}

#[test]
fn multipart_ranges_are_refused() {
    let d = Daemon::start("smoke-multirange", "");
    let r = d.request(
        "GET",
        &entry(SAMPLE_UUID, "index.html"),
        "Range: bytes=0-1,4-5\r\n",
    );
    assert_eq!(r.status, 416);
    assert_eq!(r.header("content-range"), Some("bytes */31"));
    assert!(r.text().contains("range_not_satisfiable"));
}

#[test]
fn missing_and_malformed_requests() {
    let d = Daemon::start("smoke-errors", "");

    assert_eq!(d.get(&entry(SAMPLE_UUID, "nope.html")).status, 404);
    assert_eq!(
        d.get("/v1/archives/ffffffff-ffff-ffff-ffff-ffffffffffff")
            .status,
        404
    );
    assert_eq!(d.get("/v1/nothing").status, 404);
    assert_eq!(d.get("/").status, 404);
    assert_eq!(d.get("/v1/archives/NOT-CANONICAL").status, 400);
    assert_eq!(d.request("DELETE", "/v1/status", "").status, 405);
    assert_eq!(d.get(&entry(SAMPLE_UUID, "%00")).status, 400);
    assert_eq!(d.get(&entry(SAMPLE_UUID, "%C0%AF")).status, 400);
    assert_eq!(
        d.get(&format!("/v1/archives/{SAMPLE_UUID}/suggest")).status,
        400
    );

    // Raw protocol errors, each on its own connection.
    let cases: [(&[u8], u16); 5] = [
        (b"GET / HTTP/1.0\r\nHost: c\r\n\r\n", 505),
        (b"GET / HTTP/1.1\r\n\r\n", 400),
        (
            b"GET / HTTP/1.1\r\nHost: c\r\nContent-Length: 9\r\n\r\nbody-here",
            400,
        ),
        (b"GET / HTTP/1.1\nHost: c\r\n\r\n", 400),
        (b"GET http://elsewhere/ HTTP/1.1\r\nHost: c\r\n\r\n", 400),
    ];
    for (raw, expected) in cases {
        let bytes = d.raw(raw).expect("raw request");
        let reply = common::parse_reply(&bytes).expect("a response");
        assert_eq!(
            reply.status,
            expected,
            "for {:?}",
            String::from_utf8_lossy(raw)
        );
        assert!(reply.text().contains(r#""error""#));
    }
    assert!(
        d.get("/v1/status").status == 200,
        "daemon survived every malformed request"
    );
}

#[test]
fn oversized_headers_are_refused() {
    let d = Daemon::start(
        "smoke-big",
        "max_header_bytes = 2K\nmax_request_line = 1K\n",
    );

    let long_target = format!("/{}", "a".repeat(2000));
    let raw = format!("GET {long_target} HTTP/1.1\r\nHost: c\r\n\r\n");
    let reply = common::parse_reply(&d.raw(raw.as_bytes()).unwrap()).unwrap();
    assert_eq!(reply.status, 414);

    let raw = format!(
        "GET / HTTP/1.1\r\nHost: c\r\nX: {}\r\n\r\n",
        "a".repeat(4000)
    );
    let reply = common::parse_reply(&d.raw(raw.as_bytes()).unwrap()).unwrap();
    assert_eq!(reply.status, 431);

    assert_eq!(d.get("/v1/status").status, 200);
}

#[test]
fn keep_alive_and_pipelining() {
    let d = Daemon::start("smoke-keepalive", "");
    let one = format!(
        "GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\nGET {} HTTP/1.1\r\nHost: c\r\n\r\nGET /v1/archives HTTP/1.1\r\nHost: c\r\nConnection: close\r\n\r\n",
        entry(SAMPLE_UUID, "index.html")
    );
    let bytes = d.raw(one.as_bytes()).expect("pipelined");
    let replies = parse_replies(&bytes);
    assert_eq!(replies.len(), 3, "three responses on one connection");
    assert_eq!(replies[0].status, 200);
    assert_eq!(replies[1].body, b"<html><body>index</body></html>");
    assert!(replies[2].head.contains("Connection: close"));
    assert!(replies[0].head.contains("Connection: keep-alive"));
}

#[test]
fn the_request_rate_ceiling_applies_per_connection() {
    let d = Daemon::start("smoke-rate", "request_rate = 0.001\nrequest_burst = 2\n");
    let mut raw = String::new();
    for _ in 0..4 {
        raw.push_str("GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\n");
    }
    let replies = parse_replies(&d.raw(raw.as_bytes()).unwrap());
    assert_eq!(replies[0].status, 200);
    assert_eq!(replies[1].status, 200);
    assert_eq!(replies[2].status, 429, "burst spent");
    assert!(replies[2].head.contains("Connection: close"));

    // A fresh connection gets a fresh bucket, and the daemon is unharmed.
    assert_eq!(d.get("/v1/status").status, 200);
}

#[test]
fn keepalive_request_ceiling_closes_the_connection() {
    let d = Daemon::start("smoke-kacount", "keepalive_requests = 2\n");
    let mut raw = String::new();
    for _ in 0..3 {
        raw.push_str("GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\n");
    }
    let replies = parse_replies(&d.raw(raw.as_bytes()).unwrap());
    assert_eq!(replies.len(), 2, "the connection closed after two requests");
    assert!(replies[1].head.contains("Connection: close"));
}

/// `keepalive_timeout` bounds the wait between requests, and it is a
/// different bound from `read_timeout`, which covers a request already
/// arriving. A connection sitting idle must be let go on the first one; taken
/// on the second, this configuration would hold the connection thirty seconds.
#[test]
fn an_idle_connection_is_closed_by_the_keepalive_timeout() {
    let d = Daemon::start(
        "smoke-katimeout",
        "read_timeout = 30s\nkeepalive_timeout = 2s\n",
    );
    let mut s = UnixStream::connect(d.socket()).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(12)))
        .expect("read timeout");
    s.write_all(b"GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\n")
        .expect("write");
    s.flush().expect("flush");

    // Read to end of file: the answer, then nothing at all until the daemon
    // gives up on the connection and closes it.
    let started = Instant::now();
    let mut out = Vec::new();
    s.read_to_end(&mut out)
        .unwrap_or_else(|e| panic!("the daemon never closed the idle connection: {e}"));
    let waited = started.elapsed();

    assert_eq!(parse_reply(&out).expect("a reply").status, 200);
    assert!(
        waited >= Duration::from_secs(1),
        "closed in {waited:?}: the connection was not held open for keep-alive at all"
    );
    assert!(
        waited < Duration::from_secs(10),
        "held for {waited:?}: read_timeout governed the idle wait, not keepalive_timeout"
    );
}

/// The idle bound must not leak into the request that follows it. The socket
/// carries one read timeout, so a connection that waited under
/// `keepalive_timeout` and then began receiving a request has to be put back
/// under `read_timeout` before the rest of it arrives — including when the
/// request arrived in the same read that ended the idle wait, which is the
/// case where nothing else forces the switch.
#[test]
fn a_request_after_an_idle_wait_is_governed_by_the_read_timeout() {
    let d = Daemon::start(
        "smoke-kaswitch",
        "read_timeout = 10s\nkeepalive_timeout = 1s\n",
    );
    let mut s = UnixStream::connect(d.socket()).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");

    // `HEAD` throughout, so an answer ends at its blank line and none of it is
    // left in the socket to be mistaken for the next one.
    let head = b"HEAD /v1/status HTTP/1.1\r\nHost: c\r\n\r\n";
    let read_one = |s: &mut UnixStream| {
        let mut out = Vec::new();
        while !out.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            match s.read(&mut byte) {
                Ok(0) => panic!("the daemon closed before answering"),
                Ok(_) => out.push(byte[0]),
                Err(e) => panic!("reading an answer: {e}"),
            }
        }
        out
    };

    s.write_all(head).expect("write");
    s.flush().expect("flush");
    read_one(&mut s);

    // Long enough that the daemon is certainly parked in the idle read.
    std::thread::sleep(Duration::from_millis(400));

    // One write ends that wait and starts a second request in the same breath,
    // so the read that follows is the first one in its own turn with bytes
    // already buffered.
    let mut both = head.to_vec();
    both.extend_from_slice(b"HEAD /v1/status HTTP/1.1\r\nHost: c\r\n");
    s.write_all(&both).expect("write");
    s.flush().expect("flush");
    read_one(&mut s);

    // Paused for longer than the idle bound and well inside the read bound.
    std::thread::sleep(Duration::from_millis(2500));
    s.write_all(b"Connection: close\r\n\r\n").expect("write");
    s.flush().expect("flush");

    let mut out = Vec::new();
    s.read_to_end(&mut out).expect("read the last answer");
    let reply = parse_reply(&out).expect("a last reply");
    assert_eq!(
        reply.status, 200,
        "a slow request after an idle wait was cut off by keepalive_timeout"
    );
}

/// `connections.served` sits beside `active` and `max`, which count
/// connections; counting requests there would make the three numbers describe
/// two different things.
#[test]
fn served_counts_connections_not_requests() {
    let d = Daemon::start("smoke-counters", "");
    let served = |d: &Daemon| -> u64 {
        let text = d.get("/v1/status").text();
        let at = text.find("\"served\":").expect("a served counter");
        text[at + 9..]
            .trim_start()
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse().ok())
            .expect("a number")
    };

    let before = served(&d);
    let mut raw = String::new();
    for _ in 0..19 {
        raw.push_str("GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\n");
    }
    raw.push_str("GET /v1/status HTTP/1.1\r\nHost: c\r\nConnection: close\r\n\r\n");
    let replies = parse_replies(&d.raw(raw.as_bytes()).expect("pipelined"));
    assert_eq!(replies.len(), 20, "twenty answers on one connection");
    let after = served(&d);

    // Two connections since `before`: the pipelined one and the one that read
    // `after`. The slack covers the readiness probe the harness leaves behind.
    assert!(
        after - before <= 3,
        "served went up by {} across two connections carrying 21 requests",
        after - before
    );
}

#[test]
fn auth_hides_everything_including_what_exists() {
    let d = Daemon::start("smoke-auth", "auth_token = opensesame\n");

    for target in [
        "/v1/status",
        "/v1/archives",
        &entry(SAMPLE_UUID, "index.html"),
    ] {
        let r = d.get(target);
        assert_eq!(r.status, 401, "{target}");
        assert_eq!(r.header("www-authenticate"), Some("Bearer"));
    }
    // A missing archive and a real one are the same answer without a token.
    let missing = d.get("/v1/archives/ffffffff-ffff-ffff-ffff-ffffffffffff");
    let present = d.get(&format!("/v1/archives/{SAMPLE_UUID}"));
    assert_eq!(missing.status, present.status);
    assert_eq!(missing.body, present.body);

    let r = d.request("GET", "/v1/status", "Authorization: Bearer opensesame\r\n");
    assert_eq!(r.status, 200);
    let r = d.request("GET", "/v1/status", "Authorization: Bearer opensesam\r\n");
    assert_eq!(r.status, 401);
}

#[test]
fn the_cli_speaks_the_same_api() {
    let d = Daemon::start("smoke-cli", "");

    let (code, out, err) = d.cli(&["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("archives        1\n"), "{out}");
    assert!(out.contains("sandbox"), "{out}");

    let (code, out, _) = d.cli(&["archives"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("UUID"), "{out}");
    assert!(out.contains(SAMPLE_UUID), "{out}");

    let (code, out, _) = d.cli(&["archive", SAMPLE_UUID]);
    assert_eq!(code, 0);
    assert!(out.contains("content_namespace  "), "{out}");

    // Content is a byte pipe: `cli` captures a pipe, not a terminal, so these
    // are the stored bytes exactly.
    let (code, out, _) = d.cli(&["get", SAMPLE_UUID, "index.html"]);
    assert_eq!(code, 0);
    assert_eq!(out, "<html><body>index</body></html>");

    // Including for content that is not text, and not only for content that
    // happens to survive a lossy conversion.
    let (code, out, err) = d.cli_bytes(&["get", SAMPLE_UUID, "logo.png"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        d.get(&format!("/v1/archives/{SAMPLE_UUID}/entry/logo.png"))
            .body
    );

    let (code, out, _) = d.cli(&["head", SAMPLE_UUID, "logo.png"]);
    assert_eq!(code, 0);
    assert!(out.contains("Content-Type: image/png"), "{out}");

    let (code, out, _) = d.cli(&["suggest", SAMPLE_UUID, "Ma"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("TITLE"), "{out}");
    assert!(out.contains("Main Page"), "{out}");

    // A path alone, so it can be fed straight back to `get`.
    let (code, out, _) = d.cli(&["random", SAMPLE_UUID]);
    assert_eq!(code, 0);
    let path = out.trim_end().to_owned();
    assert!(!path.is_empty() && !path.contains('{'), "{out}");
    let (code, _, err) = d.cli(&["get", SAMPLE_UUID, &path]);
    assert_eq!(code, 0, "{path:?}: {err}");

    // A 404 is an error exit, and the failure goes to stderr rather than
    // leaving something answer-shaped on stdout.
    let (code, out, err) = d.cli(&["get", SAMPLE_UUID, "nope.html"]);
    assert_eq!(code, 1);
    assert_eq!(out, "");
    assert!(err.contains("not_found: no such resource"), "{err}");

    let (code, _, _) = d.cli(&["nonsense"]);
    assert_eq!(code, 2);
}

#[test]
fn the_cli_prints_json_only_when_asked() {
    let d = Daemon::start("smoke-cli-json", "");

    for args in [
        vec!["--json", "status"],
        vec!["status", "--json"],
        vec!["--json", "archives"],
        vec!["--json", "archive", SAMPLE_UUID],
        vec!["--json", "suggest", SAMPLE_UUID, "Ma"],
        vec!["--json", "random", SAMPLE_UUID],
    ] {
        let (code, out, err) = d.cli(&args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert!(out.starts_with('{'), "{args:?}: {out}");
    }

    // The same commands without it are reports, and no report is JSON.
    for args in [
        vec!["status"],
        vec!["archives"],
        vec!["archive", SAMPLE_UUID],
        vec!["suggest", SAMPLE_UUID, "Ma"],
        vec!["random", SAMPLE_UUID],
    ] {
        let (code, out, err) = d.cli(&args);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert!(!out.starts_with('{'), "{args:?}: {out}");
    }

    // An error document is what `--json` promises even when the request failed.
    let (code, out, err) = d.cli(&["--json", "get", SAMPLE_UUID, "nope.html"]);
    assert_eq!(code, 1);
    assert!(out.contains(r#""code":"not_found""#), "{out}");
    assert_eq!(err, "");
}

#[test]
fn an_archive_directory_can_be_empty() {
    let d = Daemon::with_archives("smoke-empty", "", &[]);
    let r = d.get("/v1/archives");
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), r#"{"archives":[]}"#);
}

#[test]
fn legacy_namespace_links_resolve() {
    let d = Daemon::with_archives(
        "smoke-legacy",
        "",
        &[(
            "legacy.zim",
            testutil::Builder::new()
                .uuid(*b"cairn-test-lgcy1")
                .version(5, 0)
                .content("index.html", "Main", 0, b"legacy")
                .content_in(b'I', "logo.png", "Logo", 0, b"png")
                .build(),
        )],
    );
    let uuid = "63616972-6e2d-7465-7374-2d6c67637931";
    assert_eq!(d.get(&entry(uuid, "index.html")).body, b"legacy");
    let r = d.get(&entry(uuid, "I/logo.png"));
    assert_eq!(r.body, b"png");
    assert_eq!(r.header("x-cairn-path"), Some("I/logo.png"));
}
