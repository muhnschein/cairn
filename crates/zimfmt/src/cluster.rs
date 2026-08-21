use std::borrow::Cow;

use crate::bytes::{slice, u32le, u64le};
use crate::decompress::{decompress, is_uncompressed};
use crate::error::{Error, Result};

/// Bit set in the cluster info byte when blob offsets are 64-bit.
pub const EXTENDED_FLAG: u8 = 0x10;

/// A cluster body: a table of blob offsets followed by the blobs.
///
/// Borrowed when the cluster is stored uncompressed, owned when it was decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster<'a> {
    body: Cow<'a, [u8]>,
    offset_size: usize,
    blob_count: u32,
}

impl<'a> Cluster<'a> {
    /// Parse a cluster from its raw bytes, info byte first.
    ///
    /// `limit` bounds the decompressed size.
    pub fn parse(raw: &'a [u8], limit: usize) -> Result<Cluster<'a>> {
        let info = *raw.first().ok_or(Error::Cluster("empty"))?;
        let kind = info & 0x0f;
        let offset_size = if info & EXTENDED_FLAG != 0 { 8 } else { 4 };
        let payload = raw.get(1..).ok_or(Error::Cluster("empty"))?;
        let body: Cow<'a, [u8]> = if is_uncompressed(kind) {
            Cow::Borrowed(payload)
        } else {
            Cow::Owned(decompress(kind, payload, limit)?)
        };
        Cluster::from_body(body, offset_size)
    }

    fn from_body(body: Cow<'a, [u8]>, offset_size: usize) -> Result<Cluster<'a>> {
        if body.is_empty() {
            return Ok(Cluster {
                body,
                offset_size,
                blob_count: 0,
            });
        }
        let first = read_offset(&body, 0, offset_size).ok_or(Error::Cluster("no offset table"))?;
        let first = usize::try_from(first).map_err(|_| Error::Cluster("offset overflow"))?;
        if first < offset_size || first % offset_size != 0 || first > body.len() {
            return Err(Error::Cluster("bad first offset"));
        }
        let blob_count = u32::try_from(first / offset_size - 1)
            .map_err(|_| Error::Cluster("blob count overflow"))?;
        Ok(Cluster {
            body,
            offset_size,
            blob_count,
        })
    }

    /// Number of blobs in the cluster.
    pub fn blob_count(&self) -> u32 {
        self.blob_count
    }

    /// Byte range of blob `index` within [`Cluster::body`].
    ///
    /// Offsets are validated per access, so a crafted offset table costs O(1),
    /// not a scan.
    pub fn blob_range(&self, index: u32) -> Result<(usize, usize)> {
        if index >= self.blob_count {
            return Err(Error::BlobIndex(index));
        }
        let at = |i: u32| -> Result<usize> {
            let off = (i as usize) * self.offset_size;
            let v = read_offset(&self.body, off, self.offset_size)
                .ok_or(Error::Cluster("offset past body"))?;
            usize::try_from(v).map_err(|_| Error::Cluster("offset overflow"))
        };
        let start = at(index)?;
        let end = at(index + 1)?;
        if start > end || end > self.body.len() {
            return Err(Error::Cluster("blob outside body"));
        }
        Ok((start, end))
    }

    /// Bytes of blob `index`.
    pub fn blob(&self, index: u32) -> Result<&[u8]> {
        let (start, end) = self.blob_range(index)?;
        self.body
            .get(start..end)
            .ok_or(Error::Cluster("blob outside body"))
    }

    /// The whole body, offset table included.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Take ownership of the body, copying only if it was borrowed.
    pub fn into_body(self) -> Vec<u8> {
        self.body.into_owned()
    }

    /// Rebuild a cluster from a body already decoded and cached.
    pub fn from_cached(body: &'a [u8], offset_size: usize) -> Result<Cluster<'a>> {
        Cluster::from_body(Cow::Borrowed(body), offset_size)
    }

    /// 4 or 8, depending on the extended flag.
    pub fn offset_size(&self) -> usize {
        self.offset_size
    }
}

/// Offset size implied by a cluster info byte.
pub fn offset_size_of(info: u8) -> usize {
    if info & EXTENDED_FLAG != 0 { 8 } else { 4 }
}

fn read_offset(body: &[u8], off: usize, size: usize) -> Option<u64> {
    match size {
        4 => u32le(body, off).map(u64::from),
        8 => u64le(body, off),
        _ => None,
    }
}

/// Raw compression type from a cluster info byte.
pub fn compression_of(info: u8) -> u8 {
    info & 0x0f
}

/// Length check used before parsing: a cluster must have at least an info byte.
pub fn check_raw(raw: &[u8]) -> Result<()> {
    slice(raw, 0, 1).map(|_| ()).ok_or(Error::Cluster("empty"))
}

#[cfg(test)]
mod tests {
    //! Offsets here are the fixtures' own, sized by the test.
    #![allow(clippy::cast_possible_truncation)]

    use super::*;

    fn uncompressed(blobs: &[&[u8]]) -> Vec<u8> {
        let n = blobs.len();
        let mut body = Vec::new();
        let table = ((n + 1) * 4) as u32;
        let mut off = table;
        body.extend_from_slice(&off.to_le_bytes());
        for b in blobs {
            off += b.len() as u32;
            body.extend_from_slice(&off.to_le_bytes());
        }
        for b in blobs {
            body.extend_from_slice(b);
        }
        let mut raw = vec![1u8];
        raw.extend_from_slice(&body);
        raw
    }

    #[test]
    fn reads_blobs() {
        let raw = uncompressed(&[b"alpha", b"", b"omega"]);
        let c = Cluster::parse(&raw, 1 << 20).unwrap();
        assert_eq!(c.blob_count(), 3);
        assert_eq!(c.blob(0).unwrap(), b"alpha");
        assert_eq!(c.blob(1).unwrap(), b"");
        assert_eq!(c.blob(2).unwrap(), b"omega");
        assert_eq!(c.blob(3), Err(Error::BlobIndex(3)));
    }

    #[test]
    fn descending_offsets_are_refused() {
        let mut raw = uncompressed(&[b"alpha", b"omega"]);
        raw[5..9].copy_from_slice(&0u32.to_le_bytes()); // second offset below the first
        let c = Cluster::parse(&raw, 1 << 20).unwrap();
        assert_eq!(c.blob(0), Err(Error::Cluster("blob outside body")));
    }

    #[test]
    fn offset_past_body_is_refused() {
        let mut raw = uncompressed(&[b"alpha"]);
        raw[5..9].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        let c = Cluster::parse(&raw, 1 << 20).unwrap();
        assert_eq!(c.blob(0), Err(Error::Cluster("blob outside body")));
    }

    #[test]
    fn bad_first_offset_is_refused() {
        let mut raw = uncompressed(&[b"alpha"]);
        raw[1..5].copy_from_slice(&3u32.to_le_bytes()); // not a multiple of 4
        assert_eq!(
            Cluster::parse(&raw, 1 << 20),
            Err(Error::Cluster("bad first offset"))
        );
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let raw = uncompressed(&[b"alpha", b"omega"]);
        for n in 0..raw.len() {
            match Cluster::parse(&raw[..n], 1 << 20) {
                Err(_) => {}
                Ok(c) => {
                    for i in 0..c.blob_count() {
                        let _ = c.blob(i);
                    }
                }
            }
        }
    }
}
