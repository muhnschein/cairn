//! Making an archive's text safe to put in front of a person, and laying it out.
//!
//! Almost everything cairn prints came from the archive: titles, entry paths,
//! MIME types, every `M` namespace value. `SECURITY.md` already scopes an
//! archive as hostile input, and a terminal is an interpreter — `"\x1b[2J"`
//! clears the reader's screen and `"\x1b]0;…\x07"` retitles their window. That
//! is not a parser bug; `zimfmt` has no opinion about `ESC` because `ESC` is not
//! a format problem. It is a display problem, and this is where it is fixed.
//!
//! **Scrub at the boundary, not at the source.** The stored title is the
//! archive's actual title and `/v1/archives` has to keep it; `api::json`
//! escapes everything below `0x20`, so `--json` consumers are inert. The
//! substitution happens here, at the one point where those bytes become a
//! terminal's input.

use std::fmt::Write as _;

/// A single-line cell: every control character goes, so nothing can forge a row.
pub fn line(text: &str) -> String {
    scrub(text, false)
}

/// A block of entry content on its way to a terminal: newlines and tabs stay,
/// because they are the document's own layout; everything else that a terminal
/// would act on rather than draw does not.
pub fn block(text: &str) -> String {
    scrub(text, true)
}

fn scrub(text: &str, keep_layout: bool) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\t' if keep_layout => c,
            c if c.is_control() => '.',
            // LRM/RLM, the LRE/RLE/PDF/LRO/RLO run, and the isolates: they draw
            // nothing and reorder the text around them, so a title can be made
            // to render as something it is not.
            '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => '.',
            c => c,
        })
        .collect()
}

/// [`line()`], then at most `width` characters of it, marking any elision.
pub fn elide(text: &str, width: usize) -> String {
    let safe = line(text);
    if safe.chars().count() <= width {
        return safe;
    }
    let mut out: String = safe.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Which edge a column's cells share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Columns sized to their widest cell, header included; the last is not padded.
///
/// Widths are measured in characters, not display columns. A CJK title is
/// therefore laid out a little wide, which is a dependency's worth of Unicode
/// tables away from being right and does not stop anything being read.
pub fn table(headers: &[(&str, Align)], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|(h, _)| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let mut out = String::new();
    let head: Vec<String> = headers.iter().map(|(h, _)| (*h).to_owned()).collect();
    write_row(&mut out, &head, headers, &widths);
    for row in rows {
        write_row(&mut out, row, headers, &widths);
    }
    out
}

fn write_row(out: &mut String, cells: &[String], headers: &[(&str, Align)], widths: &[usize]) {
    let last = cells.len().saturating_sub(1);
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let width = widths.get(i).copied().unwrap_or(0);
        match headers.get(i).map(|(_, a)| *a) {
            // Padding the last cell would leave trailing spaces on every line.
            _ if i == last => out.push_str(cell),
            Some(Align::Right) => {
                let _ = write!(out, "{cell:>width$}");
            }
            _ => {
                let _ = write!(out, "{cell:<width$}");
            }
        }
    }
    // An empty last cell would otherwise leave the previous column's padding
    // hanging off the end of the line.
    while out.ends_with(' ') {
        out.pop();
    }
    out.push('\n');
}

/// `1.2 GiB`, `700.0 MiB`, `43 B`.
#[allow(
    clippy::cast_precision_loss,
    reason = "display only; the rounded value is bounded by the unit table"
)]
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if count < 1024 {
        return format!("{count} B");
    }
    let mut size = count as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    // Rounding to one decimal can push a value back over the boundary the loop
    // just cleared: 1048575 bytes is 1023.999 KiB, which prints as "1024.0 KiB".
    // Step up rather than show a size in units of itself.
    if (size * 10.0).round() >= 10_240.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Compact uptime: `9s`, `5m`, `3h12m`, `2d4h`.
pub fn duration(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_go() {
        assert_eq!(line("a\tb\nc\r\n"), "a.b.c..");
        assert_eq!(line("\u{1b}[2J"), ".[2J");
        assert_eq!(line("\u{1b}]0;title\u{7}"), ".]0;title.");
        // C1 and DEL are controls too, and are easy to forget.
        for c in ['\u{0}', '\u{7f}', '\u{80}', '\u{9b}'] {
            assert_eq!(line(&c.to_string()), ".", "{c:?} survived");
        }
    }

    #[test]
    fn bidi_overrides_go() {
        // The classic spoof: RLO makes this render as "safe.exe.gnp".
        assert_eq!(line("safe\u{202e}gnp.exe"), "safe.gnp.exe");
        for c in [
            '\u{200e}', '\u{200f}', '\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            assert_eq!(line(&c.to_string()), ".", "{c:?} survived");
        }
    }

    #[test]
    fn a_block_keeps_the_documents_own_layout() {
        // An article printed to a terminal is still an article.
        assert_eq!(block("<p>a</p>\n\t<p>b</p>\n"), "<p>a</p>\n\t<p>b</p>\n");
        // Everything a terminal acts on still goes, including the CR that would
        // let a document overwrite the line above it.
        assert_eq!(block("a\u{1b}[2Jb\r"), "a.[2Jb.");
        assert_eq!(block("safe\u{202e}gnp.exe"), "safe.gnp.exe");
    }

    #[test]
    fn ordinary_text_is_untouched() {
        for ok in ["Climate change", "café", "日本語", "a b c", ""] {
            assert_eq!(line(ok), ok, "{ok:?} was altered");
        }
    }

    #[test]
    fn elision_counts_characters() {
        assert_eq!(elide("short", 10), "short");
        assert_eq!(elide("abcde", 5), "abcde");
        assert_eq!(elide("abcdef", 5), "abcd…");
        // Multi-byte text must not be cut mid-character nor counted as longer
        // than it reads.
        assert_eq!(elide("ééééé", 5), "ééééé");
        assert_eq!(elide("ééééé", 3), "éé…");
        assert_eq!(elide("abc", 0), "…");
        // Scrubbing still applies to what survives the cut.
        assert_eq!(elide("a\u{1b}bcdef", 3), "a.…");
    }

    #[test]
    fn columns_align_to_the_widest_cell() {
        let rows = vec![
            vec!["1".to_owned(), "short".to_owned()],
            vec!["1000".to_owned(), "longer".to_owned()],
        ];
        assert_eq!(
            table(&[("N", Align::Right), ("NAME", Align::Left)], &rows),
            "   N  NAME\n   1  short\n1000  longer\n"
        );
    }

    #[test]
    fn the_last_column_is_not_padded() {
        let rows = vec![
            vec!["a".to_owned(), "b".to_owned()],
            // An empty last cell is the case that leaves the padding hanging.
            vec!["aaa".to_owned(), String::new()],
        ];
        for line in table(&[("X", Align::Left), ("YY", Align::Left)], &rows).lines() {
            assert_eq!(line.trim_end(), line, "{line:?} has trailing space");
        }
    }

    #[test]
    fn sizes_never_read_in_units_of_themselves() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1_048_575), "1.0 MiB");
        assert_eq!(bytes(67_108_864), "64.0 MiB");
        assert!(!bytes(u64::MAX).starts_with("1024"));
    }

    #[test]
    fn durations_are_compact() {
        assert_eq!(duration(9), "9s");
        assert_eq!(duration(300), "5m");
        assert_eq!(duration(11_520), "3h12m");
        assert_eq!(duration(187_200), "2d4h");
    }
}
