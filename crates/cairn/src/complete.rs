//! Completion candidates for shell line editors.
//!
//! A shell asks `cairn __complete WORDS...`, where WORDS is the line the
//! person is typing — global options included — with the word under the cursor
//! last, empty if nothing is typed yet. The answer is one candidate per line,
//! `value<TAB>description`; anything the shell cannot learn statically comes
//! from the live daemon through the same API every other client uses.
//!
//! Three properties matter. It is **silent**: an unreachable daemon, an auth
//! failure, or a malformed answer all mean no candidates, never output on
//! standard error and never a nonzero exit, because a shell calls this on
//! every keypress and nobody wants an error mid-word. It is **scrubbed**:
//! titles and paths came out of an archive ([`SECURITY.md`]), so they go
//! through [`text::line`] before they enter the line protocol, whose one
//! separator is the tab. And it is **small**: the planner below is a pure
//! function of the words, tested against a table, so the only untested part
//! is the request itself, which the integration tests drive end to end.
//!
//! [`SECURITY.md`]: ../../../SECURITY.md

use std::io::Write;

use crate::json::{self, Value};
use crate::text;

/// Subcommands, with the same one-line summaries as the usage message.
const COMMANDS: &[(&str, &str)] = &[
    ("status", "daemon state and the sandbox actually applied"),
    ("archives", "open archives"),
    ("archive", "one archive and its metadata"),
    ("get", "entry content, written to stdout"),
    ("head", "entry headers only"),
    ("suggest", "title-prefix suggestions"),
    ("random", "one random entry path"),
    ("raw", "any request, for debugging"),
];

/// Options accepted before the command word.
const FLAGS: &[(&str, &str)] = &[
    ("-s", "unix socket (default /run/cairn/cairn.sock)"),
    ("--socket", "unix socket (default /run/cairn/cairn.sock)"),
    ("-a", "loopback TCP address instead of a socket"),
    ("--address", "loopback TCP address instead of a socket"),
    ("-t", "bearer token"),
    ("--token", "bearer token"),
    ("--timeout", "read and write timeout, seconds"),
    ("--json", "print the daemon's JSON instead of a report"),
    ("-V", "print the version and exit"),
    ("--version", "print the version and exit"),
    ("-h", "print the usage and exit"),
    ("--help", "print the usage and exit"),
];

/// After the command word only `--json` still means anything.
const LATE_FLAGS: &[(&str, &str)] = &[("--json", "print the daemon's JSON instead of a report")];

/// Methods `raw` forwards.
const METHODS: &[(&str, &str)] = &[("GET", "read the target"), ("HEAD", "headers only")];

