//! `make chaos`: archives that change or lie underneath a running daemon.
//!
//! Archives are mapped, so truncating one under the daemon can fault with
//! SIGBUS, which is not recoverable in safe Rust and which cairn does not try
//! to catch. In practice a write whose source pages lost their backing fails
//! with EFAULT instead, and the daemon survives with a cut-short transfer.
//! The property tested here covers both: the daemon never answers with wrong
//! bytes. It serves the entry, refuses it, cuts the transfer short, or dies —
//! and the units set `Restart=on-failure` for the last.

// a panic in a test is the failure report.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// SCOPE §7.1 says no process is spawned; clippy.toml enforces it. Here
// the daemon under test is a process.
#![allow(clippy::disallowed_methods)]

mod common;

use common::{Daemon, SAMPLE_UUID};

fn entry(path: &str) -> String {
    format!("/v1/archives/{SAMPLE_UUID}/entry/{path}")
}

/// A cluster large enough that its pages are not all resident at open time.
fn fat_archive() -> Vec<u8> {
    testutil::sample()
        .blobs_per_cluster(1)
        .content("big.txt", "Big", 2, &vec![b'z'; 512 * 1024])
        .build()
}

#[test]
fn a_truncated_archive_never_yields_wrong_bytes() {
    let mut d = Daemon::with_archives("chaos-truncate", "", &[("s.zim", fat_archive())]);
    assert_eq!(
        d.get(&entry("index.html")).body,
        b"<html><body>index</body></html>"
    );

    let path = d.archive_dir().join("s.zim");
    let full = std::fs::metadata(&path).expect("stat").len();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open");
    file.set_len(full / 3).expect("truncate");
    drop(file);

    // Three outcomes are acceptable and one is not. Acceptable: an honest
    // refusal, a complete answer that is still the right bytes, or a transfer
    // visibly cut short because the mapping faulted and the process died.
    // Unacceptable: a complete answer carrying content the file no longer has.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.get(&entry("big.txt")))) {
        Ok(reply) if reply.status == 200 => {
            let declared: usize = reply
                .header("content-length")
                .and_then(|v| v.parse().ok())
                .expect("content-length");
            if reply.body.len() == declared {
                assert!(
                    reply.body.iter().all(|&b| b == b'z'),
                    "a complete 200 must carry the entry's real bytes"
                );
            } else {
                // Visibly incomplete: any client comparing the body against
                // Content-Length sees the failure. Writing from a mapping whose
                // backing file shrank fails the write with EFAULT rather than
                // faulting, so the daemon often survives this.
                assert!(reply.body.len() < declared);
                if d.alive() {
                    assert_eq!(d.get("/v1/status").status, 200, "still healthy");
                }
            }
        }
        Ok(reply) => assert!(
            reply.status == 503 || reply.status == 404,
            "truncated archive answered {}",
            reply.status
        ),
        Err(_) => {
            // The mapping faulted before a response head was written.
            assert!(
                !d.alive(),
                "request failed but the daemon claims to be alive"
            );
        }
    }
}

#[test]
fn an_archive_replaced_in_place_does_not_change_the_answer_silently() {
    let mut d = Daemon::with_archives("chaos-replace", "", &[("s.zim", fat_archive())]);
    let before = d.get(&entry("index.html"));
    assert_eq!(before.status, 200);

    // Same length, different content: the daemon holds the old mapping.
    let mut other = fat_archive();
    let len = other.len();
    other.truncate(len);
    std::fs::write(d.archive_dir().join("s.zim"), &other).expect("rewrite");

    if let Ok(after) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.get(&entry("index.html"))))
    {
        assert_eq!(after.status, before.status);
        assert_eq!(after.body, before.body, "identity is fixed at open time");
    } else {
        assert!(!d.alive());
    }
}

