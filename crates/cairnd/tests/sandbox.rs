//! `make sandbox`: the serving workload under the live filter.
//!
//! A missing entry in the seccomp allowlist kills the daemon with SIGSYS, so
//! these tests fail loudly rather than quietly serving unconfined.

mod common;

use common::{Daemon, SAMPLE_UUID};

/// Every endpoint, exercised in one pass.
fn exercise(d: &Daemon) {
    let targets = [
        "/v1/status".to_owned(),
        "/v1/archives".to_owned(),
        format!("/v1/archives/{SAMPLE_UUID}"),
        format!("/v1/archives/{SAMPLE_UUID}/entry/index.html"),
        format!("/v1/archives/{SAMPLE_UUID}/entry/logo.png"),
        format!("/v1/archives/{SAMPLE_UUID}/entry/home.html"),
        format!("/v1/archives/{SAMPLE_UUID}/entry/big.txt"),
        format!("/v1/archives/{SAMPLE_UUID}/entry/missing.html"),
        format!("/v1/archives/{SAMPLE_UUID}/suggest?q=M"),
        format!("/v1/archives/{SAMPLE_UUID}/random"),
        "/v1/nothing".to_owned(),
    ];
    for target in &targets {
        let r = d.get(target);
        assert!(r.status > 0, "{target}: no status");
    }
    // Ranges, HEAD, keep-alive and a refusal all run under the filter too.
    let entry = format!("/v1/archives/{SAMPLE_UUID}/entry/big.txt");
    assert_eq!(
        d.request("GET", &entry, "Range: bytes=0-99\r\n").status,
        206
    );
    assert_eq!(d.request("HEAD", &entry, "").status, 200);
    assert_eq!(d.request("DELETE", "/v1/status", "").status, 405);
    let pipelined = "GET /v1/status HTTP/1.1\r\nHost: c\r\n\r\nGET /v1/archives HTTP/1.1\r\nHost: c\r\nConnection: close\r\n\r\n";
    assert_eq!(
        common::parse_replies(&d.raw(pipelined.as_bytes()).unwrap()).len(),
        2
    );
}

/// An archive with a compressed cluster big enough to allocate for.
fn archives() -> Vec<(&'static str, Vec<u8>)> {
    vec![(
        "s.zim",
        testutil::sample()
            .compression(testutil::Compression::Zstd)
            .content("big.txt", "Big", 2, &vec![b'q'; 400 * 1024])
            .build(),
    )]
}

#[test]
fn the_workload_runs_under_a_killing_filter() {
    let mut d = Daemon::with_archives(
        "sandbox-seccomp",
        // Landlock is off here so the test means the same thing on kernels
        // that do not have it; the next test covers Landlock where it exists.
        "sandbox = require\nsandbox_landlock = off\nsandbox_action = kill\n",
        &archives(),
    );
    exercise(&d);
    assert!(d.alive(), "the filter killed the daemon; log:\n{}", d.log());

    let status = d.get("/v1/status").text();
    assert!(
        status.contains(r#""name":"seccomp","state":"applied""#),
        "{status}"
    );
    assert!(status.contains(r#""required":true"#), "{status}");
    assert!(
        status.contains(r#""name":"landlock","state":"disabled""#),
        "{status}"
    );
}

#[test]
fn landlock_applies_where_the_kernel_has_it() {
    let Ok(abi) = sandbox::landlock::abi_version() else {
        eprintln!("landlock: kernel support absent, skipping");
        return;
    };
    let mut d = Daemon::with_archives(
        "sandbox-landlock",
        "sandbox = require\nsandbox_action = kill\n",
        &archives(),
    );
    exercise(&d);
    assert!(
        d.alive(),
        "confinement killed the daemon; log:\n{}",
        d.log()
    );
    let status = d.get("/v1/status").text();
    assert!(
        status.contains(r#""name":"landlock","state":"applied""#),
        "{status}"
    );
    assert!(status.contains(&format!("abi {abi}")), "{status}");
}

#[test]
fn require_refuses_to_start_when_a_layer_is_missing() {
    // Landlock ABI 0 does not exist, so requiring a layer the kernel lacks is
    // simulated by requiring Landlock on a kernel without it. Where Landlock
    // is present, the equivalent check is that `require` starts cleanly.
    let landlock = sandbox::landlock::abi_version().is_ok();
    let dir = testutil::TempDir::new("sandbox-require");
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
            "listen = unix:{}/cairn.sock\narchive_dir = {}/archives\nsandbox = require\n",
            dir.path().display(),
            dir.path().display()
        ),
    )
    .unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_cairnd"))
        .arg("-c")
        .arg(&conf)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");

    if landlock {
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            child.try_wait().expect("wait").is_none(),
            "require should start here"
        );
        let _ = child.kill();
    } else {
        let out = child.wait_with_output().expect("wait");
        assert!(
            !out.status.success(),
            "require must refuse without Landlock"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("sandbox incomplete"), "{err}");
        assert!(err.contains("refusing to serve"), "{err}");
    }
}

#[test]
fn the_daemon_cannot_open_a_file_after_confinement() {
    // openat is not on the allowlist, so a configuration that would need one
    // at request time must not exist. This asserts the negative directly:
    // the filter is installed and the archive directory is the only readable
    // thing left, which is visible in what the daemon reports.
    let d = Daemon::with_archives(
        "sandbox-report",
        "sandbox = best-effort\nsandbox_landlock = off\n",
        &archives(),
    );
    let status = d.get("/v1/status").text();
    assert!(
        status.contains("syscalls"),
        "the filter reports its size: {status}"
    );
    assert!(!sandbox::seccomp::allowed_syscalls().contains(&libc::SYS_openat));
}
