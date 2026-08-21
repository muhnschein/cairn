//! Self-restriction: Landlock, seccomp, and an honest report of what was
//! applied.
//!
//! No dependency on the rest of the workspace. A daemon that failed to confine
//! itself must not look like one that succeeded, so every layer records its
//! outcome and `/v1/status` publishes it.

use std::path::{Path, PathBuf};

pub mod landlock;
pub mod seccomp;

pub use seccomp::Action;

/// Outcome of one layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// In force.
    Applied,
    /// The kernel does not offer it.
    Unsupported,
    /// The kernel offers it and refused.
    Failed,
    /// Turned off by configuration.
    Disabled,
}

impl State {
    /// Name used in `/v1/status`.
    pub fn name(&self) -> &'static str {
        match self {
            State::Applied => "applied",
            State::Unsupported => "unsupported",
            State::Failed => "failed",
            State::Disabled => "disabled",
        }
    }
}

/// One layer's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// `no_new_privs`, `landlock`, or `seccomp`.
    pub name: &'static str,
    /// Whether it applied, and if not, why not.
    pub state: State,
    /// ABI version, filter shape, or the reason it is missing.
    pub detail: Option<String>,
}

impl Layer {
    fn new(name: &'static str, state: State, detail: Option<String>) -> Layer {
        Layer {
            name,
            state,
            detail,
        }
    }
}

/// What to confine, and how strictly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// Directories that stay readable. Everything else becomes unreachable.
    pub read_only: Vec<PathBuf>,
    /// Refuse to run unless every layer applies.
    pub require: bool,
    /// Skip Landlock. For kernels without it, when `require` is off.
    pub landlock: bool,
    /// Skip seccomp.
    pub seccomp: bool,
    /// What the filter does with a syscall outside the allowlist.
    pub action: Action,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            read_only: Vec::new(),
            require: false,
            landlock: true,
            seccomp: true,
            action: Action::Kill,
        }
    }
}

/// What was actually applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Whether `sandbox = require` was set.
    pub required: bool,
    /// One entry per layer, in the order they were applied.
    pub layers: Vec<Layer>,
}

impl Report {
    /// True when every enabled layer is in force.
    pub fn complete(&self) -> bool {
        self.layers
            .iter()
            .all(|l| matches!(l.state, State::Applied | State::Disabled))
    }

    /// Layers that are neither applied nor deliberately disabled.
    pub fn shortfall(&self) -> Vec<&Layer> {
        self.layers
            .iter()
            .filter(|l| !matches!(l.state, State::Applied | State::Disabled))
            .collect()
    }
}

/// Refusal to start, raised when `require` is set and a layer did not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incomplete {
    /// The layers that did not apply.
    pub layers: Vec<Layer>,
}

impl std::fmt::Display for Incomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sandbox incomplete:")?;
        for l in &self.layers {
            write!(f, " {}={}", l.name, l.state.name())?;
            if let Some(d) = &l.detail {
                write!(f, " ({d})")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Incomplete {}

/// Confine this process.
///
/// Order matters: Landlock needs to open the archive directory, so it runs
/// before the filter that denies `openat`; `no_new_privs` precedes both.
/// Call after archives are open, the listener is bound, and workers exist.
pub fn apply(policy: &Policy) -> Result<Report, Incomplete> {
    let mut layers = Vec::new();

    layers.push(match seccomp::set_no_new_privs() {
        Ok(()) => Layer::new("no_new_privs", State::Applied, None),
        Err(e) => Layer::new("no_new_privs", State::Failed, Some(e.to_string())),
    });

    layers.push(if policy.landlock {
        let paths: Vec<&Path> = policy.read_only.iter().map(PathBuf::as_path).collect();
        match landlock::abi_version() {
            Err(e) => Layer::new("landlock", State::Unsupported, Some(e.to_string())),
            Ok(abi) => match landlock::restrict(&paths, abi) {
                Ok(abi) => Layer::new("landlock", State::Applied, Some(format!("abi {abi}"))),
                Err(e) => Layer::new("landlock", State::Failed, Some(e.to_string())),
            },
        }
    } else {
        Layer::new("landlock", State::Disabled, None)
    });

    layers.push(if policy.seccomp {
        let allowed = seccomp::allowed_syscalls();
        match seccomp::install(&allowed, policy.action) {
            Ok(len) => Layer::new(
                "seccomp",
                State::Applied,
                Some(format!(
                    "{} syscalls, {} denied, {} on violation, {len} instructions",
                    allowed.len(),
                    seccomp::denied_syscalls().len(),
                    policy.action
                )),
            ),
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
                Layer::new("seccomp", State::Unsupported, Some(e.to_string()))
            }
            Err(e) => Layer::new("seccomp", State::Failed, Some(e.to_string())),
        }
    } else {
        Layer::new("seccomp", State::Disabled, None)
    });

    let report = Report {
        required: policy.require,
        layers,
    };
    if policy.require && !report.complete() {
        return Err(Incomplete {
            layers: report.shortfall().into_iter().cloned().collect(),
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_completeness() {
        let ok = Report {
            required: true,
            layers: vec![
                Layer::new("a", State::Applied, None),
                Layer::new("b", State::Disabled, None),
            ],
        };
        assert!(ok.complete());
        assert!(ok.shortfall().is_empty());

        let bad = Report {
            required: true,
            layers: vec![
                Layer::new("a", State::Applied, None),
                Layer::new("b", State::Unsupported, Some("old kernel".into())),
            ],
        };
        assert!(!bad.complete());
        assert_eq!(bad.shortfall().len(), 1);
        assert!(
            Incomplete {
                layers: bad.shortfall().into_iter().cloned().collect()
            }
            .to_string()
            .contains("b=unsupported")
        );
    }
}
