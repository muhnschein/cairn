//! Turning the daemon's JSON into something a person reads.
//!
//! Every string here came out of an archive, so every string here goes through
//! [`text::line`] on its way out. `to_line` on its own is the bug.

use std::fmt::Write as _;

use crate::json::Value;
use crate::text::{self, Align};

/// Label column for the reports: two spaces of indent plus the longest label
/// nested under a heading (`no_new_privs`), and two more before the value.
const LABEL: usize = 16;

/// Label column for `archive`, whose longest field is `content_namespace`.
const FIELD: usize = 19;

/// How wide a title may be in a table before it is elided.
const TITLE_WIDTH: usize = 48;

/// `status`: who is answering, what confinement took, and what it is doing.
pub fn status(v: &Value) -> String {
    let num = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut rows: Vec<(String, String)> = vec![
        ("cairn".to_owned(), field(v, "version")),
        ("listener".to_owned(), field(v, "listener")),
        ("uptime".to_owned(), text::duration(num("uptime_seconds"))),
        ("auth".to_owned(), field(v, "auth")),
        ("archives".to_owned(), num("archives").to_string()),
    ];

    // The reason this command exists: a daemon that failed to confine itself
    // otherwise looks exactly like one that succeeded.
    if let Some(sandbox) = v.get("sandbox") {
        let required = sandbox.get("required").and_then(Value::as_bool) == Some(true);
        rows.push((String::new(), String::new()));
        rows.push((
            "sandbox".to_owned(),
            if required { "required" } else { "best-effort" }.to_owned(),
        ));
        for layer in sandbox
            .get("layers")
            .and_then(Value::as_array)
            .unwrap_or(&[])
        {
            let state = field(layer, "state");
            let detail = layer.get("detail").and_then(Value::as_str);
            rows.push((
                format!("  {}", field(layer, "name")),
                match detail {
                    Some(d) => format!("{state} ({})", text::line(d)),
                    None => state,
                },
            ));
        }
    }

    if let Some(c) = v.get("connections") {
        let n = |key: &str| c.get(key).and_then(Value::as_u64).unwrap_or(0);
        rows.extend([
            (String::new(), String::new()),
            (
                "connections".to_owned(),
                format!("{} active of {}", n("active"), n("max")),
            ),
            ("  served".to_owned(), n("served").to_string()),
            ("  rejected".to_owned(), n("rejected").to_string()),
        ]);
    }

    if let Some(c) = v.get("cache") {
        let n = |key: &str| c.get(key).and_then(Value::as_u64).unwrap_or(0);
        rows.extend([
            (String::new(), String::new()),
            (
                "cache".to_owned(),
                format!(
                    "{} of {} in {} cluster{}",
                    text::bytes(n("bytes")),
                    text::bytes(n("budget_bytes")),
                    n("entries"),
                    if n("entries") == 1 { "" } else { "s" },
                ),
            ),
            ("  hits".to_owned(), n("hits").to_string()),
            ("  misses".to_owned(), n("misses").to_string()),
            ("  evictions".to_owned(), n("evictions").to_string()),
        ]);
    }

    let mut out = String::new();
    for (label, value) in rows {
        if label.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{label:<LABEL$}{value}");
        }
    }
    out
}

/// `archives`: the whole discovery surface, one row each.
pub fn archives(v: &Value) -> String {
    let Some(items) = v.get("archives").and_then(Value::as_array) else {
        return format!("{}\n", text::line(&v.to_line()));
    };
    if items.is_empty() {
        return "no archives\n".to_owned();
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|a| {
            vec![
                field(a, "uuid"),
                count(a, "entry_count"),
                count(a, "cluster_count"),
                field(a, "suggest"),
                title(a, TITLE_WIDTH),
            ]
        })
        .collect();
    text::table(
        &[
            ("UUID", Align::Left),
            ("ENTRIES", Align::Right),
            ("CLUSTERS", Align::Right),
            ("SUGGEST", Align::Left),
            ("TITLE", Align::Left),
        ],
        &rows,
    )
}