/// What the word under the cursor asks for.
#[derive(Debug, PartialEq)]
pub(crate) enum Query {
    /// Nothing to offer; the shell falls back to filenames.
    Nothing,
    /// Fixed words, filtered by the prefix before printing.
    Static(&'static [(&'static str, &'static str)]),
    /// Open archives: values are UUIDs, descriptions their titles.
    Archives,
    /// Entry paths of one archive by way of its title prefixes.
    Paths { uuid: String },
}

/// Which global option consumes a following word.
fn takes_value(word: &str) -> bool {
    matches!(
        word,
        "-s" | "--socket" | "-a" | "--address" | "-t" | "--token" | "--timeout"
    )
}

fn is_flag(word: &str) -> bool {
    word.starts_with('-') && word.len() > 1
}

/// Read the line and decide what the last word wants to be.
///
/// Global options are skipped wherever they appear, because the shell sends
/// the whole line and people put them anywhere. A word that begins with `-`
/// offers options; a command word picks the argument positions after it.
pub(crate) fn plan(words: &[&str]) -> Query {
    let Some(partial_at) = words.len().checked_sub(1) else {
        return Query::Nothing;
    };

    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        if !is_flag(word) {
            break;
        }
        if takes_value(word) {
            // Its value may be the word under the cursor: completing a socket
            // path or an address is the shell's filename job, not ours.
            if i + 1 >= partial_at {
                return Query::Nothing;
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    // Still before the command: the word under the cursor is the command
    // itself, or an option.
    if i >= partial_at {
        return if words[partial_at].starts_with('-') {
            Query::Static(FLAGS)
        } else {
            Query::Static(COMMANDS)
        };
    }

    let command = words[i];
    let args = &words[i + 1..];
    let position = args.len() - 1;
    let prefix = args[position];

    if prefix.starts_with('-') {
        return Query::Static(LATE_FLAGS);
    }
    match (command, position) {
        ("archive" | "get" | "head" | "suggest" | "random", 0) => Query::Archives,
        ("raw", 0) => Query::Static(METHODS),
        ("get" | "head" | "suggest", 1) => Query::Paths {
            uuid: args[0].to_owned(),
        },
        _ => Query::Nothing,
    }
}

/// One output line: the value a shell will insert, then its description.
///
/// Both halves came from the archive, so both go through [`text::line`],
/// which also removes the tabs and newlines the line protocol is built on.
fn candidate(value: &str, description: &str) -> String {
    format!("{}\t{}", text::line(value), text::line(description))
}

/// Fixed words matching the prefix, in table order.
fn static_candidates(list: &'static [(&'static str, &'static str)], prefix: &str) -> Vec<String> {
    list.iter()
        .filter(|(value, _)| value.starts_with(prefix))
        .map(|(value, description)| candidate(value, description))
        .collect()
}

/// UUIDs from `/v1/archives`, filtered by prefix, described by title.
///
/// `None` means the answer was not the documented shape; the caller treats
/// that exactly like an empty list.
fn archive_candidates(body: &str, prefix: &str) -> Option<Vec<String>> {
    let value = json::parse(body).ok()?;
    let archives = value.get("archives")?.as_array()?;
    let mut out = Vec::new();
    for archive in archives {
        let Some(uuid) = archive.get("uuid").and_then(Value::as_str) else {
            continue;
        };
        if !uuid.starts_with(prefix) {
            continue;
        }
        let title = archive.get("title").and_then(Value::as_str).unwrap_or("");
        out.push(candidate(uuid, title));
    }
    Some(out)
}

/// Paths from `/suggest`: the daemon matched on title prefix already, so the
/// paths often do not begin with what was typed, and no second filtering
/// would be right. Order is the daemon's, which is title order.
fn path_candidates(body: &str) -> Option<Vec<String>> {
    let value = json::parse(body).ok()?;
    let suggestions = value.get("suggestions")?.as_array()?;
    let mut out = Vec::new();
    for suggestion in suggestions {
        let Some(path) = suggestion.get("path").and_then(Value::as_str) else {
            continue;
        };
        let title = suggestion
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push(candidate(path, title));
    }
    Some(out)
}

/// One request, answered with bytes only on a plain 200.
fn fetch(client: &crate::Client, target: &str) -> Option<Vec<u8>> {
    let reply = client.request("GET", target).ok()?;
    (reply.status == 200).then_some(reply.body)
}

/// The connection options a forwarded line carries.
struct Options {
    socket: Option<String>,
    address: Option<String>,
    token: Option<String>,
}

/// Split the forwarded words into their leading options and the rest.
///
/// The shell sends the person's line as they typed it, options included, and
/// those are the ones that say which daemon to ask. The last word is never
/// an option: it is whatever is under the cursor, even mid-flag (`--so`),
/// and it stays in the remainder for [`plan`] to judge.
fn split<'a>(words: &'a [&'a str]) -> (Options, &'a [&'a str]) {
    let mut options = Options {
        socket: None,
        address: None,
        token: None,
    };
    let end = words.len().saturating_sub(1);
    let mut i = 0;
    while i < end {
        let word = words[i];
        if !is_flag(word) {
            break;
        }
        if takes_value(word) {
            // Its value may be the word under the cursor: completing a
            // socket path or an address is the shell's filename job, not
            // ours, so an incomplete pair is left alone.
            let Some(value) = words.get(i + 1).filter(|_| i + 1 < end) else {
                break;
            };
            match word {
                "-s" | "--socket" => options.socket = Some((*value).to_owned()),
                "-a" | "--address" => options.address = Some((*value).to_owned()),
                "-t" | "--token" => options.token = Some((*value).to_owned()),
                // `--timeout`: its value is consumed, the caller's own
                // completion timeout stands.
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    (options, &words[i..])
}

/// Answer a `__complete` invocation: candidates on stdout, silence otherwise.
///
/// Every failure path prints nothing and returns 0. Completion runs dozens of
/// times a minute against whatever daemon happens to be configured; it is a
/// convenience, not a command whose diagnosis anyone will read.
pub(crate) fn run(
    endpoint: crate::Endpoint,
    token: Option<&str>,
    timeout: std::time::Duration,
    words: &[String],
) -> i32 {
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let (options, line) = split(&refs);
    let endpoint = if let Some(path) = options.socket {
        crate::Endpoint::Unix(std::path::PathBuf::from(path))
    } else if let Some(address) = options.address {
        crate::Endpoint::Tcp(address)
    } else {
        endpoint
    };
    let client = crate::Client {
        endpoint,
        token: options.token.or_else(|| token.map(str::to_owned)),
        timeout,
    };

    let query = plan(line);
    let prefix = line.last().map_or("", |w| *w);

    let lines = match query {
        Query::Nothing => Vec::new(),
        Query::Static(list) => static_candidates(list, prefix),
        Query::Archives => fetch(&client, "/v1/archives")
            .and_then(|body| String::from_utf8(body).ok())
            .and_then(|text| archive_candidates(&text, prefix))
            .unwrap_or_default(),
        Query::Paths { ref uuid } => {
            let target = format!(
                "/v1/archives/{}/suggest?q={}",
                crate::encode(uuid),
                crate::encode(prefix)
            );
            fetch(&client, &target)
                .and_then(|body| String::from_utf8(body).ok())
                .and_then(|text| path_candidates(&text))
                .unwrap_or_default()
        }
    };

    let mut out = std::io::stdout().lock();
    for line in &lines {
        let _ = writeln!(out, "{line}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_line_offers_the_commands() {
        assert_eq!(plan(&[""]), Query::Static(COMMANDS));
        assert_eq!(plan(&["--json", ""]), Query::Static(COMMANDS));
        assert_eq!(plan(&["-s", "/tmp/s.sock", ""]), Query::Static(COMMANDS));
    }

    #[test]
    fn a_dash_offers_options_before_the_command() {
        assert_eq!(plan(&["--js"]), Query::Static(FLAGS));
        assert_eq!(plan(&["-s", "/tmp/s.sock", "--to"]), Query::Static(FLAGS));
        // After the command word, only `--json` means anything.
        assert_eq!(plan(&["archives", "--j"]), Query::Static(LATE_FLAGS));
        assert_eq!(plan(&["get", SAMPLE, "--j"]), Query::Static(LATE_FLAGS));
    }

    const SAMPLE: &str = "63616972-6e2d-7465-7374-2d7575696431";

    #[test]
    fn commands_that_take_an_archive_ask_for_it() {
        for command in ["archive", "get", "head", "suggest", "random"] {
            assert_eq!(plan(&[command, ""]), Query::Archives, "{command}");
            assert_eq!(plan(&[command, "63616972"]), Query::Archives, "{command}");
        }
    }

    #[test]
    fn entry_positions_ask_for_paths_through_the_archive() {
        assert_eq!(
            plan(&["get", SAMPLE, ""]),
            Query::Paths {
                uuid: SAMPLE.to_owned()
            }
        );
        assert_eq!(
            plan(&["-t", "tok", "suggest", SAMPLE, "Clim"]),
            Query::Paths {
                uuid: SAMPLE.to_owned()
            }
        );
        assert_eq!(
            plan(&["head", SAMPLE, "index.html"]),
            Query::Paths {
                uuid: SAMPLE.to_owned()
            }
        );
    }

    #[test]
    fn positions_with_nothing_to_say_say_nothing() {
        assert_eq!(plan(&["status", ""]), Query::Nothing);
        assert_eq!(plan(&["archives", "x"]), Query::Nothing);
        assert_eq!(plan(&["get", SAMPLE, "p", "extra"]), Query::Nothing);
        assert_eq!(plan(&["suggest", SAMPLE, "q", "12"]), Query::Nothing);
        assert_eq!(plan(&["no-such-command", ""]), Query::Nothing);
        assert_eq!(plan(&["raw", SAMPLE, "/x"]), Query::Nothing);
    }

    #[test]
    fn raw_method_position_offers_methods() {
        assert_eq!(plan(&["raw", "GE"]), Query::Static(METHODS));
    }

    #[test]
    fn completing_an_option_value_leaves_it_to_the_shell() {
        assert_eq!(plan(&["-s", ""]), Query::Nothing);
        assert_eq!(plan(&["-t", "se"]), Query::Nothing);
        assert_eq!(plan(&["--socket", "/tmp/ne"]), Query::Nothing);
    }

    #[test]
    fn fixed_words_are_filtered_by_the_prefix() {
        let got = static_candidates(COMMANDS, "ar");
        // Table order, which is usage-message order.
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0], "archives\topen archives");
        assert_eq!(got[1], "archive\tone archive and its metadata");
        assert_eq!(
            static_candidates(FLAGS, "--ver"),
            vec!["--version\tprint the version and exit"]
        );
        assert!(static_candidates(FLAGS, "--zzz").is_empty());
    }

    #[test]
    fn uuids_are_read_from_the_archives_answer() {
        let body = concat!(
            r#"{"archives":["#,
            r#"{"uuid":"aaaa-bbbb","title":"Climate Change","entry_count":20317,"suggest":true},"#,
            r#"{"uuid":"cccc-dddd","title":"","entry_count":1,"suggest":false}"#,
            r#"]}"#
        );
        let got = archive_candidates(body, "").expect("shape");
        assert_eq!(got, vec!["aaaa-bbbb\tClimate Change", "cccc-dddd\t"]);
        assert!(archive_candidates(body, "a").unwrap().len() == 1);
        assert!(archive_candidates(body, "z").unwrap().is_empty());
    }

    #[test]
    fn a_wrong_shape_is_reported_as_such() {
        for bad in [
            "",
            "{}",
            r#"{"archives":{}}"#,
            "not json",
            r#"{"archives":[3]}"#,
        ] {
            assert!(
                archive_candidates(bad, "").is_none()
                    || archive_candidates(bad, "").unwrap().is_empty(),
                "{bad}"
            );
        }
    }

    #[test]
    fn paths_keep_the_daemons_order_and_titles() {
        let body = concat!(
            r#"{"archive":"u","suggestions":["#,
            r#"{"title":"Climate change","path":"Climate_change"},"#,
            r#"{"title":"Climate change denial","path":"Climate_change_denial"}"#,
            r#"]}"#
        );
        let got = path_candidates(body).expect("shape");
        assert_eq!(
            got,
            vec![
                "Climate_change\tClimate change",
                "Climate_change_denial\tClimate change denial",
            ]
        );
    }

    #[test]
    fn archive_text_is_scrubbed_on_its_way_into_the_protocol() {
        let body = r#"{"suggestions":[{"title":"a\tb\u001b[2Jc","path":"p"}]}"#;
        let got = path_candidates(body).expect("shape");
        assert_eq!(got, vec!["p\ta.b.[2Jc"]);
        // The same holds for a UUID column fed by a hostile summary, and for
        // fixed tables whose descriptions are ours.
        let body = r#"{"archives":[{"uuid":"u\u0007","title":"t\n"}]}"#;
        let got = archive_candidates(body, "").expect("shape");
        assert_eq!(got, vec!["u.\tt."]);
        assert_eq!(candidate("v", "d\u{202e}X"), "v\td.X");
    }

    #[test]
    fn the_partial_word_is_always_the_last_one() {
        // A middle word being edited is not modelled; the shell sends the
        // whole line and the last element governs. These all stay sane.
        assert_eq!(
            plan(&["get", SAMPLE]),
            Query::Archives,
            "no trailing empty word: completing the uuid slot"
        );
    }

    #[test]
    fn options_are_read_from_the_forwarded_line() {
        let words = ["-s", "/tmp/s.sock", "archive", "0e7"];
        let (options, rest) = split(&words);
        assert_eq!(options.socket.as_deref(), Some("/tmp/s.sock"));
        assert_eq!(options.token, None);
        assert_eq!(rest, &["archive", "0e7"]);

        // A token meant for the daemon is captured, not mistaken for text;
        // `--json` and its kind are skipped wherever they sit.
        let words = ["--json", "-t", "sekrit", "status", ""];
        let (options, rest) = split(&words);
        assert_eq!(options.token.as_deref(), Some("sekrit"));
        assert_eq!(rest, &["status", ""]);
    }

    #[test]
    fn the_word_under_the_cursor_is_never_an_option() {
        // Mid-flag: completing "--so" must offer flags, not swallow the word
        // as a finished one and answer nothing.
        let (options, rest) = split(&["--so"]);
        assert!(options.socket.is_none());
        assert_eq!(rest, &["--so"]);
        assert_eq!(plan(rest), Query::Static(FLAGS));

        // An option whose value is still being typed stays untouched too.
        let (options, rest) = split(&["-s", "/tmp/ne"]);
        assert!(options.socket.is_none(), "the value was under the cursor");
        assert_eq!(plan(rest), Query::Nothing);
    }
}
