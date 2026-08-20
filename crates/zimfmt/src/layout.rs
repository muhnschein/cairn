use crate::bytes::to_usize;
use crate::error::{Error, Result};
use crate::header::Header;

/// Hard caps on the MIME table, which is attacker-controlled.
pub const MAX_MIME_ENTRIES: usize = 4096;
/// Byte cap on the MIME table.
pub const MAX_MIME_TABLE_BYTES: usize = 64 * 1024;

/// Everything parsed once when an archive is opened: header, MIME table, and the
/// file-relative extents of the three pointer lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    header: Header,
    mimes: Vec<Box<[u8]>>,
    file_len: u64,
    data_end: u64,
}

impl Layout {
    /// Parse the header and MIME table and check that every declared region fits.
    pub fn parse(bytes: &[u8]) -> Result<Layout> {
        let header = Header::parse(bytes)?;
        let file_len = bytes.len() as u64;

        // The checksum, when present, is the last 16 bytes and is not archive data.
        let data_end = match header.checksum_pos {
            0 => file_len,
            pos if pos.checked_add(16) == Some(file_len) => pos,
            pos if pos < file_len => pos,
            _ => file_len,
        };

        region(
            header.url_ptr_pos,
            8,
            header.entry_count,
            data_end,
            "URL pointer list",
        )?;
        // A header without a title index is normal, not broken: current libzim
        // writes a sentinel here and stores the ordering as an entry.
        if header.has_title_index() {
            region(
                header.title_ptr_pos,
                4,
                header.entry_count,
                data_end,
                "title pointer list",
            )?;
        }
        region(
            header.cluster_ptr_pos,
            8,
            header.cluster_count,
            data_end,
            "cluster pointer list",
        )?;

        let mimes = parse_mime_table(bytes, header.mime_list_pos, data_end)?;
        Ok(Layout {
            header,
            mimes,
            file_len,
            data_end,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// MIME type string as stored, or `None` if the index is past the table.
    pub fn mime(&self, index: u16) -> Option<&[u8]> {
        self.mimes.get(index as usize).map(|m| &m[..])
    }

    /// Number of MIME table entries.
    pub fn mime_count(&self) -> usize {
        self.mimes.len()
    }

    /// End of archive data: the checksum position, or the file length.
    pub fn data_end(&self) -> u64 {
        self.data_end
    }

    /// Size of the mapped file.
    pub fn file_len(&self) -> u64 {
        self.file_len
    }
}

fn region(pos: u64, stride: u64, count: u32, data_end: u64, what: &'static str) -> Result<()> {
    // The numbers travel with the error: an operator looking at a refused
    // archive needs to see which region, where it starts, and how far it runs.
    let fail = |bytes: u64| Error::Region {
        what,
        at: pos,
        bytes,
        data_end,
    };
    let len = stride
        .checked_mul(u64::from(count))
        .ok_or_else(|| fail(u64::MAX))?;
    let end = pos.checked_add(len).ok_or_else(|| fail(len))?;
    if end > data_end {
        Err(fail(len))
    } else {
        Ok(())
    }
}

fn parse_mime_table(bytes: &[u8], pos: u64, data_end: u64) -> Result<Vec<Box<[u8]>>> {
    if pos >= data_end {
        return Err(Error::MimeList);
    }
    let start = to_usize(pos).ok_or(Error::MimeList)?;
    let end = to_usize(data_end).ok_or(Error::MimeList)?.min(bytes.len());
    let limit = end.min(start.saturating_add(MAX_MIME_TABLE_BYTES));
    let table = bytes.get(start..limit).ok_or(Error::MimeList)?;

    let mut mimes = Vec::new();
    let mut off = 0usize;
    loop {
        let rest = table.get(off..).ok_or(Error::MimeList)?;
        let nul = rest.iter().position(|&c| c == 0).ok_or(Error::MimeList)?;
        if nul == 0 {
            return Ok(mimes); // empty string terminates the table
        }
        if mimes.len() == MAX_MIME_ENTRIES {
            return Err(Error::MimeList);
        }
        mimes.push(rest[..nul].to_vec().into_boxed_slice());
        off += nul + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_table_needs_terminator() {
        let mut buf = b"text/html\0image/png\0".to_vec();
        assert_eq!(
            parse_mime_table(&buf, 0, buf.len() as u64),
            Err(Error::MimeList)
        );
        buf.push(0);
        let mimes = parse_mime_table(&buf, 0, buf.len() as u64).unwrap();
        assert_eq!(mimes.len(), 2);
        assert_eq!(&*mimes[0], b"text/html");
    }

    #[test]
    fn mime_table_is_capped() {
        let mut buf = Vec::new();
        for _ in 0..MAX_MIME_ENTRIES + 1 {
            buf.extend_from_slice(b"a\0");
        }
        buf.push(0);
        assert_eq!(
            parse_mime_table(&buf, 0, buf.len() as u64),
            Err(Error::MimeList)
        );
    }

    #[test]
    fn control_bytes_in_mime_are_kept_not_rejected() {
        // The API layer decides what is safe in a header; the parser only reports.
        let buf = b"text/html\r\nX: y\0\0".to_vec();
        let mimes = parse_mime_table(&buf, 0, buf.len() as u64).unwrap();
        assert_eq!(&*mimes[0], b"text/html\r\nX: y");
    }
}
