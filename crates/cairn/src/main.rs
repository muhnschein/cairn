//! `cairn(1)`: the control CLI. Speaks the same HTTP API as any other client.
//!
//! One request per invocation, hand-rolled argument parsing, no dependencies.
//! Metadata commands render the daemon's JSON for a person; `--json` passes it
//! through untouched, which is the form scripts should read. `get` is a byte
//! pipe either way — entry content is not JSON and is never reformatted.

mod json;
mod render;
mod text;

use std::io::{IsTerminal, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_SOCKET: &str = "/run/cairn/cairn.sock";

/// Release name printed by `--version`: `YYYY.0M`, with a counter appended for
/// a second release in one month.
///
/// Spelled out here rather than taken from `api`, because the CLI has no
/// dependencies at all (`ci/check-boundaries.sh`); a test below pins it to the
/// manifest, which is the same workspace version `api` is pinned to.
const VERSION: &str = "2026.08";

const USAGE: &str = "\
usage: cairn [-s PATH | -a ADDR] [-t TOKEN] [--json] COMMAND [ARGS]

  -s, --socket PATH   unix socket (default /run/cairn/cairn.sock)
  -a, --address ADDR  loopback TCP address instead of a socket
  -t, --token TOKEN   bearer token
      --timeout SECS  read and write timeout (default 30)
      --json          print the daemon's JSON instead of a report
  -V, --version       print the version and exit
  -h, --help          print this message and exit

commands:
  status                     daemon state and the sandbox actually applied
  archives                   open archives
  archive UUID               one archive and its metadata
  get UUID PATH              entry content, written to stdout
  head UUID PATH             entry headers only
  suggest UUID QUERY [N]     title-prefix suggestions
  random UUID                one random entry path
  raw METHOD TARGET          any request, for debugging
";

enum Endpoint {
    Unix(PathBuf),
    Tcp(String),
}

/// Where the daemon is and how to reach it, once.
struct Client {
    endpoint: Endpoint,
    token: Option<String>,
    timeout: Duration,
}

/// One HTTP answer.
struct Reply {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut endpoint = Endpoint::Unix(PathBuf::from(DEFAULT_SOCKET));
    let mut token: Option<String> = None;
    let mut timeout = Duration::from_secs(30);
    let mut as_json = false;
    let mut rest: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--socket" => match args.next() {
                Some(v) => endpoint = Endpoint::Unix(PathBuf::from(v)),
                None => return usage_error(&arg),
            },
            "-a" | "--address" => match args.next() {
                Some(v) => endpoint = Endpoint::Tcp(v),
                None => return usage_error(&arg),
            },
            "-t" | "--token" => match args.next() {
                Some(v) => token = Some(v),
                None => return usage_error(&arg),
            },
            "--timeout" => match args.next().and_then(|v| v.parse().ok()) {
                Some(secs) => timeout = Duration::from_secs(secs),
                None => return usage_error(&arg),
            },
            "--json" => as_json = true,
            "-V" | "--version" => {
                println!("cairn {VERSION}");
                return 0;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            // `--json` is also accepted after the command, where clove puts it
            // and where a person reaching for it will type it.
            _ => {
                rest.push(arg);
                for later in args {
                    if later == "--json" {
                        as_json = true;
                    } else {
                        rest.push(later);
                    }
                }
                break;
            }
        }
    }

    let Some(command) = rest.first().cloned() else {
        eprint!("{USAGE}");
        return 2;
    };
    let arg = |n: usize| rest.get(n).unwrap_or(&String::new()).clone();

    let (method, target) = match (command.as_str(), rest.len()) {
        ("status", 1) => ("GET", "/v1/status".to_owned()),
        ("archives", 1) => ("GET", "/v1/archives".to_owned()),
        ("archive", 2) => ("GET", format!("/v1/archives/{}", arg(1))),
        ("get", 3) => (
            "GET",
            format!("/v1/archives/{}/entry/{}", arg(1), encode(&arg(2))),
        ),
        ("head", 3) => (
            "HEAD",
            format!("/v1/archives/{}/entry/{}", arg(1), encode(&arg(2))),
        ),
        ("suggest", 3) => (
            "GET",
            format!("/v1/archives/{}/suggest?q={}", arg(1), encode(&arg(2))),
        ),
        ("suggest", 4) => (
            "GET",
            format!(
                "/v1/archives/{}/suggest?q={}&limit={}",
                arg(1),
                encode(&arg(2)),
                arg(3)
            ),
        ),
        ("random", 2) => ("GET", format!("/v1/archives/{}/random", arg(1))),
        ("raw", 3) => {
            let m = arg(1);
            (if m == "HEAD" { "HEAD" } else { "GET" }, arg(2))
        }
        _ => {
            eprint!("{USAGE}");
            return 2;
        }
    };

    let client = Client {
        endpoint,
        token,
        timeout,
    };
    let reply = match client.request(method, &target) {
        Ok(reply) => reply,
        Err(e) => {
            eprintln!("cairn: {e}");
            return 1;
        }
    };

    // An error document is one line on stderr, so a failed command does not put
    // something that looks like an answer on stdout. `--json` keeps the
    // document itself, because that is what a script asked for.
    if reply.status >= 400 {
        if as_json {
            let _ = std::io::stdout().write_all(&reply.body);
            newline_if_missing(&reply.body);
        } else {
            eprintln!("cairn: {}", render::fault(&reply.body, reply.status));
        }
        return 1;
    }

    match command.as_str() {
        // Bytes, not text: `head` is a debugging command and `get` and `raw`
        // are pipes. Nothing here is reformatted in either mode.
        "head" => print!("{}", reply.headers),
        "get" => return write_entry(&reply),
        "raw" => {
            let _ = std::io::stdout().write_all(&reply.body);
            newline_if_missing(&reply.body);
        }
        _ if as_json => {
            let _ = std::io::stdout().write_all(&reply.body);
            newline_if_missing(&reply.body);
        }
        _ => {
            let text = match std::str::from_utf8(&reply.body) {
                Ok(text) => text,
                Err(_) => {
                    eprintln!("cairn: the daemon's answer was not UTF-8");
                    return 1;
                }
            };
            let value = match json::parse(text) {
                Ok(value) => value,
                Err(e) => {
                    eprintln!("cairn: parsing the daemon's answer: {e}");
                    return 1;
                }
            };
            print!(
                "{}",
                match command.as_str() {
                    "status" => render::status(&value),
                    "archives" => render::archives(&value),
                    "archive" => render::archive(&value),
                    "random" => render::random(&value),
                    // An empty list has two causes and a person needs to know
                    // which: no match, or an archive with no title ordering at
                    // all. Only the second is worth a second request.
                    _ => {
                        let empty = value
                            .get("suggestions")
                            .and_then(json::Value::as_array)
                            .is_some_and(<[json::Value]>::is_empty);
                        render::suggest(&value, empty && !client.can_suggest(&arg(1)))
                    }
                }
            );
        }
    }
    0
}

