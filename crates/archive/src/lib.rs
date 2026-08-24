//! Opening and holding ZIM archives.
//!
//! Owns the mmap lifetime, the UUID index, redirect resolution and the cluster
//! cache. Bounds checks above [`zimfmt`] live here; HTTP does not.

pub mod cache;
mod error;

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memmap2::{Advice, Mmap};
use zimfmt::{Cluster, Layout, Target, TitleIndex, Uuid, Zim};

pub use cache::{ClusterCache, Stats};
pub use error::{LookupError, OpenError};
/// Re-exported so callers can name the error inside [`LookupError::Corrupt`]
/// and [`OpenError::Format`] without depending on the parser directly.
pub use zimfmt;

/// Bounds that apply to every archive in a catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Ceiling on one decompressed cluster.
    pub max_cluster_bytes: usize,
    /// Global cluster cache budget.
    pub cache_bytes: usize,
    /// Ceiling on one metadata value.
    pub max_metadata_bytes: usize,
    /// Ceiling on the number of metadata entries reported.
    pub max_metadata_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_cluster_bytes: 32 * 1024 * 1024,
            cache_bytes: 64 * 1024 * 1024,
            max_metadata_bytes: 8 * 1024,
            max_metadata_entries: 64,
        }
    }
}

/// Anything that can hand out a byte slice for the lifetime of an `Arc`.
///
/// Either the mapped archive or a cached cluster body, so entry content is
/// served without copying it again.
pub type Bytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// A byte range inside something kept alive by an `Arc`.
#[derive(Clone)]
pub struct Blob {
    /// Backing bytes: the mapped file, or a decoded cluster.
    pub data: Bytes,
    /// Start of the blob within `data`.
    pub start: usize,
    /// End of the blob within `data`.
    pub end: usize,
}

impl Blob {
    /// The blob's bytes, empty if the range no longer fits its backing store.
    pub fn as_slice(&self) -> &[u8] {
        (*self.data)
            .as_ref()
            .get(self.start..self.end)
            .unwrap_or(&[])
    }

    /// Blob length in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// True for a zero-length blob.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blob").field("len", &self.len()).finish()
    }
}

/// An entry's content plus what it resolved to.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path after redirect following.
    pub path: String,
    /// MIME type as stored in the archive, unvalidated.
    pub mime: String,
    /// Entry content.
    pub blob: Blob,
}

/// What `/v1/archives` reports per archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Archive identity from the header.
    pub uuid: Uuid,
    /// Archive title, as stored.
    pub title: String,
    /// Entries in the archive, redirects included.
    pub entry_count: u32,
    /// Clusters in the archive.
    pub cluster_count: u32,
    /// Path of the archive's main page, when it declares one.
    pub main_page: Option<String>,
    /// ZIM format major version.
    pub major_version: u16,
    /// ZIM format minor version.
    pub minor_version: u16,
    /// Namespace holding content: `C` in modern archives, `A` in older ones.
    pub content_namespace: char,
    /// True when the archive carries a title ordering to suggest from.
    pub has_title_index: bool,
}

/// One title-prefix suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Title as stored, which is what the prefix matched.
    pub title: String,
    /// Path of the entry with that title.
    pub path: String,
}

/// Metadata read from the `M` namespace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Name/value pairs whose value is text within the size bound.
    pub text: Vec<(String, String)>,
    /// Names whose value was binary or oversized.
    pub binary: Vec<String>,
}

/// One open archive: a mapping, its parsed layout, and its identity.
pub struct Archive {
    path: PathBuf,
    map: Mmap,
    layout: Layout,
    title: String,
    /// Resolved once at open, as libzim does: the listing entry if there is
    /// one, else the header's list, else nothing to suggest from.
    title_index: Option<TitleIndex>,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("uuid", &self.uuid())
            .finish_non_exhaustive()
    }
}

impl AsRef<[u8]> for Archive {
    fn as_ref(&self) -> &[u8] {
        &self.map
    }
}

