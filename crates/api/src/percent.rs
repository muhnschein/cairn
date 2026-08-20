//! Percent-decoding. Exactly once, never on a decoded result.

/// Why a target could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// `%` not followed by two hex digits.
    BadEscape,
    /// `%00`, or a raw control byte.
    ForbiddenByte,
    /// Decoded bytes are not valid UTF-8 (this rejects over-long forms too).
    NotUtf8,
}

/// Decode one percent-encoded path or query value.
///
/// Rejects `%00`, raw control bytes, and any byte sequence that is not
/// canonical UTF-8. The result is never decoded again.
pub fn decode(raw: &str) -> Result<String, DecodeError> {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hi = b
                    .get(i + 1)
                    .copied()
                    .and_then(hex)
                    .ok_or(DecodeError::BadEscape)?;
                let lo = b
                    .get(i + 2)
                    .copied()
                    .and_then(hex)
                    .ok_or(DecodeError::BadEscape)?;
                let byte = (hi << 4) | lo;
                if byte == 0 || byte < 0x20 || byte == 0x7f {
                    return Err(DecodeError::ForbiddenByte);
                }
                out.push(byte);
                i += 3;
            }
            c if c < 0x20 || c == 0x7f => return Err(DecodeError::ForbiddenByte),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| DecodeError::NotUtf8)
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode a string for use in a header value.
///
/// Everything outside an unreserved-plus-slash set is escaped, so archive data
/// cannot inject a header.
pub fn encode_header_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@'
            | b'!'
            | b'$'
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'='
            | b'\'' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_once() {
        assert_eq!(decode("a%2Fb").unwrap(), "a/b");
        // %252F decodes to the literal text %2F and stops there.
        assert_eq!(decode("a%252Fb").unwrap(), "a%2Fb");
    }

    #[test]
    fn rejects_nul_and_controls() {
        assert_eq!(decode("a%00b"), Err(DecodeError::ForbiddenByte));
        assert_eq!(decode("a%0Ab"), Err(DecodeError::ForbiddenByte));
        assert_eq!(decode("a\nb"), Err(DecodeError::ForbiddenByte));
        assert_eq!(decode("a%7Fb"), Err(DecodeError::ForbiddenByte));
    }

    #[test]
    fn rejects_bad_escapes() {
        assert_eq!(decode("%zz"), Err(DecodeError::BadEscape));
        assert_eq!(decode("%2"), Err(DecodeError::BadEscape));
        assert_eq!(decode("%"), Err(DecodeError::BadEscape));
    }

    #[test]
    fn rejects_over_long_utf8() {
        // C0 80 is an over-long encoding of NUL; C0 AF of '/'.
        assert_eq!(decode("%C0%80"), Err(DecodeError::NotUtf8));
        assert_eq!(decode("%C0%AF"), Err(DecodeError::NotUtf8));
        assert_eq!(decode("%E0%80%AF"), Err(DecodeError::NotUtf8));
        // Surrogate halves are not valid UTF-8 either.
        assert_eq!(decode("%ED%A0%80"), Err(DecodeError::NotUtf8));
    }

    #[test]
    fn accepts_real_utf8() {
        assert_eq!(decode("caf%C3%A9").unwrap(), "café");
    }

    #[test]
    fn header_encoding_escapes_control_bytes() {
        assert_eq!(encode_header_value("a/b c"), "a/b%20c");
        assert_eq!(encode_header_value("x\r\nY: z"), "x%0D%0AY:%20z");
        assert_eq!(encode_header_value("café"), "caf%C3%A9");
    }
}
