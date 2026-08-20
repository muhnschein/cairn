use core::cmp::Ordering;

use crate::bytes::{to_usize, u32le, u64le};
use crate::cluster::{Cluster, compression_of, offset_size_of};
use crate::dirent::Dirent;
use crate::error::{Error, Result};
use crate::header::Header;
use crate::layout::Layout;

/// How far a redirect chain is followed before it is abandoned.
pub const MAX_REDIRECT_HOPS: u32 = 4;

/// A read-only view over archive bytes plus its parsed [`Layout`].
///
/// Cheap to construct: nothing is re-parsed here.
#[derive(Debug, Clone, Copy)]
pub struct Zim<'a> {
    bytes: &'a [u8],
    layout: &'a Layout,
}

impl<'a> Zim<'a> {
    /// Pair archive bytes with the layout parsed from them.
    pub fn new(bytes: &'a [u8], layout: &'a Layout) -> Zim<'a> {
        Zim { bytes, layout }
    }

    /// The header.
    pub fn header(&self) -> &'a Header {
        self.layout.header()
    }

    /// The layout.
    pub fn layout(&self) -> &'a Layout {
        self.layout
    }

    /// Number of directory entries.
    pub fn entry_count(&self) -> u32 {
        self.header().entry_count
    }

    /// Number of clusters.
    pub fn cluster_count(&self) -> u32 {
        self.header().cluster_count
    }

    /// MIME type string for a dirent's `mime_index`.
    pub fn mime(&self, index: u16) -> Option<&'a [u8]> {
        self.layout.mime(index)
    }

    /// Directory entry `index`, in URL order.
    pub fn dirent(&self, index: u32) -> Result<Dirent<'a>> {
        if index >= self.entry_count() {
            return Err(Error::EntryIndex(index));
        }
        let ptr = self.header().url_ptr_pos + u64::from(index) * 8;
        let off = u64le(self.bytes, to_usize(ptr).ok_or(Error::EntryIndex(index))?)
            .ok_or(Error::EntryIndex(index))?;
        if off >= self.layout.data_end() {
            return Err(Error::Dirent("offset past archive data"));
        }
        Dirent::parse(self.bytes, to_usize(off).ok_or(Error::Dirent("offset overflow"))?)
    }

    /// Entry index of the `position`-th entry in title order.
    pub fn title_entry(&self, position: u32) -> Result<u32> {
        if position >= self.entry_count() {
            return Err(Error::EntryIndex(position));
        }
        let ptr = self.header().title_ptr_pos + u64::from(position) * 4;
        let index = u32le(self.bytes, to_usize(ptr).ok_or(Error::EntryIndex(position))?)
            .ok_or(Error::EntryIndex(position))?;
        if index >= self.entry_count() {
            return Err(Error::EntryIndex(index));
        }
        Ok(index)
    }

    /// Raw bytes of cluster `index`, info byte first.
    pub fn cluster_raw(&self, index: u32) -> Result<&'a [u8]> {
        if index >= self.cluster_count() {
            return Err(Error::ClusterIndex(index));
        }
        let base = self.header().cluster_ptr_pos;
        let start = u64le(self.bytes, to_usize(base + u64::from(index) * 8).ok_or(
            Error::ClusterIndex(index),
        )?)
        .ok_or(Error::ClusterIndex(index))?;
        let end = if index + 1 < self.cluster_count() {
            u64le(self.bytes, to_usize(base + u64::from(index + 1) * 8).ok_or(
                Error::ClusterIndex(index),
            )?)
            .ok_or(Error::ClusterIndex(index))?
        } else {
            self.layout.data_end()
        };
        if start >= end || end > self.layout.data_end() {
            return Err(Error::Cluster("cluster extent out of order"));
        }
        let (s, e) = (
            to_usize(start).ok_or(Error::Cluster("offset overflow"))?,
            to_usize(end).ok_or(Error::Cluster("offset overflow"))?,
        );
        self.bytes.get(s..e).ok_or(Error::Cluster("cluster past EOF"))
    }

    /// Compression type and offset width of cluster `index`, without decoding it.
    pub fn cluster_shape(&self, index: u32) -> Result<(u8, usize)> {
        let raw = self.cluster_raw(index)?;
        let info = *raw.first().ok_or(Error::Cluster("empty"))?;
        Ok((compression_of(info), offset_size_of(info)))
    }

    /// Decode cluster `index`, bounding decompressed output by `limit`.
    pub fn cluster(&self, index: u32, limit: usize) -> Result<Cluster<'a>> {
        Cluster::parse(self.cluster_raw(index)?, limit)
    }

    /// Binary search the URL pointer list for `(namespace, url)`.
    pub fn find(&self, namespace: u8, url: &[u8]) -> Result<Option<u32>> {
        let (mut lo, mut hi) = (0u32, self.entry_count());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let d = self.dirent(mid)?;
            match cmp_key(d.url_key(), (namespace, url)) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(Some(mid)),
            }
        }
        Ok(None)
    }

    /// First position in title order whose key is not less than `(namespace, prefix)`.
    pub fn title_lower_bound(&self, namespace: u8, prefix: &[u8]) -> Result<u32> {
        let (mut lo, mut hi) = (0u32, self.entry_count());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let d = self.dirent(self.title_entry(mid)?)?;
            match cmp_key(d.title_key(), (namespace, prefix)) {
                Ordering::Less => lo = mid + 1,
                _ => hi = mid,
            }
        }
        Ok(lo)
    }

    /// Follow redirects from `index` to a content entry.
    ///
    /// Stops after [`MAX_REDIRECT_HOPS`], so a cycle costs a bounded walk.
    pub fn resolve(&self, index: u32) -> Result<u32> {
        let mut at = index;
        for _ in 0..=MAX_REDIRECT_HOPS {
            match self.dirent(at)?.target {
                crate::dirent::Target::Redirect { entry } => {
                    if entry == at {
                        return Err(Error::RedirectDepth);
                    }
                    at = entry;
                }
                _ => return Ok(at),
            }
        }
        Err(Error::RedirectDepth)
    }
}

fn cmp_key(a: (u8, &[u8]), b: (u8, &[u8])) -> Ordering {
    a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1))
}