/// `archive`: one archive's fields, then its `M` namespace.
pub fn archive(v: &Value) -> String {
    let mut out = String::new();
    for key in [
        "uuid",
        "title",
        "entry_count",
        "cluster_count",
        "main_page",
        "format_version",
        "content_namespace",
        "suggest",
    ] {
        let Some(value) = v.get(key) else { continue };
        let rendered = match key {
            "entry_count" | "cluster_count" => count(v, key),
            _ => cell(value),
        };
        let _ = writeln!(out, "{key:<FIELD$}{rendered}");
    }

    if let Some(members) = v.get("metadata").and_then(Value::as_object)
        && !members.is_empty()
    {
        out.push_str("\nmetadata:\n");
        let rows: Vec<Vec<String>> = members
            .iter()
            .map(|(k, value)| vec![text::line(k), cell(value)])
            .collect();
        out.push_str(&indent(&text::table(
            &[("NAME", Align::Left), ("VALUE", Align::Left)],
            &rows,
        )));
    }

    // Named but not shown: their whole point is that they are not text.
    if let Some(names) = v.get("binary_metadata").and_then(Value::as_array)
        && !names.is_empty()
    {
        out.push_str("\nbinary metadata:\n");
        for name in names {
            let _ = writeln!(out, "  {}", cell(name));
        }
    }
    out
}

/// `suggest`: title and the path that fetches it.
///
/// `no_index` marks an archive that carries no title ordering at all, which is
/// otherwise indistinguishable from a prefix that simply matched nothing.
pub fn suggest(v: &Value, no_index: bool) -> String {
    let Some(items) = v.get("suggestions").and_then(Value::as_array) else {
        return format!("{}\n", text::line(&v.to_line()));
    };
    if items.is_empty() {
        return if no_index {
            "no suggestions: this archive carries no title index\n".to_owned()
        } else {
            "no suggestions\n".to_owned()
        };
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|s| vec![title(s, TITLE_WIDTH), field(s, "path")])
        .collect();
    text::table(&[("TITLE", Align::Left), ("PATH", Align::Left)], &rows)
}

/// `random`: the path alone, so it can be fed straight back to `cairn get`.
pub fn random(v: &Value) -> String {
    format!("{}\n", field(v, "path"))
}

/// An error document, as one line for stderr.
pub fn fault(body: &[u8], status: u16) -> String {
    let parsed = std::str::from_utf8(body)
        .ok()
        .and_then(|t| crate::json::parse(t).ok());
    let Some(error) = parsed.as_ref().and_then(|v| v.get("error")) else {
        return format!("the daemon answered {status}");
    };
    format!("{}: {}", field(error, "code"), field(error, "message"))
}

/// One field of an object, as a cell.
fn field(v: &Value, key: &str) -> String {
    v.get(key).map_or_else(|| "-".to_owned(), cell)
}

/// A value on its way into a report, sanitised.
///
/// Absent, null and empty all render as `-`. An archive that set no title and
/// one that set an empty title are the same fact to a reader, and a cell that
/// renders as nothing reads as a layout bug.
fn cell(v: &Value) -> String {
    let text = text::line(&v.to_line());
    if text.is_empty() {
        "-".to_owned()
    } else {
        text
    }
}

/// A title cell, elided to `width`.
fn title(v: &Value, width: usize) -> String {
    let text = text::elide(v.get("title").and_then(Value::as_str).unwrap_or(""), width);
    if text.is_empty() {
        "-".to_owned()
    } else {
        text
    }
}

/// A count with no thousands separator: it is a number a script may compare.
fn count(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |n| n.to_string())
}

