use crate::bytes::{slice, u16le, u32le};
use crate::error::{Error, Result};

/// `mimetype` value marking a redirect entry.
pub const MIME_REDIRECT: u16 = 0xffff;
/// `mimetype` value marking a link target entry (obsolete).
pub const MIME_LINKTARGET: u16 = 0xfffe;
/// `mimetype` value marking a deleted entry.
pub const MIME_DELETED: u16 = 0xfffd;

/// Cap on the URL and title strings inside one directory entry.
pub const MAX_STRING: usize = 4096;

/// What a directory entry points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Blob `blob` of cluster `cluster`.
    Content { cluster: u32, blob: u32 },
    /// Another entry, by index into the URL pointer list.
    Redirect { entry: u32 },
    /// Link target or deleted: no content, no redirect.
    Absent,
}

/// One directory entry, borrowing its strings from the mapped file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dirent<'a> {
    pub mime_index: u16,
    pub namespace: u8,
    pub revision: u32,
    pub target: Target,
    pub url: &'a [u8],
    pub title: &'a [u8],
}

impl<'a> Dirent<'a> {
    /// Parse the entry at `off`. Every field is checked against `bytes`.
    pub fn parse(bytes: &'a [u8], off: usize) -> Result<Dirent<'a>> {
        let mime_index = u16le(bytes, off).ok_or(Error::Dirent("header past EOF"))?;
        let parameter_len = *bytes.get(off + 2).ok_or(Error::Dirent("header past EOF"))? as usize;
        let namespace = *bytes.get(off + 3).ok_or(Error::Dirent("header past EOF"))?;
        let revision = u32le(bytes, off + 4).ok_or(Error::Dirent("header past EOF"))?;

        let (target, strings_at) = match mime_index {
            MIME_REDIRECT => {
                let entry = u32le(bytes, off + 8).ok_or(Error::Dirent("redirect past EOF"))?;
                (Target::Redirect { entry }, off + 12)
            }
            MIME_LINKTARGET | MIME_DELETED => (Target::Absent, off + 8),
            _ => {
                let cluster = u32le(bytes, off + 8).ok_or(Error::Dirent("cluster past EOF"))?;
                let blob = u32le(bytes, off + 12).ok_or(Error::Dirent("blob past EOF"))?;
                (Target::Content { cluster, blob }, off + 16)
            }
        };

        let (url, after_url) = cstr(bytes, strings_at, "url")?;
        let (title, after_title) = cstr(bytes, after_url, "title")?;
        // The parameter block is skipped, but it must be present.
        slice(bytes, after_title, parameter_len).ok_or(Error::Dirent("parameter past EOF"))?;

        Ok(Dirent {
            mime_index,
            namespace,
            revision,
            target,
            url,
            title,
        })
    }

    /// Title if the entry has one, else the URL — the ZIM convention.
    pub fn effective_title(&self) -> &'a [u8] {
        if self.title.is_empty() {
            self.url
        } else {
            self.title
        }
    }

    /// Sort key of the URL pointer list: namespace first, then URL bytes.
    pub fn url_key(&self) -> (u8, &'a [u8]) {
        (self.namespace, self.url)
    }

    /// Sort key of the title pointer list.
    pub fn title_key(&self) -> (u8, &'a [u8]) {
        (self.namespace, self.effective_title())
    }
}

/// Read a NUL-terminated string, returning it and the offset just past the NUL.
fn cstr<'a>(bytes: &'a [u8], off: usize, what: &'static str) -> Result<(&'a [u8], usize)> {
    let rest = bytes.get(off..).ok_or(Error::Dirent(what))?;
    let window = rest
        .get(..MAX_STRING.min(rest.len()))
        .ok_or(Error::Dirent(what))?;
    match window.iter().position(|&c| c == 0) {
        Some(n) => Ok((&window[..n], off + n + 1)),
        None if rest.len() > MAX_STRING => Err(Error::Dirent("string too long")),
        None => Err(Error::Dirent("unterminated string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_dirent() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0u16.to_le_bytes()); // mime 0
        d.push(0); // parameter len
        d.push(b'C'); // namespace
        d.extend_from_slice(&0u32.to_le_bytes()); // revision
        d.extend_from_slice(&7u32.to_le_bytes()); // cluster
        d.extend_from_slice(&3u32.to_le_bytes()); // blob
        d.extend_from_slice(b"index.html\0");
        d.extend_from_slice(b"Main Page\0");
        d
    }

    #[test]
    fn parses_content() {
        let d = content_dirent();
        let e = Dirent::parse(&d, 0).unwrap();
        assert_eq!(
            e.target,
            Target::Content {
                cluster: 7,
                blob: 3
            }
        );
        assert_eq!(e.namespace, b'C');
        assert_eq!(e.url, b"index.html");
        assert_eq!(e.effective_title(), b"Main Page");
    }

    #[test]
    fn parses_redirect() {
        let mut d = Vec::new();
        d.extend_from_slice(&MIME_REDIRECT.to_le_bytes());
        d.push(0);
        d.push(b'C');
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&42u32.to_le_bytes());
        d.extend_from_slice(b"old\0new\0");
        let e = Dirent::parse(&d, 0).unwrap();
        assert_eq!(e.target, Target::Redirect { entry: 42 });
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let d = content_dirent();
        for n in 0..d.len() {
            assert!(
                Dirent::parse(&d[..n], 0).is_err(),
                "prefix of {n} bytes parsed"
            );
        }
    }

    #[test]
    fn unterminated_string_is_bounded() {
        let mut d = content_dirent();
        d.truncate(16);
        d.extend(std::iter::repeat_n(b'a', MAX_STRING + 10));
        assert_eq!(Dirent::parse(&d, 0), Err(Error::Dirent("string too long")));
    }

    #[test]
    fn empty_title_falls_back_to_url() {
        let mut d = Vec::new();
        d.extend_from_slice(&0u16.to_le_bytes());
        d.push(0);
        d.push(b'C');
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(b"a.html\0\0");
        let e = Dirent::parse(&d, 0).unwrap();
        assert_eq!(e.effective_title(), b"a.html");
    }
}
