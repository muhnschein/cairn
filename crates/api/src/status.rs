//! What `/v1/status` reports.
//!
//! A daemon that failed to confine itself otherwise looks identical to one
//! that succeeded, so the numbers here are gathered by the daemon and passed
//! in, never guessed.

/// State of one confinement layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// `no_new_privs`, `landlock`, `seccomp`.
    pub name: String,
    /// `applied`, `unsupported`, `failed`, or `disabled`.
    pub state: String,
    /// ABI version, filter action, or the reason it is not applied.
    pub detail: Option<String>,
}

/// Confinement as actually applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sandbox {
    /// True when `sandbox require` is set.
    pub required: bool,
    /// One entry per layer, in the order they were applied.
    pub layers: Vec<Layer>,
}

/// Cluster cache counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cache {
    /// Ceiling from `cluster_cache_bytes`.
    pub budget_bytes: u64,
    /// Decompressed bytes currently held.
    pub bytes: u64,
    /// Clusters currently held.
    pub entries: u64,
    /// Requests served from the cache.
    pub hits: u64,
    /// Requests that had to decode.
    pub misses: u64,
    /// Clusters dropped to stay inside the budget.
    pub evictions: u64,
}

/// Connection counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Connections {
    /// Size of the worker pool, which is the connection ceiling.
    pub max: u64,
    /// Connections being served right now.
    pub active: u64,
    /// Connections accepted since startup.
    pub served: u64,
    /// Connections refused since startup.
    pub rejected: u64,
}

/// The whole status document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// Release name, `YYYY.0M`.
    pub version: String,
    /// Seconds since the daemon finished starting.
    pub uptime_seconds: u64,
    /// The listen address, as configured.
    pub listener: String,
    /// Archives opened at startup.
    pub archive_count: u64,
    /// Whether a bearer token is required.
    pub auth_required: bool,
    /// Confinement as actually applied.
    pub sandbox: Sandbox,
    /// Cluster cache counters.
    pub cache: Cache,
    /// Connection counters.
    pub connections: Connections,
}
