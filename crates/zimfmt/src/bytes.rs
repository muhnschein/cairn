//! Bounds-checked little-endian reads. Every offset in the file goes through here.

pub(crate) fn slice(b: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    let end = off.checked_add(len)?;
    b.get(off..end)
}

pub(crate) fn u16le(b: &[u8], off: usize) -> Option<u16> {
    let s = slice(b, off, 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

pub(crate) fn u32le(b: &[u8], off: usize) -> Option<u32> {
    let s = slice(b, off, 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn u64le(b: &[u8], off: usize) -> Option<u64> {
    let s = slice(b, off, 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// `u64` file offset narrowed to `usize`, or `None` if it cannot be one.
pub(crate) fn to_usize(v: u64) -> Option<usize> {
    usize::try_from(v).ok()
}
