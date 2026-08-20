//! Header value grammar.
//!
//! MIME types come from the archive's own table and are attacker-controlled;
//! this is where the two hostile inputs meet.

/// Fallback for any MIME type that is not a clean `type/subtype`.
pub const FALLBACK_MIME: &str = "application/octet-stream";

/// Cap on a MIME type length. Longer values are replaced, not truncated.
pub const MAX_MIME_LEN: usize = 255;

/// True for RFC 9110 token characters.
pub fn is_token_byte(c: u8) -> bool {
    matches!(c,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
        | b'`' | b'|' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z')
}

/// Sanitize a MIME type for use in `Content-Type`.
///
/// Anything containing CR, LF, NUL, or a non-token byte becomes
/// [`FALLBACK_MIME`]. Parameters (`; charset=...`) are kept only when every
/// byte in them is safe.
pub fn content_type(raw: &str) -> &str {
    if raw.is_empty() || raw.len() > MAX_MIME_LEN {
        return FALLBACK_MIME;
    }
    let (essence, params) = match raw.split_once(';') {
        Some((e, p)) => (e, Some(p)),
        None => (raw, None),
    };
    let Some((kind, subtype)) = essence.split_once('/') else {
        return FALLBACK_MIME;
    };
    let token_ok = |s: &str| !s.is_empty() && s.bytes().all(is_token_byte);
    if !token_ok(kind) || !token_ok(subtype) {
        return FALLBACK_MIME;
    }
    if let Some(params) = params
        && !params.bytes().all(is_param_byte)
    {
        return FALLBACK_MIME;
    }
    raw
}

fn is_param_byte(c: u8) -> bool {
    is_token_byte(c) || matches!(c, b' ' | b'\t' | b'=' | b'"' | b';' | b'/')
}

/// True if `value` is safe to write as a header value as-is.
pub fn is_safe_header_value(value: &str) -> bool {
    value.bytes().all(|c| c >= 0x20 && c != 0x7f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ordinary_types() {
        assert_eq!(content_type("text/html"), "text/html");
        assert_eq!(content_type("text/html; charset=utf-8"), "text/html; charset=utf-8");
        assert_eq!(content_type("image/svg+xml"), "image/svg+xml");
    }

    #[test]
    fn replaces_injection_attempts() {
        assert_eq!(content_type("text/html\r\nX-Evil: 1"), FALLBACK_MIME);
        assert_eq!(content_type("text/html\nX-Evil: 1"), FALLBACK_MIME);
        assert_eq!(content_type("text/html\r"), FALLBACK_MIME);
        assert_eq!(content_type("text/ht\0ml"), FALLBACK_MIME);
        assert_eq!(content_type("text/html; charset=\r\nx"), FALLBACK_MIME);
    }

    #[test]
    fn replaces_malformed_types() {
        assert_eq!(content_type(""), FALLBACK_MIME);
        assert_eq!(content_type("nosubtype"), FALLBACK_MIME);
        assert_eq!(content_type("/html"), FALLBACK_MIME);
        assert_eq!(content_type("text/"), FALLBACK_MIME);
        assert_eq!(content_type("te xt/html"), FALLBACK_MIME);
        assert_eq!(content_type(&"a/".repeat(200)), FALLBACK_MIME);
    }

    #[test]
    fn every_byte_is_covered() {
        // No single byte inserted into a MIME type may survive into a header
        // unless the whole value is still a clean token.
        for byte in 0u8..=255 {
            let raw = format!("text/ht{}ml", byte as char);
            let out = content_type(&raw);
            assert!(is_safe_header_value(out), "byte {byte} produced {out:?}");
        }
    }
}
