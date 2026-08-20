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
    pub layers: Vec<Layer>,
}

/// Cluster cache counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cache {
    pub budget_bytes: u64,
    pub bytes: u64,
    pub entries: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// Connection counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Connections {
    pub max: u64,
    pub active: u64,
    pub served: u64,
    pub rejected: u64,
}

/// The whole status document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub version: String,
    pub uptime_seconds: u64,
    pub listener: String,
    pub archive_count: u64,
    pub auth_required: bool,
    pub sandbox: Sandbox,
    pub cache: Cache,
    pub connections: Connections,
}