/// Write entry content: the bytes as stored, unless stdout is a terminal.
///
/// A terminal is an interpreter and an archive is hostile input (`SECURITY.md`),
/// so an article read at a prompt is scrubbed of what a terminal would act on —
/// its own newlines and tabs kept, since those are the document's layout — and
/// binary is refused outright rather than left to wedge the terminal. Redirected
/// or piped, which is how anything is actually extracted, the bytes are exact.
fn write_entry(reply: &Reply) -> i32 {
    let mut out = std::io::stdout();
    if !out.is_terminal() {
        let _ = out.write_all(&reply.body);
        return 0;
    }
    match std::str::from_utf8(&reply.body) {
        Ok(content) => {
            let _ = out.write_all(text::block(content).as_bytes());
            newline_if_missing(&reply.body);
        }
        Err(_) => {
            eprintln!(
                "cairn: entry is {} ({}), not text; redirect it to a file or pipe it",
                header(&reply.headers, "content-type").unwrap_or("of unknown type"),
                text::bytes(reply.body.len() as u64),
            );
            return 1;
        }
    }
    0
}

fn newline_if_missing(body: &[u8]) {
    if !body.is_empty() && body.last() != Some(&b'\n') {
        println!();
    }
}

/// One response header's value, matched case-insensitively.
fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn usage_error(arg: &str) -> i32 {
    eprintln!("cairn: {arg} needs a value");
    2
}

