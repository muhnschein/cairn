//! `cairn(1)`: the control CLI. Speaks the same HTTP API as any other client.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_SOCKET: &str = "/run/cairn/cairn.sock";

const USAGE: &str = "\
usage: cairn [-s PATH | -a ADDR] [-t TOKEN] COMMAND [ARGS]

  -s, --socket PATH   unix socket (default /run/cairn/cairn.sock)
  -a, --address ADDR  loopback TCP address instead of a socket
  -t, --token TOKEN   bearer token
      --timeout SECS  read and write timeout (default 30)
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

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut endpoint = Endpoint::Unix(PathBuf::from(DEFAULT_SOCKET));
    let mut token: Option<String> = None;
    let mut timeout = Duration::from_secs(30);
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
            "-V" | "--version" => {
                println!("cairn {}", env!("CARGO_PKG_VERSION"));
                return 0;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            _ => {
                rest.push(arg);
                rest.extend(args);
                break;
            }
        }
    }

    let Some(command) = rest.first().cloned() else {
        eprint!("{USAGE}");
        return 2;
    };
    let arg = |n: usize| rest.get(n).map(String::as_str);

    let (method, target) = match (command.as_str(), rest.len()) {
        ("status", 1) => ("GET", "/v1/status".to_owned()),
        ("archives", 1) => ("GET", "/v1/archives".to_owned()),
        ("archive", 2) => ("GET", format!("/v1/archives/{}", arg(1).unwrap_or(""))),
        ("get", 3) => (
            "GET",
            format!(
                "/v1/archives/{}/entry/{}",
                arg(1).unwrap_or(""),
                encode(arg(2).unwrap_or(""))
            ),
        ),
        ("head", 3) => (
            "HEAD",
            format!(
                "/v1/archives/{}/entry/{}",
                arg(1).unwrap_or(""),
                encode(arg(2).unwrap_or(""))
            ),
        ),
        ("suggest", 3) => (
            "GET",
            format!(
                "/v1/archives/{}/suggest?q={}",
                arg(1).unwrap_or(""),
                encode(arg(2).unwrap_or(""))
            ),
        ),
        ("suggest", 4) => (
            "GET",
            format!(
                "/v1/archives/{}/suggest?q={}&limit={}",
                arg(1).unwrap_or(""),
                encode(arg(2).unwrap_or("")),
                arg(3).unwrap_or("")
            ),
        ),
        ("random", 2) => (
            "GET",
            format!("/v1/archives/{}/random", arg(1).unwrap_or("")),
        ),
        ("raw", 3) => {
            let m = arg(1).unwrap_or("GET");
            let t = arg(2).unwrap_or("/").to_owned();
            (if m == "HEAD" { "HEAD" } else { "GET" }, t)
        }
        _ => {
            eprint!("{USAGE}");
            return 2;
        }
    };

    match request(&endpoint, method, &target, token.as_deref(), timeout) {
        Ok((status, headers, body)) => {
            if method == "HEAD" || command == "head" {
                print!("{headers}");
            }
            if !body.is_empty() && method != "HEAD" {
                let _ = std::io::stdout().write_all(&body);
                if body.last() != Some(&b'\n') && looks_like_text(&body) {
                    println!();
                }
            }
            if status >= 400 { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("cairn: {e}");
            1
        }
    }
}

fn usage_error(arg: &str) -> i32 {
    eprintln!("cairn: {arg} needs a value");
    2
}

fn request(
    endpoint: &Endpoint,
    method: &str,
    target: &str,
    token: Option<&str>,
    timeout: Duration,
) -> std::io::Result<(u16, String, Vec<u8>)> {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: cairn\r\nConnection: close\r\n");
    if let Some(t) = token {
        head.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    head.push_str("\r\n");

    let mut stream: Box<dyn ReadWrite> = match endpoint {
        Endpoint::Unix(path) => {
            let s = UnixStream::connect(path)?;
            s.set_read_timeout(Some(timeout))?;
            s.set_write_timeout(Some(timeout))?;
            Box::new(s)
        }
        Endpoint::Tcp(addr) => {
            let s = TcpStream::connect(addr)?;
            s.set_read_timeout(Some(timeout))?;
            s.set_write_timeout(Some(timeout))?;
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
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no response head"))?;
    let headers = String::from_utf8_lossy(&raw[..split + 2]).into_owned();
    let body = raw[split + 4..].to_vec();
    let status = headers
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no status"))?;
    Ok((status, headers, body))
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Percent-encode everything the daemon would refuse in a request target.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn looks_like_text(body: &[u8]) -> bool {
    std::str::from_utf8(body).is_ok_and(|s| !s.contains('\0'))
}
