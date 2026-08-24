//! `cairn.conf` parsing.
//!
//! `key = value`, one per line, `#` to end of line is a comment. Unknown keys
//! are errors: a typo in a limit must not silently leave it at the default.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sandbox::Action;

/// Where the daemon listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    /// A unix socket at this path.
    Unix(PathBuf),
    /// A loopback TCP address. Non-loopback addresses are refused.
    Tcp(SocketAddr),
}

impl fmt::Display for Listen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Listen::Unix(p) => write!(f, "unix:{}", p.display()),
            Listen::Tcp(a) => write!(f, "tcp:{a}"),
        }
    }
}

/// How hard confinement is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Refuse to start unless every enabled layer applies.
    Require,
    /// Apply what the kernel offers and report the rest.
    BestEffort,
    /// Apply nothing.
    Off,
}

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Failures only.
    Error,
    /// Failures and things that will become failures.
    Warn,
    /// The default: startup, confinement, and refusals.
    Info,
    /// Adds per-request detail.
    Debug,
}

/// The daemon's whole configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Unix socket path or loopback TCP address.
    pub listen: Listen,
    /// Mode applied to the unix socket after binding.
    pub socket_mode: u32,
    /// Directory of `*.zim` files, not descended into.
    pub archive_dir: PathBuf,
    /// Shared bearer token, or `None` for an open socket.
    pub auth_token: Option<String>,
    /// Whether a partially applied sandbox is fatal.
    pub sandbox: SandboxMode,
    /// Whether to build a Landlock ruleset at all.
    pub sandbox_landlock: bool,
    /// Whether to install a seccomp filter at all.
    pub sandbox_seccomp: bool,
    /// What a syscall outside the allowlist does.
    pub sandbox_action: Action,
    /// Size of the worker pool, and so the connection ceiling.
    pub max_connections: usize,
    /// How long a client may take to finish a request.
    pub read_timeout: Duration,
    /// How long a response may take to write.
    pub write_timeout: Duration,
    /// Requests one connection may make before it is closed.
    pub keepalive_requests: u32,
    /// How long an idle connection is held open.
    pub keepalive_timeout: Duration,
    /// Requests per second one connection may sustain.
    pub request_rate: f64,
    /// Requests one connection may make in a burst.
    pub request_burst: f64,
    /// Ceiling on one decompressed cluster.
    pub max_cluster_bytes: usize,
    /// Total decompressed bytes the shared cluster cache may hold.
    pub cluster_cache_bytes: usize,
    /// Ceiling on one `M` namespace value, as `cairn.conf(5)` documents it.
    pub max_metadata_bytes: usize,
    /// Ceiling on the entries one metadata scan may read.
    pub max_metadata_entries: usize,
    /// Longest request line, method and target included.
    pub max_request_line: usize,
    /// Longest header block.
    pub max_header_bytes: usize,
    /// Most headers in one request.
    pub max_headers: usize,
    /// Longest decoded entry path.
    pub max_path_bytes: usize,
    /// Longest `q` value accepted by `/suggest`.
    pub suggest_max_query: usize,
    /// Most suggestions returned.
    pub suggest_max_results: usize,
    /// Sent with entry content; see `SECURITY.md`.
    pub content_security_policy: String,
    /// Verbosity of the daemon log.
    pub log_level: Level,
    /// Whether to log one line per request.
    pub access_log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: Listen::Unix(PathBuf::from("/run/cairn/cairn.sock")),
            socket_mode: 0o660,
            archive_dir: PathBuf::from("/var/lib/cairn/archives"),
            auth_token: None,
            sandbox: SandboxMode::BestEffort,
            sandbox_landlock: true,
            sandbox_seccomp: true,
            sandbox_action: Action::Kill,
            max_connections: 64,
            read_timeout: Duration::from_secs(15),
            write_timeout: Duration::from_secs(30),
            keepalive_requests: 100,
            keepalive_timeout: Duration::from_secs(15),
            request_rate: 50.0,
            request_burst: 100.0,
            max_cluster_bytes: 32 * 1024 * 1024,
            cluster_cache_bytes: 64 * 1024 * 1024,
            max_metadata_bytes: 8 * 1024,
            max_metadata_entries: 64,
            max_request_line: 8 * 1024,
            max_header_bytes: 16 * 1024,
            max_headers: 64,
            max_path_bytes: 1024,
            suggest_max_query: 128,
            suggest_max_results: 32,
            content_security_policy: "default-src 'none'; sandbox".to_owned(),
            log_level: Level::Info,
            access_log: false,
        }
    }
}

