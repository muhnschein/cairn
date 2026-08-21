use core::fmt;

/// Every way a ZIM archive can be wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read ran past the end of the file.
    Truncated {
        /// What was being read.
        what: &'static str,
        /// Bytes the read needed.
        need: u64,
        /// Bytes the file has.
        have: u64,
    },
    /// Magic number is not [`crate::MAGIC`].
    BadMagic(u32),
    /// Major version outside the supported range.
    UnsupportedVersion {
        /// Header major version.
        major: u16,
        /// Header minor version.
        minor: u16,
    },
    /// The header UUID is all zeroes.
    NilUuid,
    /// A declared region does not fit in the file.
    Region {
        /// What the region holds.
        what: &'static str,
        /// Declared start offset.
        at: u64,
        /// Declared length in bytes.
        bytes: u64,
        /// End of the file the offsets are checked against.
        data_end: u64,
    },
    /// MIME table unterminated, oversized, or truncated.
    MimeList,
    /// Entry index past `entry_count`.
    EntryIndex(u32),
    /// Cluster index past `cluster_count`.
    ClusterIndex(u32),
    /// Blob index past the cluster's blob count.
    BlobIndex(u32),
    /// Malformed directory entry.
    Dirent(&'static str),
    /// Malformed cluster.
    Cluster(&'static str),
    /// Compression algorithm not supported (obsolete zlib/bzip2, or unknown).
    UnsupportedCompression(u8),
    /// Decompression refused the input.
    Decompress(&'static str),
    /// Decompressed output would exceed the configured bound.
    TooLarge {
        /// The bound that was exceeded.
        limit: usize,
    },
    /// Redirect chain longer than the allowed depth.
    RedirectDepth,
    /// Asked for content of a redirect or of a deleted entry.
    NotContent,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, need, have } => {
                write!(f, "truncated: {what} needs {need} bytes, file has {have}")
            }
            Error::BadMagic(m) => write!(f, "bad magic number {m:#x}"),
            Error::UnsupportedVersion { major, minor } => {
                write!(f, "unsupported ZIM version {major}.{minor}")
            }
            Error::NilUuid => write!(f, "nil archive UUID"),
            Error::Region {
                what,
                at,
                bytes,
                data_end,
            } => write!(
                f,
                "{what} starts at {at} and spans {bytes} bytes, past the end of archive data at {data_end}"
            ),
            Error::MimeList => write!(f, "malformed MIME type table"),
            Error::EntryIndex(i) => write!(f, "entry index {i} out of range"),
            Error::ClusterIndex(i) => write!(f, "cluster index {i} out of range"),
            Error::BlobIndex(i) => write!(f, "blob index {i} out of range"),
            Error::Dirent(why) => write!(f, "malformed directory entry: {why}"),
            Error::Cluster(why) => write!(f, "malformed cluster: {why}"),
            Error::UnsupportedCompression(k) => write!(f, "unsupported cluster compression {k}"),
            Error::Decompress(why) => write!(f, "decompression failed: {why}"),
            Error::TooLarge { limit } => write!(f, "output exceeds the {limit} byte bound"),
            Error::RedirectDepth => write!(f, "redirect chain too long"),
            Error::NotContent => write!(f, "entry holds no content"),
        }
    }
}

impl std::error::Error for Error {}

/// Shorthand for parser results.
pub type Result<T> = core::result::Result<T, Error>;