impl Archive {
    /// Map an archive read-only and parse its layout.
    ///
    /// The mapping lives as long as the `Archive`. Replacing or truncating the
    /// file underneath a running daemon faults on access; see `cairnd(8)`.
    pub fn open(path: &Path, limits: &Limits) -> Result<Archive, OpenError> {
        let file = File::open(path).map_err(|e| OpenError::Io {
            path: path.into(),
            source: e,
        })?;
        // SAFETY: the mapping is read-only and the file is required to be
        // immutable for the daemon's lifetime; that constraint is documented in
        // cairnd(8) and enforced in deployment by a read-only mount. A file
        // changed anyway faults with SIGBUS, which cairn does not catch.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| OpenError::Io {
            path: path.into(),
            source: e,
        })?;
        // Entry reads are scattered; readahead would only evict useful pages.
        let _ = map.advise(Advice::Random);
        let layout = Layout::parse(&map).map_err(|e| OpenError::Format {
            path: path.into(),
            source: e,
        })?;
        let mut archive = Archive {
            path: path.into(),
            map,
            layout,
            title: String::new(),
            title_index: None,
        };
        archive.title = archive.read_title(limits);
        // A malformed listing costs suggestion, not the archive.
        archive.title_index = archive.zim().title_index().unwrap_or(None);
        Ok(archive)
    }

    /// Path this archive was opened from. Logged, never served.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Archive identity, from the header.
    pub fn uuid(&self) -> Uuid {
        self.layout.header().uuid
    }

    /// A parser view over the mapping.
    pub fn zim(&self) -> Zim<'_> {
        Zim::new(&self.map, &self.layout)
    }

    /// Namespace holding content in this archive.
    pub fn content_namespace(&self) -> u8 {
        self.layout.header().content_namespace()
    }

    /// Listing entry for this archive.
    pub fn summary(&self) -> Summary {
        let h = self.layout.header();
        Summary {
            uuid: h.uuid,
            title: self.title.clone(),
            entry_count: h.entry_count,
            cluster_count: h.cluster_count,
            main_page: h.main_page().and_then(|i| self.path_of(i).ok()),
            major_version: h.major_version,
            minor_version: h.minor_version,
            content_namespace: self.content_namespace() as char,
            has_title_index: self.has_title_index(),
        }
    }

    /// Resolve an API path to an entry index, following redirects.
    ///
    /// The content namespace is tried first, so an entry really named `A/x`
    /// wins over namespace `A` entry `x`. An explicit `N/path` form is tried
    /// second, which is what old-scheme cross-namespace links look like.
    pub fn find(&self, path: &str) -> Result<u32, LookupError> {
        let zim = self.zim();
        if let Some(i) = zim.find(self.content_namespace(), path.as_bytes())? {
            return Ok(i);
        }
        if let Some((ns, rest)) = split_namespace(path)
            && let Some(i) = zim.find(ns, rest.as_bytes())?
        {
            return Ok(i);
        }
        Err(LookupError::NoSuchEntry)
    }

    /// The API path of an entry index.
    pub fn path_of(&self, index: u32) -> Result<String, LookupError> {
        let d = self.zim().dirent(index)?;
        let url = String::from_utf8_lossy(d.url).into_owned();
        Ok(if d.namespace == self.content_namespace() {
            url
        } else {
            format!("{}/{}", d.namespace as char, url)
        })
    }

    /// True when this archive can answer suggestions at all.
    pub fn has_title_index(&self) -> bool {
        self.title_index.is_some()
    }

    /// Title-prefix suggestions, byte-exact on the stored title.
    ///
    /// Empty when the archive carries no title ordering; `has_title_index`
    /// reports that, so a client is not left guessing why.
    pub fn suggest(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>, LookupError> {
        let Some(index) = &self.title_index else {
            return Ok(Vec::new());
        };
        let zim = self.zim();
        let ns = self.content_namespace();
        let start = zim.title_lower_bound(index, ns, prefix.as_bytes())?;
        let mut out = Vec::new();
        let mut position = start;
        while out.len() < limit && position < index.count() {
            let index = zim.title_entry(index, position)?;
            let d = zim.dirent(index)?;
            if d.namespace != ns || !d.effective_title().starts_with(prefix.as_bytes()) {
                break;
            }
            out.push(Suggestion {
                title: String::from_utf8_lossy(d.effective_title()).into_owned(),
                path: self.path_of(index)?,
            });
            position += 1;
        }
        Ok(out)
    }

    /// Entry index range of the content namespace, in URL order.
    pub fn content_range(&self) -> Result<(u32, u32), LookupError> {
        let zim = self.zim();
        let ns = self.content_namespace();
        let lo = zim.url_lower_bound(ns, b"")?;
        let hi = zim.url_lower_bound(ns + 1, b"")?;
        Ok((lo, hi.max(lo)))
    }

    /// Path of a random content entry, chosen from `pick`.
    pub fn random(&self, pick: u64) -> Result<String, LookupError> {
        let (lo, hi) = self.content_range()?;
        if lo >= hi {
            return Err(LookupError::NoSuchEntry);
        }
        let span = u64::from(hi - lo);
        // A redirect is a legitimate answer, but a broken one is not; a few
        // draws from the same seed keep this bounded.
        let mut seed = pick;
        for _ in 0..4 {
            // `seed % span < span <= u32::MAX`, so the narrowing loses nothing.
            #[allow(clippy::cast_possible_truncation)]
            let index = lo + (seed % span) as u32;
            if let Ok(target) = self.zim().resolve(index)
                && let Ok(path) = self.path_of(target)
            {
                return Ok(path);
            }
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
        }
        Err(LookupError::NoSuchEntry)
    }

    /// Text and binary metadata from the `M` namespace.
    pub fn metadata(&self, limits: &Limits) -> Result<Metadata, LookupError> {
        let zim = self.zim();
        let lo = zim.url_lower_bound(b'M', b"")?;
        let mut out = Metadata::default();
        let mut position = lo;
        let mut decoded = None;
        while position < zim.entry_count()
            && out.text.len() + out.binary.len() < limits.max_metadata_entries
        {
            let d = zim.dirent(position)?;
            if d.namespace != b'M' {
                break;
            }
            let name = String::from_utf8_lossy(d.url).into_owned();
            match read_blob(zim, &mut decoded, position, limits) {
                Ok(bytes) if bytes.len() <= limits.max_metadata_bytes => {
                    match std::str::from_utf8(&bytes) {
                        Ok(v) if !v.contains('\0') => out.text.push((name, v.to_owned())),
                        _ => out.binary.push(name),
                    }
                }
                _ => out.binary.push(name),
            }
            position += 1;
        }
        Ok(out)
    }

    /// Read one entry's blob without touching the cache. Startup only.
    fn read_blob_uncached(&self, index: u32, limits: &Limits) -> Result<Vec<u8>, LookupError> {
        read_blob(self.zim(), &mut None, index, limits)
    }

    fn read_title(&self, limits: &Limits) -> String {
        let Ok(Some(index)) = self.zim().find(b'M', b"Title") else {
            return String::new();
        };
        match self.read_blob_uncached(index, limits) {
            Ok(b) if b.len() <= limits.max_metadata_bytes => {
                String::from_utf8_lossy(&b).trim().to_owned()
            }
            _ => String::new(),
        }
    }
}

