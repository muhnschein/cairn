use std::fmt;
use std::path::PathBuf;

use zimfmt::Uuid;

/// Why an archive could not be opened. Startup errors, never request errors.
#[derive(Debug)]
pub enum OpenError {
    /// The file could not be opened or mapped.
    Io {
        /// The file that failed.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// The file is not a ZIM archive cairn can serve.
    Format {
        /// The file that failed.
        path: PathBuf,
        /// What the parser said.
        source: zimfmt::Error,
    },
    /// Two files carry the same archive UUID.
    DuplicateUuid {
        /// The uuid both files claim.
        uuid: Uuid,
        /// First file seen with it.
        first: PathBuf,
        /// Second file seen with it.
        second: PathBuf,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            OpenError::Format { path, source } => write!(f, "{}: {source}", path.display()),
            OpenError::DuplicateUuid {
                uuid,
                first,
                second,
            } => write!(
                f,
                "duplicate archive UUID {uuid}: {} and {}",
                first.display(),
                second.display()
            ),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why a request could not be answered from an open archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// No archive with that UUID is open.
    NoSuchArchive,
    /// No entry at that path.
    NoSuchEntry,
    /// The archive region backing this answer is malformed.
    Corrupt(zimfmt::Error),
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NoSuchArchive => write!(f, "no such archive"),
            LookupError::NoSuchEntry => write!(f, "no such entry"),
            LookupError::Corrupt(e) => write!(f, "malformed archive: {e}"),
        }
    }
}

impl std::error::Error for LookupError {}

impl From<zimfmt::Error> for LookupError {
    fn from(e: zimfmt::Error) -> Self {
        LookupError::Corrupt(e)
    }
}