#[test]
fn a_corrupt_archive_is_refused_at_startup_not_served_broken() {
    let mut bytes = testutil::sample().build();
    // Point the cluster pointer list past the end of the file.
    let at = 48;
    bytes[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());

    let dir = testutil::TempDir::new("chaos-corrupt");
    std::fs::create_dir_all(dir.path().join("archives")).unwrap();
    std::fs::write(dir.path().join("archives/bad.zim"), &bytes).unwrap();
    let conf = dir.path().join("cairn.conf");
    std::fs::write(
        &conf,
        format!(
            "listen = unix:{}/cairn.sock\narchive_dir = {}/archives\n",
            dir.path().display(),
            dir.path().display()
        ),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cairnd"))
        .arg("-c")
        .arg(&conf)
        .arg("--check")
        .output()
        .expect("run cairnd");
    assert!(!out.status.success(), "a malformed archive must not open");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.zim"), "the error names the file: {err}");
}

#[test]
fn junk_bytes_in_the_archive_directory_are_refused_by_name() {
    let dir = testutil::TempDir::new("chaos-junk");
    std::fs::create_dir_all(dir.path().join("archives")).unwrap();
    std::fs::write(
        dir.path().join("archives/not-really.zim"),
        b"this is not a ZIM file",
    )
    .unwrap();
    let conf = dir.path().join("cairn.conf");
    std::fs::write(
        &conf,
        format!(
            "listen = unix:{}/cairn.sock\narchive_dir = {}/archives\n",
            dir.path().display(),
            dir.path().display()
        ),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cairnd"))
        .arg("-c")
        .arg(&conf)
        .arg("--check")
        .output()
        .expect("run cairnd");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not-really.zim"));
}

#[test]
fn a_duplicate_uuid_refuses_to_start_and_names_both() {
    let dir = testutil::TempDir::new("chaos-dup");
    std::fs::create_dir_all(dir.path().join("archives")).unwrap();
    let bytes = testutil::sample().build();
    std::fs::write(dir.path().join("archives/first.zim"), &bytes).unwrap();
    std::fs::write(dir.path().join("archives/second.zim"), &bytes).unwrap();
    let conf = dir.path().join("cairn.conf");
    std::fs::write(
        &conf,
        format!(
            "listen = unix:{}/cairn.sock\narchive_dir = {}/archives\n",
            dir.path().display(),
            dir.path().display()
        ),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cairnd"))
        .arg("-c")
        .arg(&conf)
        .output()
        .expect("run cairnd");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("first.zim") && err.contains("second.zim"),
        "{err}"
    );
}

#[test]
fn a_client_that_leaves_mid_response_does_not_take_the_daemon_with_it() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let d = Daemon::with_archives("chaos-hangup", "", &[("s.zim", fat_archive())]);
    for _ in 0..8 {
        let mut s = UnixStream::connect(d.socket()).expect("connect");
        let _ =
            s.write_all(format!("GET {} HTTP/1.1\r\nHost: c\r\n\r\n", entry("big.txt")).as_bytes());
        // Leave without reading a byte of the 512K body.
        drop(s);
    }
    assert_eq!(
        d.get("/v1/status").status,
        200,
        "still serving; log:\n{}",
        d.log()
    );
}

#[test]
fn a_half_written_request_is_dropped_on_the_read_timeout() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let d = Daemon::start(
        "chaos-slowloris",
        "read_timeout = 1s\nmax_connections = 2\n",
    );
    let mut s = UnixStream::connect(d.socket()).expect("connect");
    s.write_all(b"GET /v1/status HTTP/1.1\r\nHost: c\r\n")
        .expect("partial write");
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let reply = common::parse_reply(&out).expect("a response");
    assert_eq!(reply.status, 408);
    assert_eq!(d.get("/v1/status").status, 200);
}

#[test]
fn a_missing_socket_directory_is_named_not_guessed_at() {
    let dir = testutil::TempDir::new("chaos-nosockdir");
    std::fs::create_dir_all(dir.path().join("archives")).unwrap();
    std::fs::write(
        dir.path().join("archives/s.zim"),
        testutil::sample().build(),
    )
    .unwrap();
    let conf = dir.path().join("cairn.conf");
    std::fs::write(
        &conf,
        format!(
            "listen = unix:{}/absent/cairn.sock\narchive_dir = {}/archives\n",
            dir.path().display(),
            dir.path().display()
        ),
    )
    .unwrap();

    // Both the dry run and the real one, because a check that passes and then
    // a start that fails is worse than either.
    for args in [vec!["-c"], vec!["-c"]].into_iter().zip([true, false]) {
        let (flags, dry) = args;
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_cairnd"));
        cmd.args(flags).arg(&conf);
        if dry {
            cmd.arg("--check");
        }
        let out = cmd.output().expect("run cairnd");
        assert!(!out.status.success(), "dry={dry}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("cannot listen on unix:"), "dry={dry}: {err}");
        assert!(err.contains("absent"), "the directory is named: {err}");
        assert!(err.contains("does not exist"), "dry={dry}: {err}");
    }
}