/// Read one entry's blob, reusing `decoded` when it already holds the cluster.
///
/// Nothing here touches the catalog's cluster cache: this is the startup and
/// `M` namespace path, and a metadata scan has no business evicting the
/// clusters that are serving content. But libzim packs the whole `M` namespace
/// into one cluster, so decoding per entry would decompress the same cluster
/// once for every key — up to `max_metadata_entries` times for one cheap
/// request. Holding the last one turns that back into a single decode, and it
/// is dropped when the scan is.
fn read_blob<'a>(
    zim: Zim<'a>,
    decoded: &mut Option<(u32, Cluster<'a>)>,
    index: u32,
    limits: &Limits,
) -> Result<Vec<u8>, LookupError> {
    let d = zim.dirent(zim.resolve(index)?)?;
    let Target::Content { cluster, blob } = d.target else {
        return Err(LookupError::NoSuchEntry);
    };
    if !matches!(decoded.as_ref(), Some((at, _)) if *at == cluster) {
        // Dropped before the next is decoded, so a scan holds one at a time.
        *decoded = None;
        *decoded = Some((cluster, zim.cluster(cluster, limits.max_cluster_bytes)?));
    }
    match decoded.as_ref() {
        Some((_, c)) => Ok(c.blob(blob)?.to_vec()),
        // Unreachable: the branch above filled it or returned.
        None => Err(LookupError::NoSuchEntry),
    }
}

/// Split an explicit `N/path` namespace prefix.
fn split_namespace(path: &str) -> Option<(u8, &str)> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b'/' && (b[0].is_ascii_alphanumeric() || b[0] == b'-') {
        Some((b[0], &path[2..]))
    } else {
        None
    }
}

/// Every open archive, indexed by UUID, sharing one cluster cache.
#[derive(Debug)]
pub struct Catalog {
    archives: Vec<Arc<Archive>>,
    index: HashMap<Uuid, usize>,
    cache: ClusterCache,
    limits: Limits,
}

