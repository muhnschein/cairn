//! `Range` parsing. Single range only: multipart `byteranges` is refused.

/// Outcome of parsing a `Range` header against a known content length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Serve the whole entry.
    Whole,
    /// Serve `[start, end)`.
    Partial { start: u64, end: u64 },
    /// Well formed but cannot be satisfied: answer 416.
    Unsatisfiable,
}

/// Parse a `Range` header value for an entry of `len` bytes.
///
/// A syntactically invalid header is ignored, per RFC 9110. A well-formed but
/// unsatisfiable one, and any multi-range request, is refused.
pub fn parse(value: &str, len: u64) -> Range {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Range::Whole; // other units are ignored
    };
    if spec.contains(',') {
        // Multipart byteranges is an amplification vector and a parser surface.
        return Range::Unsatisfiable;
    }
    let spec = spec.trim();
    let Some((first, last)) = spec.split_once('-') else {
        return Range::Whole;
    };

    let digits = |s: &str| -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok()
    };

    match (first.trim(), last.trim()) {
        ("", suffix) => match digits(suffix) {
            None => Range::Whole,
            Some(0) => Range::Unsatisfiable,
            Some(_) if len == 0 => Range::Unsatisfiable,
            Some(n) => Range::Partial {
                start: len.saturating_sub(n),
                end: len,
            },
        },
        (start, "") => match digits(start) {
            None => Range::Whole,
            Some(s) if s >= len => Range::Unsatisfiable,
            Some(s) => Range::Partial { start: s, end: len },
        },
        (start, end) => match (digits(start), digits(end)) {
            (Some(s), Some(e)) if s > e || s >= len => Range::Unsatisfiable,
            (Some(s), Some(e)) => Range::Partial {
                start: s,
                end: e.saturating_add(1).min(len),
            },
            _ => Range::Whole,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ranges() {
        assert_eq!(
            parse("bytes=0-9", 100),
            Range::Partial { start: 0, end: 10 }
        );
        assert_eq!(
            parse("bytes=10-", 100),
            Range::Partial {
                start: 10,
                end: 100
            }
        );
        assert_eq!(
            parse("bytes=-10", 100),
            Range::Partial {
                start: 90,
                end: 100
            }
        );
        assert_eq!(
            parse("bytes=0-1000", 100),
            Range::Partial { start: 0, end: 100 }
        );
        assert_eq!(
            parse("bytes=99-99", 100),
            Range::Partial {
                start: 99,
                end: 100
            }
        );
        assert_eq!(
            parse("bytes=-500", 100),
            Range::Partial { start: 0, end: 100 }
        );
    }

    #[test]
    fn multipart_is_refused() {
        assert_eq!(parse("bytes=0-1,5-6", 100), Range::Unsatisfiable);
        assert_eq!(parse("bytes=0-1, 5-6", 100), Range::Unsatisfiable);
    }

    #[test]
    fn unsatisfiable_ranges() {
        assert_eq!(parse("bytes=100-", 100), Range::Unsatisfiable);
        assert_eq!(parse("bytes=200-300", 100), Range::Unsatisfiable);
        assert_eq!(parse("bytes=5-4", 100), Range::Unsatisfiable);
        assert_eq!(parse("bytes=-0", 100), Range::Unsatisfiable);
        assert_eq!(parse("bytes=0-0", 0), Range::Unsatisfiable);
    }

    #[test]
    fn junk_is_ignored() {
        assert_eq!(parse("items=0-9", 100), Range::Whole);
        assert_eq!(parse("bytes=abc", 100), Range::Whole);
        assert_eq!(parse("bytes=-", 100), Range::Whole);
        assert_eq!(parse("", 100), Range::Whole);
        assert_eq!(parse("bytes=--5", 100), Range::Whole);
        assert_eq!(parse("bytes=999999999999999999999999-", 100), Range::Whole);
    }

    #[test]
    fn overlapping_and_degenerate_values_never_panic() {
        for a in ["", "0", "1", "18446744073709551615", "-1", "x", " "] {
            for b in ["", "0", "1", "18446744073709551615", "-1", "x", " "] {
                for len in [0u64, 1, 100, u64::MAX] {
                    let v = format!("bytes={a}-{b}");
                    if let Range::Partial { start, end } = parse(&v, len) {
                        assert!(start < end && end <= len, "{v} len={len}");
                    }
                }
            }
        }
    }
}