impl Client {
    fn request(&self, method: &str, target: &str) -> std::io::Result<Reply> {
        let mut head =
            format!("{method} {target} HTTP/1.1\r\nHost: cairn\r\nConnection: close\r\n");
        if let Some(t) = &self.token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        head.push_str("\r\n");

        let mut stream: Box<dyn ReadWrite> = match &self.endpoint {
            Endpoint::Unix(path) => {
                let s = UnixStream::connect(path).map_err(|e| {
                    // A bare ENOENT names nothing, and the socket path is the
                    // one thing worth naming when nothing answers.
                    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
                })?;
                s.set_read_timeout(Some(self.timeout))?;
                s.set_write_timeout(Some(self.timeout))?;
                Box::new(s)
            }
            Endpoint::Tcp(addr) => {
                let s = TcpStream::connect(addr)
                    .map_err(|e| std::io::Error::new(e.kind(), format!("{addr}: {e}")))?;
                s.set_read_timeout(Some(self.timeout))?;
                s.set_write_timeout(Some(self.timeout))?;
                Box::new(s)
            }
        };
        stream.write_all(head.as_bytes())?;
        stream.flush()?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw)?;

        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "no response head")
            })?;
        let headers = String::from_utf8_lossy(&raw[..split + 2]).into_owned();
        let body = raw[split + 4..].to_vec();
        let status = headers
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no status"))?;
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    /// Whether this archive has a title ordering at all.
    ///
    /// Only asked when a suggestion list came back empty, and only on the human
    /// path: it is the difference between "nothing matched" and "this archive
    /// cannot answer". A daemon that will not say counts as "it can", so an
    /// unreachable second request never invents an explanation.
    fn can_suggest(&self, uuid: &str) -> bool {
        let Ok(reply) = self.request("GET", &format!("/v1/archives/{uuid}")) else {
            return true;
        };
        let Ok(text) = std::str::from_utf8(&reply.body) else {
            return true;
        };
        json::parse(text)
            .ok()
            .and_then(|v| v.get("suggest").and_then(json::Value::as_bool))
            .unwrap_or(true)
    }
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Percent-encode everything the daemon would refuse in a request target.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_encoded_but_paths_keep_their_separators() {
        assert_eq!(encode("Climate_change"), "Climate_change");
        assert_eq!(encode("I/logo.png"), "I/logo.png");
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode("café"), "caf%C3%A9");
    }

    /// Nothing but this connects the two spellings, and nothing but the
    /// shared manifest connects this CLI's copy to `api::VERSION`.
    #[test]
    fn version_matches_the_manifest() {
        let major = env!("CARGO_PKG_VERSION_MAJOR");
        let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
        let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
        let expected = if patch == 0 {
            format!("{major}.{minor:02}")
        } else {
            format!("{major}.{minor:02}.{patch}")
        };
        assert_eq!(VERSION, expected);
    }

    #[test]
    fn headers_are_matched_without_regard_to_case() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\n";
        assert_eq!(header(head, "content-type"), Some("image/png"));
        assert_eq!(header(head, "CONTENT-LENGTH"), Some("4"));
        assert_eq!(header(head, "x-missing"), None);
    }
}
