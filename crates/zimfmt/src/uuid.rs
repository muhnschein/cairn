use core::fmt;

/// The 16 raw bytes of the archive UUID from the ZIM header.
///
/// Rendered and parsed only in canonical lowercase hyphenated form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
    /// Wrap raw header bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Uuid(bytes)
    }

    /// The raw bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// All-zero UUIDs are refused at open time.
    pub fn is_nil(&self) -> bool {
        self.0 == [0u8; 16]
    }

    /// Parse the canonical lowercase hyphenated form. Anything else is `None`.
    pub fn parse(s: &str) -> Option<Uuid> {
        let b = s.as_bytes();
        if b.len() != 36 {
            return None;
        }
        let mut out = [0u8; 16];
        let mut i = 0; // index into b
        let mut o = 0; // index into out
        while o < 16 {
            if matches!(i, 8 | 13 | 18 | 23) {
                if b[i] != b'-' {
                    return None;
                }
                i += 1;
                continue;
            }
            let hi = lower_hex(b[i])?;
            let lo = lower_hex(b[i + 1])?;
            out[o] = (hi << 4) | lo;
            i += 2;
            o += 1;
        }
        Some(Uuid(out))
    }
}

fn lower_hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uuid({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let u = Uuid::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ]);
        let s = u.to_string();
        assert_eq!(s, "01234567-89ab-cdef-fedc-ba9876543210");
        assert_eq!(Uuid::parse(&s), Some(u));
    }

    #[test]
    fn rejects_non_canonical() {
        assert_eq!(Uuid::parse("01234567-89AB-CDEF-FEDC-BA9876543210"), None);
        assert_eq!(Uuid::parse("0123456789abcdeffedcba9876543210"), None);
        assert_eq!(Uuid::parse("{01234567-89ab-cdef-fedc-ba9876543210}"), None);
        assert_eq!(Uuid::parse(""), None);
        assert_eq!(Uuid::parse("01234567-89ab-cdef-fedc-ba98765432zz"), None);
    }

    #[test]
    fn nil() {
        assert!(Uuid::from_bytes([0; 16]).is_nil());
    }
}
