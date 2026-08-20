//! Cluster decompression with a hard output bound.

use std::io::{self, Write};

use crate::error::{Error, Result};

/// Cluster compression type: no compression (writer default).
pub const COMP_DEFAULT: u8 = 0;
/// Cluster compression type: no compression.
pub const COMP_NONE: u8 = 1;
/// Cluster compression type: zlib. Obsolete, not supported.
pub const COMP_ZLIB: u8 = 2;
/// Cluster compression type: bzip2. Obsolete, not supported.
pub const COMP_BZIP2: u8 = 3;
/// Cluster compression type: LZMA2 in an xz container.
pub const COMP_XZ: u8 = 4;
/// Cluster compression type: Zstandard.
pub const COMP_ZSTD: u8 = 5;

/// True if `kind` means the cluster body is stored as-is.
pub fn is_uncompressed(kind: u8) -> bool {
    kind == COMP_DEFAULT || kind == COMP_NONE
}

/// Decompress a cluster body, refusing to produce more than `limit` bytes.
///
/// A crafted cluster cannot exhaust memory: the sink stops the decoder at the
/// bound rather than after the fact.
pub fn decompress(kind: u8, input: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut sink = Bounded {
        out: Vec::new(),
        limit,
    };
    let outcome = match kind {
        COMP_XZ => xz_into(input, &mut sink),
        COMP_ZSTD => zstd_into(input, &mut sink),
        COMP_ZLIB | COMP_BZIP2 => Err(Error::UnsupportedCompression(kind)),
        other => Err(Error::UnsupportedCompression(other)),
    };
    match outcome {
        Ok(()) => Ok(sink.out),
        Err(e) if sink.overflowed() => {
            let _ = e;
            Err(Error::TooLarge { limit })
        }
        Err(e) => Err(e),
    }
}

fn xz_into(input: &[u8], sink: &mut Bounded) -> Result<()> {
    let mut source = input;
    lzma_rs::xz_decompress(&mut source, sink).map_err(|_| Error::Decompress("xz"))
}

fn zstd_into(input: &[u8], sink: &mut Bounded) -> Result<()> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(input)
        .map_err(|_| Error::Decompress("zstd frame header"))?;
    io::copy(&mut decoder, sink)
        .map(|_| ())
        .map_err(|_| Error::Decompress("zstd"))
}

/// A sink that fails the write that would cross `limit`.
struct Bounded {
    out: Vec<u8>,
    limit: usize,
}

impl Bounded {
    fn overflowed(&self) -> bool {
        self.out.len() >= self.limit
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.out.len() + buf.len() > self.limit {
            // Fill to the bound so `overflowed` can distinguish this from a corrupt stream.
            let room = self.limit - self.out.len();
            self.out.extend_from_slice(&buf[..room]);
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "output bound reached",
            ));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_codecs_are_refused() {
        assert_eq!(
            decompress(COMP_ZLIB, b"", 1024),
            Err(Error::UnsupportedCompression(2))
        );
        assert_eq!(
            decompress(COMP_BZIP2, b"", 1024),
            Err(Error::UnsupportedCompression(3))
        );
        assert_eq!(
            decompress(9, b"", 1024),
            Err(Error::UnsupportedCompression(9))
        );
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decompress(COMP_ZSTD, b"not a frame", 1024).is_err());
        assert!(decompress(COMP_XZ, b"not a stream", 1024).is_err());
    }

    #[test]
    fn zstd_round_trip_and_bound() {
        let plain = vec![b'x'; 200_000];
        let packed = ruzstd::encoding::compress_to_vec(
            &plain[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert_eq!(decompress(COMP_ZSTD, &packed, 1 << 20).unwrap(), plain);
        assert_eq!(
            decompress(COMP_ZSTD, &packed, 4096),
            Err(Error::TooLarge { limit: 4096 })
        );
    }

    #[test]
    fn xz_round_trip_and_bound() {
        let plain = vec![b'y'; 200_000];
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut &plain[..], &mut packed).unwrap();
        assert_eq!(decompress(COMP_XZ, &packed, 1 << 20).unwrap(), plain);
        assert_eq!(
            decompress(COMP_XZ, &packed, 4096),
            Err(Error::TooLarge { limit: 4096 })
        );
    }
}
