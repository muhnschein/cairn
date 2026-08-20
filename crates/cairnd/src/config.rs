//! `cairn.conf` parsing.
//!
//! `key = value`, one per line, `#` to end of line is a comment. Unknown keys
//! are errors: a typo in a limit must not silently leave it at the default.

use std::fmt;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
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
    Error,
    Warn,
    Info,
    Debug,
}

/// The daemon's whole configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub listen: Listen,
    pub socket_mode: u32,
    pub archive_dir: PathBuf,
    pub auth_token: Option<String>,
    pub sandbox: SandboxMode,
    pub sandbox_landlock: bool,
    pub sandbox_seccomp: bool,
    pub sandbox_action: Action,
    pub max_connections: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub keepalive_requests: u32,
    pub keepalive_timeout: Duration,
    pub request_rate: f64,
    pub request_burst: f64,
    pub max_cluster_bytes: usize,
    pub cluster_cache_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_metadata_entries: usize,
    pub max_request_line: usize,
    pub max_header_bytes: usize,
    pub max_headers: usize,
    pub max_path_bytes: usize,
    pub suggest_max_query: usize,
    pub suggest_max_results: usize,
    pub content_security_policy: String,
    pub log_level: Level,
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
    pub line: usize,
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
            let err = |m: String| ConfigError {
                line: n,
                message: m,
            };

            match key {
                "listen" => c.listen = parse_listen(value).map_err(err)?,
                "socket_mode" => {
                    c.socket_mode = u32::from_str_radix(value.trim_start_matches("0o"), 8)
                        .map_err(|_| err(format!("not an octal mode: {value:?}")))?
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
                        .ok_or_else(|| err(format!("expected kill|errno|log, found {value:?}")))?
                }
                "max_connections" => c.max_connections = parse_size(value).map_err(err)?,
                "read_timeout" => c.read_timeout = parse_duration(value).map_err(err)?,
                "write_timeout" => c.write_timeout = parse_duration(value).map_err(err)?,
                "keepalive_requests" => {
                    c.keepalive_requests = parse_size(value).map_err(err)? as u32
                }
                "keepalive_timeout" => c.keepalive_timeout = parse_duration(value).map_err(err)?,
                "request_rate" => c.request_rate = parse_rate(value).map_err(err)?,
                "request_burst" => c.request_burst = parse_rate(value).map_err(err)?,
                "max_cluster_bytes" => c.max_cluster_bytes = parse_size(value).map_err(err)?,
                "cluster_cache_bytes" => c.cluster_cache_bytes = parse_size(value).map_err(err)?,
                "max_metadata_bytes" => c.max_metadata_bytes = parse_size(value).map_err(err)?,
                "max_metadata_entries" => {
                    c.max_metadata_entries = parse_size(value).map_err(err)?
                }
                "max_request_line" => c.max_request_line = parse_size(value).map_err(err)?,
                "max_header_bytes" => c.max_header_bytes = parse_size(value).map_err(err)?,
                "max_headers" => c.max_headers = parse_size(value).map_err(err)?,
                "max_path_bytes" => c.max_path_bytes = parse_size(value).map_err(err)?,
                "suggest_max_query" => c.suggest_max_query = parse_size(value).map_err(err)?,
                "suggest_max_results" => c.suggest_max_results = parse_size(value).map_err(err)?,
                "content_security_policy" => c.content_security_policy = value.to_owned(),
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
        if self.auth_token.as_ref().is_some_and(|t| t.is_empty()) {
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
        let mut resolved = addr
            .to_socket_addrs()
            .map_err(|e| format!("{addr:?}: {e}"))?
            .collect::<Vec<_>>();
        let addr = resolved
            .pop()
            .ok_or_else(|| format!("{addr:?} resolved to nothing"))?;
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
        assert_eq!(c.request_rate, 12.5);
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
    fn refuses_a_header_smuggling_csp() {
        assert!(Config::parse("content_security_policy = default-src 'none'").is_ok());
        let sneaky = "content_security_policy = a\u{7f}b";
        assert!(Config::parse(sneaky).is_err());
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
