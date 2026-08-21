use crate::bytes::{slice, u16le, u32le, u64le};
use crate::error::{Error, Result};
use crate::uuid::Uuid;

/// ZIM magic number, first four bytes of every archive.
pub const MAGIC: u32 = 72_173_914;

/// Fixed header size in bytes.
pub const HEADER_LEN: usize = 80;

/// Sentinel for "no main page".
pub const NO_ENTRY: u32 = u32::MAX;

/// Sentinel libzim writes into `title_ptr_pos` when the header carries no
/// title index. Every archive written by a current libzim has this, and the
/// ordering lives in the entry `X/listing/titleOrdered/v1` instead.
pub const NO_TITLE_INDEX: u64 = u64::MAX;

/// The fixed-size archive header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Format major version.
    pub major_version: u16,
    /// Format minor version.
    pub minor_version: u16,
    /// Archive identity; the API addresses archives by this.
    pub uuid: Uuid,
    /// Entries in the archive, redirects included.
    pub entry_count: u32,
    /// Clusters in the archive.
    pub cluster_count: u32,
    /// Offset of the URL pointer list.
    pub url_ptr_pos: u64,
    /// Offset of the title pointer list, or the sentinel meaning there is none.
    pub title_ptr_pos: u64,
    /// Offset of the cluster pointer list.
    pub cluster_ptr_pos: u64,
    /// Offset of the MIME type table.
    pub mime_list_pos: u64,
    /// Entry index of the main page, or `u32::MAX` for none.
    pub main_page: u32,
    /// Layout page index; unused by current archives.
    pub layout_page: u32,
    /// Offset of the trailing MD5, which cairn does not verify.
    pub checksum_pos: u64,
}

impl Header {
    /// Parse and range-check the header. Does not look at the rest of the file.
    pub fn parse(b: &[u8]) -> Result<Header> {
        let head = slice(b, 0, HEADER_LEN).ok_or(Error::Truncated {
            what: "header",
            need: HEADER_LEN as u64,
            have: b.len() as u64,
        })?;
        // `head` is exactly HEADER_LEN bytes, so no field read below can fail.
        let magic = u32le(head, 0).unwrap_or(0);
        if magic != MAGIC {
            return Err(Error::BadMagic(magic));
        }
        let major_version = u16le(head, 4).unwrap_or(0);
        let minor_version = u16le(head, 6).unwrap_or(0);
        if !(5..=6).contains(&major_version) {
            return Err(Error::UnsupportedVersion {
                major: major_version,
                minor: minor_version,
            });
        }
        let mut raw = [0u8; 16];
        raw.copy_from_slice(&head[8..24]);
        let uuid = Uuid::from_bytes(raw);
        if uuid.is_nil() {
            return Err(Error::NilUuid);
        }
        let mime_list_pos = u64le(head, 56).unwrap_or(0);
        Ok(Header {
            major_version,
            minor_version,
            uuid,
            entry_count: u32le(head, 24).unwrap_or(0),
            cluster_count: u32le(head, 28).unwrap_or(0),
            url_ptr_pos: u64le(head, 32).unwrap_or(0),
            title_ptr_pos: u64le(head, 40).unwrap_or(0),
            cluster_ptr_pos: u64le(head, 48).unwrap_or(0),
            mime_list_pos,
            main_page: u32le(head, 64).unwrap_or(NO_ENTRY),
            layout_page: u32le(head, 68).unwrap_or(NO_ENTRY),
            // A MIME table starting at 72 means a header from before the
            // checksum field existed, so those bytes are table, not position.
            checksum_pos: if mime_list_pos == 72 {
                0
            } else {
                u64le(head, 72).unwrap_or(0)
            },
        })
    }

    /// True for archives using the `C`/`M`/`W`/`X` namespace scheme.
    pub fn new_namespace_scheme(&self) -> bool {
        self.major_version >= 6 && self.minor_version >= 1
    }

    /// Namespace holding user-visible content in this archive.
    pub fn content_namespace(&self) -> u8 {
        if self.new_namespace_scheme() {
            b'C'
        } else {
            b'A'
        }
    }

    /// True when the header points at a title pointer list of its own.
    ///
    /// False for archives written by a current libzim, which set the field to
    /// [`NO_TITLE_INDEX`] and store the ordering as an entry.
    pub fn has_title_index(&self) -> bool {
        self.title_ptr_pos != NO_TITLE_INDEX
    }

    /// Main page entry index, if the archive declares one.
    pub fn main_page(&self) -> Option<u32> {
        (self.main_page != NO_ENTRY && self.main_page < self.entry_count).then_some(self.main_page)
    }
}