fn indent(block: &str) -> String {
    block.lines().map(|l| format!("  {l}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn status_names_every_layer_and_its_state() {
        let v = parse(
            r#"{"version":"0.1.0","uptime_seconds":11520,"listener":"unix:/run/cairn/cairn.sock",
                "archives":2,"auth":"none",
                "sandbox":{"required":true,"layers":[
                  {"name":"no_new_privs","state":"applied","detail":null},
                  {"name":"landlock","state":"applied","detail":"abi 6"},
                  {"name":"seccomp","state":"failed","detail":"kernel too old"}]},
                "cache":{"budget_bytes":67108864,"bytes":0,"entries":0,"hits":0,"misses":0,
                         "evictions":0},
                "connections":{"max":64,"active":1,"served":7,"rejected":0}}"#,
        )
        .unwrap();
        let out = status(&v);
        assert!(out.contains("uptime          3h12m"), "{out}");
        assert!(out.contains("sandbox         required"), "{out}");
        assert!(out.contains("  no_new_privs  applied\n"), "{out}");
        assert!(out.contains("  landlock      applied (abi 6)"), "{out}");
        // The one line this command exists to make visible.
        assert!(
            out.contains("  seccomp       failed (kernel too old)"),
            "{out}"
        );
        assert!(out.contains("1 active of 64"), "{out}");
        assert!(out.contains("0 B of 64.0 MiB in 0 clusters"), "{out}");
    }

    #[test]
    fn status_survives_a_daemon_that_reports_less() {
        // Rendering must not depend on fields an older or partial answer omits.
        assert!(!status(&parse(r#"{"version":"0.1.0"}"#).unwrap()).is_empty());
        assert!(!status(&parse("{}").unwrap()).is_empty());
    }

    #[test]
    fn archives_are_a_table_and_an_empty_one_says_so() {
        let v = parse(
            r#"{"archives":[{"uuid":"b10d","title":"Climate Change","entry_count":20317,
                             "cluster_count":389,"suggest":true}]}"#,
        )
        .unwrap();
        let out = archives(&v);
        assert!(
            out.starts_with("UUID  ENTRIES  CLUSTERS  SUGGEST  TITLE\n"),
            "{out}"
        );
        assert!(
            out.contains("b10d    20317       389  yes      Climate Change"),
            "{out}"
        );
        assert_eq!(
            archives(&parse(r#"{"archives":[]}"#).unwrap()),
            "no archives\n"
        );
    }

    #[test]
    fn an_archive_shows_its_metadata_under_it() {
        let v = parse(
            r#"{"uuid":"b10d","title":"Climate Change","entry_count":20317,"cluster_count":389,
                "main_page":"index.html","format_version":"6.3","content_namespace":"C",
                "suggest":true,"metadata":{"Language":"eng","Date":"2026-07-01"},
                "binary_metadata":["Illustration_48x48@1"]}"#,
        )
        .unwrap();
        let out = archive(&v);
        assert!(out.contains("content_namespace  C\n"), "{out}");
        assert!(out.contains("suggest            yes\n"), "{out}");
        assert!(out.contains("\nmetadata:\n  NAME      VALUE\n"), "{out}");
        assert!(out.contains("  Language  eng\n"), "{out}");
        assert!(
            out.contains("\nbinary metadata:\n  Illustration_48x48@1\n"),
            "{out}"
        );
    }

    #[test]
    fn an_empty_suggestion_list_says_which_kind_of_empty() {
        let none = parse(r#"{"archive":"b10d","suggestions":[]}"#).unwrap();
        assert_eq!(suggest(&none, false), "no suggestions\n");
        assert_eq!(
            suggest(&none, true),
            "no suggestions: this archive carries no title index\n"
        );
    }

    #[test]
    fn a_random_path_is_ready_to_be_fetched() {
        let v = parse(r#"{"archive":"b10d","path":"Climate_change"}"#).unwrap();
        assert_eq!(random(&v), "Climate_change\n");
    }

    #[test]
    fn an_archive_cannot_forge_output() {
        // Every one of these is a string an archive gets to choose, and this is
        // how the daemon writes an escape and a newline on the wire.
        let v = parse(
            r#"{"archives":[{"uuid":"b10d","title":"a\u001b[2Jb\nc","entry_count":1,
                             "cluster_count":1,"suggest":false}]}"#,
        )
        .unwrap();
        let out = archives(&v);
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert_eq!(out.lines().count(), 2, "{out:?}");
        assert!(out.contains("a.[2Jb.c"), "{out:?}");
    }

    #[test]
    fn a_fault_becomes_one_line() {
        let body = br#"{"error":{"code":"not_found","message":"no such resource"}}"#;
        assert_eq!(fault(body, 404), "not_found: no such resource");
        // A body that is not an error document still has to say something.
        assert_eq!(fault(b"", 503), "the daemon answered 503");
        assert_eq!(fault(b"<html>", 500), "the daemon answered 500");
    }
}