/// A configuration file that could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// Line the failure was found on, or 0 for the file as a whole.
    pub line: usize,
    /// What was wrong, written for the operator.
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Read and parse a configuration file.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError {
            line: 0,
            message: format!("{}: {e}", path.display()),
        })?;
        Config::parse(&text)
    }

    /// Parse configuration text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let mut c = Config::default();
        let mut token_file: Option<PathBuf> = None;

        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let n = n + 1;
            let (key, value) = line.split_once('=').ok_or_else(|| ConfigError {
                line: n,
                message: format!("expected `key = value`, found {line:?}"),
            })?;
            let (key, value) = (key.trim(), value.trim());
            // The line alone makes an operator count lines; the key makes the
            // message searchable in cairn.conf(5).
            let err = |m: String| ConfigError {
                line: n,
                message: format!("{key}: {m}"),
            };

            match key {
                "listen" => c.listen = parse_listen(value).map_err(err)?,
                "socket_mode" => {
                    c.socket_mode = u32::from_str_radix(value.trim_start_matches("0o"), 8)
                        .map_err(|_| err(format!("not an octal mode: {value:?}")))?;
                }
                "archive_dir" => c.archive_dir = PathBuf::from(value),
                "auth_token" => c.auth_token = Some(value.to_owned()),
                "auth_token_file" => token_file = Some(PathBuf::from(value)),
                "sandbox" => {
                    c.sandbox = match value {
                        "require" => SandboxMode::Require,
                        "best-effort" => SandboxMode::BestEffort,
                        "off" => SandboxMode::Off,
                        _ => {
                            return Err(err(format!(
                                "expected require|best-effort|off, found {value:?}"
                            )));
                        }
                    }
                }
                "sandbox_landlock" => c.sandbox_landlock = parse_bool(value).map_err(err)?,
                "sandbox_seccomp" => c.sandbox_seccomp = parse_bool(value).map_err(err)?,
                "sandbox_action" => {
                    c.sandbox_action = Action::parse(value)
                        .ok_or_else(|| err(format!("expected kill|errno|log, found {value:?}")))?;
                }
                "max_connections" => c.max_connections = parse_size(value).map_err(err)?,
                "read_timeout" => c.read_timeout = parse_duration(value).map_err(err)?,
                "write_timeout" => c.write_timeout = parse_duration(value).map_err(err)?,
                "keepalive_requests" => {
                    let n = parse_size(value).map_err(err)?;
                    c.keepalive_requests = u32::try_from(n)
                        .map_err(|_| err(format!("{n} is more than a connection can serve")))?;
                }
                "keepalive_timeout" => c.keepalive_timeout = parse_duration(value).map_err(err)?,
                "request_rate" => c.request_rate = parse_rate(value).map_err(err)?,
                "request_burst" => c.request_burst = parse_rate(value).map_err(err)?,
                "max_cluster_bytes" => c.max_cluster_bytes = parse_size(value).map_err(err)?,
                "cluster_cache_bytes" => c.cluster_cache_bytes = parse_size(value).map_err(err)?,
                "max_metadata_bytes" => c.max_metadata_bytes = parse_size(value).map_err(err)?,
                "max_metadata_entries" => {
                    c.max_metadata_entries = parse_size(value).map_err(err)?;
                }
                "max_request_line" => c.max_request_line = parse_size(value).map_err(err)?,
                "max_header_bytes" => c.max_header_bytes = parse_size(value).map_err(err)?,
                "max_headers" => c.max_headers = parse_size(value).map_err(err)?,
                "max_path_bytes" => c.max_path_bytes = parse_size(value).map_err(err)?,
                "suggest_max_query" => c.suggest_max_query = parse_size(value).map_err(err)?,
                "suggest_max_results" => c.suggest_max_results = parse_size(value).map_err(err)?,
                "content_security_policy" => value.clone_into(&mut c.content_security_policy),
                "log_level" => {
                    c.log_level = match value {
                        "error" => Level::Error,
                        "warn" => Level::Warn,
                        "info" => Level::Info,
                        "debug" => Level::Debug,
                        _ => {
                            return Err(err(format!(
                                "expected error|warn|info|debug, found {value:?}"
                            )));
                        }
                    }
                }
                "access_log" => c.access_log = parse_bool(value).map_err(err)?,
                other => return Err(err(format!("unknown key {other:?}"))),
            }
        }

        if let Some(path) = token_file {
            let token = std::fs::read_to_string(&path).map_err(|e| ConfigError {
                line: 0,
                message: format!("{}: {e}", path.display()),
            })?;
            c.auth_token = Some(token.trim().to_owned());
        }
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| ConfigError {
            line: 0,
            message: m.to_owned(),
        };
        if self.max_connections == 0 {
            return Err(bad("max_connections must be at least 1"));
        }
        if self.max_request_line == 0 || self.max_header_bytes == 0 || self.max_headers == 0 {
            return Err(bad("request limits must be non-zero"));
        }
        if self.max_cluster_bytes == 0 {
            return Err(bad("max_cluster_bytes must be non-zero"));
        }
        // A socket refuses a zero timeout, and the daemon answers that by
        // dropping the connection. Every client would be disconnected without
        // a byte and without a log line, so this is refused where it can still
        // name the key.
        for (key, value) in [
            ("read_timeout", self.read_timeout),
            ("write_timeout", self.write_timeout),
            ("keepalive_timeout", self.keepalive_timeout),
        ] {
            if value.is_zero() {
                return Err(bad(&format!("{key} must be non-zero")));
            }
        }
        if self
            .auth_token
            .as_ref()
            .is_some_and(std::string::String::is_empty)
        {
            return Err(bad("auth_token is empty"));
        }
        if !api::token::is_safe_header_value(&self.content_security_policy) {
            return Err(bad("content_security_policy contains a control byte"));
        }
        Ok(())
    }

    /// Limits handed to the HTTP layer.
    pub fn api_limits(&self) -> api::Limits {
        api::Limits {
            max_request_line: self.max_request_line,
            max_header_bytes: self.max_header_bytes,
            max_headers: self.max_headers,
            max_path_bytes: self.max_path_bytes,
            suggest_max_query: self.suggest_max_query,
            suggest_max_results: self.suggest_max_results,
        }
    }

    /// Limits handed to the archive layer.
    pub fn archive_limits(&self) -> archive::Limits {
        archive::Limits {
            max_cluster_bytes: self.max_cluster_bytes,
            cache_bytes: self.cluster_cache_bytes,
            max_metadata_bytes: self.max_metadata_bytes,
            max_metadata_entries: self.max_metadata_entries,
        }
    }

    /// The confinement policy this configuration asks for.
    pub fn sandbox_policy(&self) -> sandbox::Policy {
        sandbox::Policy {
            read_only: vec![self.archive_dir.clone()],
            require: self.sandbox == SandboxMode::Require,
            landlock: self.sandbox != SandboxMode::Off && self.sandbox_landlock,
            seccomp: self.sandbox != SandboxMode::Off && self.sandbox_seccomp,
            action: self.sandbox_action,
        }
    }
}

