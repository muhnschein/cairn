//! Starting a real daemon over a crafted archive, and talking to it.
//!
//! Each test binary compiles this module separately and uses part of it.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use testutil::TempDir;

/// UUID of the archive `testutil::sample()` builds.
pub const SAMPLE_UUID: &str = "63616972-6e2d-7465-7374-2d7575696431";
/// UUID of the zstd-compressed sample.
pub const ZSTD_UUID: &str = "63616972-6e2d-7465-7374-2d7a73746431";

/// A running `cairnd`, killed when it goes out of scope.
pub struct Daemon {
    child: Child,
    dir: TempDir,
    socket: PathBuf,
}

/// One response, as the client sees it.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub head: String,
    pub body: Vec<u8>,
}

impl Reply {
    /// First value of a response header.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.head.split("\r\n").skip(1).find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Daemon {
    /// Start a daemon over the standard test archives with extra config lines.
    pub fn start(tag: &str, extra: &str) -> Daemon {
        Daemon::with_archives(tag, extra, &[("sample.zim", testutil::sample().build())])
    }

    /// Start a daemon over specific archives.
    pub fn with_archives(tag: &str, extra: &str, archives: &[(&str, Vec<u8>)]) -> Daemon {
        let dir = TempDir::new(tag);
        std::fs::create_dir_all(dir.path().join("archives")).expect("archive dir");
        for (name, bytes) in archives {
            std::fs::write(dir.path().join("archives").join(name), bytes).expect("write archive");
        }
        let socket = dir.path().join("cairn.sock");
        let conf = dir.path().join("cairn.conf");
        std::fs::write(
            &conf,
            format!(
                "listen = unix:{}\narchive_dir = {}/archives\nmax_connections = 4\n{extra}\n",
                socket.display(),
                dir.path().display()
            ),
        )
        .expect("write config");

        let log = std::fs::File::create(dir.path().join("cairnd.log")).expect("log file");
        let child = Command::new(env!("CARGO_BIN_EXE_cairnd"))
            .arg("-c")
            .arg(&conf)
            .stdout(log.try_clone().expect("clone log"))
            .stderr(log)
            .spawn()
            .expect("spawn cairnd");

        let daemon = Daemon { child, dir, socket };
        daemon.wait_for_socket();
        daemon
    }

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.socket.exists() && UnixStream::connect(&self.socket).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not listen; log:\n{}", self.log());
    }

    /// Path of the daemon's socket.
    pub fn socket(&self) -> &PathBuf {
        &self.socket
    }

    /// The archive directory.
    pub fn archive_dir(&self) -> PathBuf {
        self.dir.path().join("archives")
    }

    /// Everything the daemon has logged so far.
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("cairnd.log")).unwrap_or_default()
    }

    /// True while the daemon is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// How the daemon exited, waiting for it if necessary.
    pub fn wait(&mut self) -> std::process::ExitStatus {
        self.child.wait().expect("wait")
    }

    /// Send raw bytes and read everything that comes back.
    pub fn raw(&self, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut s = UnixStream::connect(&self.socket)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        s.set_write_timeout(Some(Duration::from_secs(10)))?;
        s.write_all(bytes)?;
        s.flush()?;
        let mut out = Vec::new();
        s.read_to_end(&mut out)?;
        Ok(out)
    }

    /// One request on its own connection.
    pub fn request(&self, method: &str, target: &str, headers: &str) -> Reply {
        let raw = format!(
            "{method} {target} HTTP/1.1\r\nHost: cairn\r\nConnection: close\r\n{headers}\r\n"
        );
        let bytes = self
            .raw(raw.as_bytes())
            .unwrap_or_else(|e| panic!("{method} {target}: {e}"));
        parse_reply(&bytes)
            .unwrap_or_else(|| panic!("no response to {method} {target}; log:\n{}", self.log()))
    }

    /// A `GET` on its own connection.
    pub fn get(&self, target: &str) -> Reply {
        self.request("GET", target, "")
    }

    /// Run the `cairn` CLI against this daemon.
    pub fn cli(&self, args: &[&str]) -> (i32, String, String) {
        let (code, out, err) = self.cli_bytes(args);
        (code, String::from_utf8_lossy(&out).into_owned(), err)
    }

    /// [`Daemon::cli`] without the lossy conversion, for content.
    pub fn cli_bytes(&self, args: &[&str]) -> (i32, Vec<u8>, String) {
        let out = Command::new(cli_binary())
            .arg("-s")
            .arg(&self.socket)
            .args(args)
            .output()
            .expect("run cairn");
        (
            out.status.code().unwrap_or(-1),
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Path to the `cairn` CLI, building it if this run has not.
///
/// `cargo test -p cairnd` builds this package's binaries, not another crate's,
/// so the CLI may simply not exist yet.
pub fn cli_binary() -> PathBuf {
    static BUILD: std::sync::Once = std::sync::Once::new();
    let path = sibling_binary("cairn");
    BUILD.call_once(|| {
        if path.exists() {
            return;
        }
        let mut cargo = Command::new(env!("CARGO"));
        cargo.args(["build", "--quiet", "--package", "cairn"]);
        if path.parent().is_some_and(|p| p.ends_with("release")) {
            cargo.arg("--release");
        }
        let status = cargo.status().expect("run cargo build");
        assert!(status.success(), "building the cairn CLI failed");
    });
    assert!(path.exists(), "cairn CLI missing at {}", path.display());
    path
}

/// Path to another crate's binary in the same target directory.
///
/// `CARGO_BIN_EXE_*` only covers this package, and the CLI is its own crate.
pub fn sibling_binary(name: &str) -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop(); // the test binary's directory (…/deps)
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(name)
}

/// Split one response into status, head and body.
pub fn parse_reply(bytes: &[u8]) -> Option<Reply> {
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&bytes[..split]).into_owned();
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    Some(Reply {
        status,
        head,
        body: bytes[split + 4..].to_vec(),
    })
}

/// Split a pipelined stream of responses.
pub fn parse_replies(mut bytes: &[u8]) -> Vec<Reply> {
    let mut out = Vec::new();
    while let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        let head = String::from_utf8_lossy(&bytes[..split]).into_owned();
        let Some(status) = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()) else {
            break;
        };
        let len: usize = head
            .split("\r\n")
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let start = split + 4;
        let end = (start + len).min(bytes.len());
        out.push(Reply {
            status,
            head,
            body: bytes[start..end].to_vec(),
        });
        bytes = &bytes[end..];
    }
    out
}