impl Catalog {
    /// Open every `*.zim` file directly inside `dir`.
    ///
    /// Subdirectories are not descended. A duplicate UUID names both files and
    /// fails the whole catalog rather than picking one silently.
    pub fn open_dir(dir: &Path, limits: Limits) -> Result<Catalog, OpenError> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| OpenError::Io {
                path: dir.into(),
                source: e,
            })?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zim")))
            .collect();
        paths.sort();

        let mut archives = Vec::new();
        let mut index: HashMap<Uuid, usize> = HashMap::new();
        for path in paths {
            let archive = Archive::open(&path, &limits)?;
            let uuid = archive.uuid();
            if let Some(&first) = index.get(&uuid) {
                let first: &Arc<Archive> = &archives[first];
                return Err(OpenError::DuplicateUuid {
                    uuid,
                    first: first.path().to_path_buf(),
                    second: path,
                });
            }
            index.insert(uuid, archives.len());
            archives.push(Arc::new(archive));
        }
        let cache = ClusterCache::new(limits.cache_bytes);
        Ok(Catalog {
            archives,
            index,
            cache,
            limits,
        })
    }

    /// A catalog over already-open archives. Used by tests.
    pub fn from_archives(archives: Vec<Archive>, limits: Limits) -> Result<Catalog, OpenError> {
        let mut index: HashMap<Uuid, usize> = HashMap::new();
        let mut out: Vec<Arc<Archive>> = Vec::new();
        for archive in archives {
            let uuid = archive.uuid();
            if let Some(&first) = index.get(&uuid) {
                return Err(OpenError::DuplicateUuid {
                    uuid,
                    first: out[first].path().to_path_buf(),
                    second: archive.path().to_path_buf(),
                });
            }
            index.insert(uuid, out.len());
            out.push(Arc::new(archive));
        }
        let cache = ClusterCache::new(limits.cache_bytes);
        Ok(Catalog {
            archives: out,
            index,
            cache,
            limits,
        })
    }

    /// Open archives, in the order they were opened.
    pub fn archives(&self) -> &[Arc<Archive>] {
        &self.archives
    }

    /// Limits in force.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Cluster cache counters.
    pub fn cache_stats(&self) -> Stats {
        self.cache.stats()
    }

    fn lookup(&self, uuid: &str) -> Result<(usize, &Arc<Archive>), LookupError> {
        let uuid = Uuid::parse(uuid).ok_or(LookupError::NoSuchArchive)?;
        let i = *self.index.get(&uuid).ok_or(LookupError::NoSuchArchive)?;
        Ok((i, &self.archives[i]))
    }

    /// Listing entry for one archive.
    pub fn summary(&self, uuid: &str) -> Result<Summary, LookupError> {
        Ok(self.lookup(uuid)?.1.summary())
    }

    /// Metadata for one archive.
    pub fn metadata(&self, uuid: &str) -> Result<Metadata, LookupError> {
        self.lookup(uuid)?.1.metadata(&self.limits)
    }

    /// Title-prefix suggestions for one archive.
    pub fn suggest(
        &self,
        uuid: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>, LookupError> {
        self.lookup(uuid)?.1.suggest(prefix, limit)
    }

    /// A random content path from one archive.
    pub fn random(&self, uuid: &str, pick: u64) -> Result<String, LookupError> {
        self.lookup(uuid)?.1.random(pick)
    }

    /// Fetch an entry, following redirects and using the cluster cache.
    pub fn entry(&self, uuid: &str, path: &str) -> Result<Entry, LookupError> {
        let (slot, archive) = self.lookup(uuid)?;
        let index = archive.find(path)?;
        let target = archive.zim().resolve(index)?;
        let d = archive.zim().dirent(target)?;
        let Target::Content { cluster, blob } = d.target else {
            return Err(LookupError::NoSuchEntry);
        };
        let mime = archive
            .zim()
            .mime(d.mime_index)
            .map(|m| String::from_utf8_lossy(m).into_owned())
            .unwrap_or_default();
        let blob = self.blob(slot, archive, cluster, blob)?;
        Ok(Entry {
            path: archive.path_of(target)?,
            mime,
            blob,
        })
    }

    fn blob(
        &self,
        slot: usize,
        archive: &Arc<Archive>,
        cluster: u32,
        blob: u32,
    ) -> Result<Blob, LookupError> {
        let zim = archive.zim();
        let (compression, offset_size) = zim.cluster_shape(cluster)?;

        if zimfmt::decompress::is_uncompressed(compression) {
            // Stored plain: serve straight out of the mapping, no copy, no cache.
            let (start, _) = zim.cluster_extent(cluster)?;
            let c = zim.cluster(cluster, self.limits.max_cluster_bytes)?;
            let (s, e) = c.blob_range(blob)?;
            let base = start + 1;
            return Ok(Blob {
                data: Arc::clone(archive) as Bytes,
                start: base + s,
                end: base + e,
            });
        }

        let key = (slot, cluster);
        let body = if let Some((body, _)) = self.cache.get(key) {
            body
        } else {
            // Decoding outside the lock can duplicate work under a race; a
            // lock held across decompression would serialize every worker.
            let decoded = zim
                .cluster(cluster, self.limits.max_cluster_bytes)?
                .into_body();
            self.cache.insert(key, decoded, offset_size)
        };
        let c = Cluster::from_cached(&body, offset_size)?;
        let (s, e) = c.blob_range(blob)?;
        Ok(Blob {
            data: body as Bytes,
            start: s,
            end: e,
        })
    }
}