fn parse_listen(value: &str) -> Result<Listen, String> {
    if let Some(path) = value.strip_prefix("unix:") {
        if path.is_empty() {
            return Err("unix socket path is empty".into());
        }
        return Ok(Listen::Unix(PathBuf::from(path)));
    }
    if let Some(addr) = value.strip_prefix("tcp:") {
        // Parsed, never resolved: `to_socket_addrs` would consult the resolver,
        // and the scope says no DNS (§7.1). A hostname is refused here rather
        // than looked up.
        let addr: SocketAddr = addr
            .parse()
            .map_err(|_| format!("{addr:?} is not an IP address and port"))?;
        // TLS is the reverse proxy's job, so cairn never listens off-host.
        let loopback = match addr.ip() {
            IpAddr::V4(v4) => v4.is_loopback(),
            IpAddr::V6(v6) => v6.is_loopback(),
        };
        if !loopback {
            return Err(format!("{} is not a loopback address", addr.ip()));
        }
        return Ok(Listen::Tcp(addr));
    }
    Err(format!("expected unix:PATH or tcp:ADDR, found {value:?}"))
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "on" | "yes" | "true" | "1" => Ok(true),
        "off" | "no" | "false" | "0" => Ok(false),
        _ => Err(format!("expected on|off, found {value:?}")),
    }
}

