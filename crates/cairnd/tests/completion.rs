//! `cairn __complete` against a real daemon.
//!
//! Completion is only interesting when the candidates are the daemon's own:
//! a UUID nobody typed, a path found through its title. These tests drive the
//! installed verb end to end, and pin the two failure manners that matter —
//! an archive answered with what it holds, an unreachable one with silence.

// SCOPE §7.1 says no process is spawned; clippy.toml enforces it. Here
// the CLI under test is a process.
#![allow(clippy::disallowed_methods)]

mod common;

use std::process::Command;

use common::{Daemon, SAMPLE_UUID};

#[test]
fn the_command_word_offers_subcommands_and_flags() {
    let d = Daemon::start("comp-commands", "");

    let (code, out, err) = d.cli(&["__complete", ""]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("status\tdaemon state"), "{out}");
    assert!(out.contains("get\tentry content"), "{out}");
    // The completion verb itself is not a command a person can run.
    assert!(!out.contains("__complete"), "{out}");

    let (code, out, _) = d.cli(&["__complete", "--so"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("--socket\t"), "{out}");

    // Past the command word, only --json means anything.
    let (code, out, _) = d.cli(&["__complete", "archives", "--j"]);
    assert_eq!(code, 0);
    assert_eq!(out, "--json\tprint the daemon's JSON instead of a report\n");
}

#[test]
fn uuids_come_from_the_daemon_with_their_titles() {
    let d = Daemon::start("comp-uuids", "");

    let (code, out, err) = d.cli(&["__complete", "get", ""]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains(&format!("{SAMPLE_UUID}\tSample Archive\n")),
        "{out}"
    );

    // A prefix narrows without a round trip per candidate; a wrong one is
    // empty rather than everything.
    let (code, out, _) = d.cli(&["__complete", "archive", &SAMPLE_UUID[..8]]);
    assert_eq!(code, 0);
    assert!(out.contains(SAMPLE_UUID), "{out}");
    let (_, out, _) = d.cli(&["__complete", "archive", "zzzzzzzz"]);
    assert_eq!(out, "");
}

#[test]
fn paths_are_found_through_titles() {
    let d = Daemon::start("comp-paths", "");

    // The stored title, not the path, is what suggest matches: typing part
    // of "Main Page" completes into index.html.
    let (code, out, err) = d.cli(&["__complete", "get", SAMPLE_UUID, "Ma"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("index.html\tMain Page\n"), "{out}");

    // An empty word asks for the start of the title order.
    let (code, out, _) = d.cli(&["__complete", "head", SAMPLE_UUID, ""]);
    assert_eq!(code, 0);
    for path in ["home.html", "logo.png", "index.html", "notes.txt"] {
        assert!(out.contains(path), "{path} missing from {out}");
    }

    // And the answer feeds straight back in: whatever was offered exists.
    let first = out
        .lines()
        .next()
        .unwrap_or("")
        .split('\t')
        .next()
        .unwrap_or("");
    if !first.is_empty() {
        let (code, _, err) = d.cli(&["head", SAMPLE_UUID, first]);
        assert_eq!(code, 0, "{err}");
    }
}

#[test]
fn raw_method_position_is_static() {
    let d = Daemon::start("comp-raw", "");
    let (code, out, _) = d.cli(&["__complete", "raw", "HE"]);
    assert_eq!(code, 0);
    assert_eq!(out, "HEAD\theaders only\n");
}

#[test]
fn an_unreachable_daemon_is_silently_empty() {
    let dir = temp_socket();
    let out = Command::new(common::cli_binary())
        .arg("-s")
        .arg(&dir)
        .args(["__complete", "get", ""])
        .output()
        .expect("run cairn");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}

#[test]
fn the_options_in_the_forwarded_line_are_the_ones_used() {
    // The shell sends the line as typed: `cairn __complete -s SOCK ...`,
    // with no option of its own in front. The verb must read the socket from
    // the forwarded words, not fall back to the default one.
    let d = Daemon::start("comp-forwarded", "");
    let out = Command::new(common::cli_binary())
        .arg("__complete")
        .arg("-s")
        .arg(d.socket())
        .args(["archive", &SAMPLE_UUID[..8]])
        .output()
        .expect("run cairn");
    assert_eq!(out.status.code(), Some(0));
    let out = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.contains(&format!("{SAMPLE_UUID}\tSample Archive\n")),
        "{out}"
    );

    // A token in the line likewise authenticates the completion requests.
    let d = Daemon::start("comp-forwarded-token", "auth_token = opensesame\n");
    let run = |args: &[&str]| {
        let out = Command::new(common::cli_binary())
            .arg("__complete")
            .args(args)
            .output()
            .expect("run cairn");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (code, out) = run(&[
        "-t",
        "opensesame",
        "-s",
        d.socket().to_str().unwrap(),
        "get",
        "",
    ]);
    assert_eq!(code, Some(0));
    assert!(out.contains(SAMPLE_UUID), "{out}");
}

/// A socket path under the test temporary directory that nothing listens on.
fn temp_socket() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("cairn-comp-dead-{}-{n}", std::process::id()))
}