/// Sizes accept `K`, `M` and `G` suffixes, decimal otherwise.
fn parse_size(value: &str) -> Result<usize, String> {
    let (digits, scale) = match value.as_bytes().last() {
        Some(b'K' | b'k') => (&value[..value.len() - 1], 1024),
        Some(b'M' | b'm') => (&value[..value.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    digits
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| format!("not a size: {value:?}"))
}

/// Durations accept `ms`, `s`, `m`; bare numbers are seconds.
fn parse_duration(value: &str) -> Result<Duration, String> {
    let bad = || format!("not a duration: {value:?}");
    if let Some(n) = value.strip_suffix("ms") {
        return Ok(Duration::from_millis(n.trim().parse().map_err(|_| bad())?));
    }
    if let Some(n) = value.strip_suffix('s') {
        return Ok(Duration::from_secs(n.trim().parse().map_err(|_| bad())?));
    }
    if let Some(n) = value.strip_suffix('m') {
        let m: u64 = n.trim().parse().map_err(|_| bad())?;
        return Ok(Duration::from_secs(m.checked_mul(60).ok_or_else(bad)?));
    }
    Ok(Duration::from_secs(value.parse().map_err(|_| bad())?))
}

fn parse_rate(value: &str) -> Result<f64, String> {
    let v: f64 = value
        .parse()
        .map_err(|_| format!("not a number: {value:?}"))?;
    if !v.is_finite() || v < 0.0 {
        return Err(format!("not a rate: {value:?}"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable() {
        let c = Config::parse("").unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.listen.to_string(), "unix:/run/cairn/cairn.sock");
    }

    #[test]
    fn parses_a_full_file() {
        let text = "
# where to listen
listen = tcp:127.0.0.1:8320   # loopback only
socket_mode = 0600
archive_dir = /srv/zim
sandbox = require
sandbox_action = errno
max_connections = 8
read_timeout = 500ms
write_timeout = 2m
cluster_cache_bytes = 16M
max_headers = 32
request_rate = 12.5
access_log = on
log_level = debug
";
        let c = Config::parse(text).unwrap();
        assert_eq!(c.listen, Listen::Tcp("127.0.0.1:8320".parse().unwrap()));
        assert_eq!(c.socket_mode, 0o600);
        assert_eq!(c.archive_dir, PathBuf::from("/srv/zim"));
        assert_eq!(c.sandbox, SandboxMode::Require);
        assert_eq!(c.sandbox_action, Action::Errno);
        assert_eq!(c.max_connections, 8);
        assert_eq!(c.read_timeout, Duration::from_millis(500));
        assert_eq!(c.write_timeout, Duration::from_secs(120));
        assert_eq!(c.cluster_cache_bytes, 16 * 1024 * 1024);
        assert_eq!(c.max_headers, 32);
        // `12.5` round-trips exactly; this asserts the parse, not float maths.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(c.request_rate, 12.5);
        }
        assert!(c.access_log);
        assert_eq!(c.log_level, Level::Debug);
        assert!(c.sandbox_policy().require);
    }

    #[test]
    fn rejects_typos_and_junk() {
        assert!(Config::parse("max_conections = 4").is_err());
        assert!(Config::parse("max_connections 4").is_err());
        assert!(Config::parse("max_connections = many").is_err());
        assert!(Config::parse("max_connections = 0").is_err());
        assert!(Config::parse("read_timeout = soon").is_err());
        assert!(Config::parse("sandbox = maybe").is_err());
        assert!(Config::parse("socket_mode = 999").is_err());
        assert!(Config::parse("auth_token =").is_err());
    }

    #[test]
    fn refuses_to_listen_off_host() {
        assert!(Config::parse("listen = tcp:0.0.0.0:8320").is_err());
        assert!(Config::parse("listen = tcp:192.0.2.1:8320").is_err());
        assert!(Config::parse("listen = tcp:[::1]:8320").is_ok());
        assert!(Config::parse("listen = http://localhost").is_err());
        assert!(Config::parse("listen = unix:").is_err());
    }

    #[test]
    fn a_hostname_is_refused_rather_than_resolved() {
        // "No DNS" is a stated guarantee, so a hostname here must fail at the
        // parse rather than reach the resolver. `localhost` is the one that
        // would otherwise resolve to a loopback address and be accepted.
        assert!(Config::parse("listen = tcp:localhost:8320").is_err());
        assert!(Config::parse("listen = tcp:example.invalid:8320").is_err());
        assert!(
            Config::parse("listen = tcp:127.0.0.1").is_err(),
            "port required"
        );
        assert!(Config::parse("listen = tcp:127.0.0.1:8320").is_ok());
    }

    #[test]
    fn a_keepalive_count_that_does_not_fit_is_an_error() {
        // The field is a u32; a larger value must be refused, not truncated
        // into a much smaller ceiling.
        let e = Config::parse("keepalive_requests = 4G").unwrap_err();
        assert_eq!(e.line, 1);
        assert!(e.to_string().contains("keepalive_requests"), "{e}");
        assert_eq!(
            Config::parse("keepalive_requests = 1000").map(|c| c.keepalive_requests),
            Ok(1000)
        );
    }

    #[test]
    fn an_error_names_the_line_and_the_key() {
        let text = "# a comment\nmax_connections = 8\n\nmax_headers = lots\n";
        let e = Config::parse(text).unwrap_err();
        assert_eq!(e.line, 4, "{e}");
        assert!(e.to_string().contains("max_headers"), "{e}");
        assert!(e.to_string().contains("lots"), "{e}");
    }

    #[test]
    fn refuses_a_header_smuggling_csp() {
        assert!(Config::parse("content_security_policy = default-src 'none'").is_ok());
        let sneaky = "content_security_policy = a\u{7f}b";
        assert!(Config::parse(sneaky).is_err());
    }

    /// A socket refuses `SO_RCVTIMEO` of zero, and a daemon that took the
    /// value anyway would accept every connection and drop it unanswered.
    #[test]
    fn a_zero_timeout_is_refused_rather_than_silently_dropping_connections() {
        for key in ["read_timeout", "write_timeout", "keepalive_timeout"] {
            let e = Config::parse(&format!("{key} = 0")).unwrap_err();
            assert!(e.to_string().contains(key), "{e}");
            assert!(Config::parse(&format!("{key} = 0ms")).is_err(), "{key}");
            assert!(Config::parse(&format!("{key} = 1ms")).is_ok(), "{key}");
        }
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(parse_size("1024"), Ok(1024));
        assert_eq!(parse_size("2K"), Ok(2048));
        assert_eq!(parse_size("3M"), Ok(3 * 1024 * 1024));
        assert!(parse_size("1T").is_err());
        assert_eq!(parse_duration("5"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("5s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("5ms"), Ok(Duration::from_millis(5)));
        assert!(parse_duration("5h").is_err());
    }
}
