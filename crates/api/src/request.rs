//! HTTP/1.1 request parsing from raw socket bytes.
//!
//! Fuzz target B enters here. Nothing assumes a well-formed request is the
//! common case, and no request body is ever accepted.

use crate::limits::Limits;

/// Methods cairn answers. Everything else is carried through to a 405.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Other,
}

/// Why a request could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The header block is not complete yet. Read more bytes.
    Incomplete,
    /// A bound was crossed.
    TooLong(&'static str),
    /// Syntax error.
    Malformed(&'static str),
    /// Not HTTP/1.1.
    UnsupportedVersion,
    /// A body was announced. No endpoint accepts one.
    BodyNotAllowed,
}

/// A parsed request. The target is kept raw; only handlers decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub target: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub keep_alive: bool,
}

impl Request {
    /// Parse one request from `buf`, returning it and the bytes consumed.
    pub fn parse(buf: &[u8], limits: &Limits) -> Result<(Request, usize), ParseError> {
        Request::parse_hinted(buf, limits, 0)
    }

    /// As [`Request::parse`], but resuming the header-end scan at `scanned`.
    ///
    /// A caller reading a byte at a time would otherwise rescan the whole
    /// buffer on every read.
    pub fn parse_hinted(
        buf: &[u8],
        limits: &Limits,
        scanned: usize,
    ) -> Result<(Request, usize), ParseError> {
        let Some(end) = find_terminator(buf, scanned) else {
            if buf.len() > limits.max_request_line + limits.max_header_bytes {
                return Err(ParseError::TooLong("headers"));
            }
            return Err(ParseError::Incomplete);
        };
        // Keep the CRLF that ends the last header line so every line is uniform.
        let block = &buf[..end + 2];
        let consumed = end + 4;

        let mut lines = block.split(|&c| c == b'\n');
        let request_line = lines.next().ok_or(ParseError::Malformed("no request line"))?;
        let request_line = strip_cr(request_line)?;
        if request_line.len() > limits.max_request_line {
            return Err(ParseError::TooLong("request line"));
        }

        let (method, target) = parse_request_line(request_line)?;
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
            None => (target.clone(), None),
        };

        let mut headers: Vec<(String, String)> = Vec::new();
        let mut header_bytes = 0usize;
        for line in lines {
            let line = strip_cr(line)?;
            if line.is_empty() {
                continue;
            }
            header_bytes += line.len() + 2;
            if header_bytes > limits.max_header_bytes {
                return Err(ParseError::TooLong("headers"));
            }
            if headers.len() == limits.max_headers {
                return Err(ParseError::TooLong("header count"));
            }
            if line[0] == b' ' || line[0] == b'\t' {
                // Obsolete line folding: a request smuggling primitive.
                return Err(ParseError::Malformed("folded header"));
            }
            let colon = line
                .iter()
                .position(|&c| c == b':')
                .ok_or(ParseError::Malformed("header without colon"))?;
            let name = &line[..colon];
            if name.is_empty() || !name.iter().all(|&c| crate::token::is_token_byte(c)) {
                return Err(ParseError::Malformed("header name"));
            }
            let value = &line[colon + 1..];
            if !value.iter().all(|&c| c == b'\t' || (0x20..0x7f).contains(&c)) {
                return Err(ParseError::Malformed("header value"));
            }
            let value = std::str::from_utf8(value)
                .map_err(|_| ParseError::Malformed("header value"))?
                .trim_matches([' ', '\t'])
                .to_owned();
            let name = String::from_utf8(name.to_vec())
                .map_err(|_| ParseError::Malformed("header name"))?;
            headers.push((name, value));
        }

        let request = Request { method, target, path, query, headers, keep_alive: true };
        request.check_semantics()?;
        let keep_alive = !request
            .header("connection")
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
        Ok((Request { keep_alive, ..request }, consumed))
    }

    /// First value of a header, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn count_header(&self, name: &str) -> usize {
        self.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).count()
    }

    fn check_semantics(&self) -> Result<(), ParseError> {
        match self.count_header("host") {
            1 => {}
            0 => return Err(ParseError::Malformed("missing host")),
            _ => return Err(ParseError::Malformed("duplicate host")),
        }
        if self.count_header("content-length") > 1 {
            return Err(ParseError::Malformed("duplicate content-length"));
        }
        if self.header("transfer-encoding").is_some() {
            return Err(ParseError::BodyNotAllowed);
        }
        if let Some(len) = self.header("content-length") {
            match len.parse::<u64>() {
                Ok(0) => {}
                Ok(_) => return Err(ParseError::BodyNotAllowed),
                Err(_) => return Err(ParseError::Malformed("content-length")),
            }
        }
        Ok(())
    }
}

fn parse_request_line(line: &[u8]) -> Result<(Method, String), ParseError> {
    let mut parts = line.split(|&c| c == b' ');
    let method = parts.next().ok_or(ParseError::Malformed("method"))?;
    let target = parts.next().ok_or(ParseError::Malformed("target"))?;
    let version = parts.next().ok_or(ParseError::Malformed("version"))?;
    if parts.next().is_some() {
        return Err(ParseError::Malformed("request line"));
    }
    if method.is_empty() || !method.iter().all(|&c| crate::token::is_token_byte(c)) {
        return Err(ParseError::Malformed("method"));
    }
    if version != b"HTTP/1.1" {
        return Err(ParseError::UnsupportedVersion);
    }
    // Origin form only: no absolute-form, no authority-form, no asterisk-form.
    if target.first() != Some(&b'/') {
        return Err(ParseError::Malformed("target"));
    }
    if !target.iter().all(|&c| (0x21..0x7f).contains(&c)) {
        return Err(ParseError::Malformed("target"));
    }
    let target =
        String::from_utf8(target.to_vec()).map_err(|_| ParseError::Malformed("target"))?;
    let method = match method {
        b"GET" => Method::Get,
        b"HEAD" => Method::Head,
        _ => Method::Other,
    };
    Ok((method, target))
}

fn strip_cr(line: &[u8]) -> Result<&[u8], ParseError> {
    match line.split_last() {
        Some((b'\r', rest)) => Ok(rest),
        Some(_) => Err(ParseError::Malformed("bare LF")),
        None => Ok(line),
    }
}

fn find_terminator(buf: &[u8], from: usize) -> Option<usize> {
    let from = from.saturating_sub(3).min(buf.len());
    buf.get(from..)?.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<(Request, usize), ParseError> {
        Request::parse(raw.as_bytes(), &Limits::default())
    }

    #[test]
    fn parses_a_plain_request() {
        let (r, n) = parse("GET /v1/status HTTP/1.1\r\nHost: cairn\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.path, "/v1/status");
        assert_eq!(r.query, None);
        assert_eq!(r.header("host"), Some("cairn"));
        assert!(r.keep_alive);
        assert_eq!(n, "GET /v1/status HTTP/1.1\r\nHost: cairn\r\n\r\n".len());
    }

    #[test]
    fn splits_the_query() {
        let (r, _) = parse("GET /v1/x?q=a%20b&n=2 HTTP/1.1\r\nHost: c\r\n\r\n").unwrap();
        assert_eq!(r.path, "/v1/x");
        assert_eq!(r.query.as_deref(), Some("q=a%20b&n=2"));
    }

    #[test]
    fn incomplete_requests_ask_for_more() {
        assert_eq!(parse("GET / HTTP/1.1\r\nHost: c\r\n"), Err(ParseError::Incomplete));
        assert_eq!(parse(""), Err(ParseError::Incomplete));
    }

    #[test]
    fn rejects_bodies() {
        assert_eq!(
            parse("GET / HTTP/1.1\r\nHost: c\r\nContent-Length: 5\r\n\r\n"),
            Err(ParseError::BodyNotAllowed)
        );
        assert_eq!(
            parse("GET / HTTP/1.1\r\nHost: c\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(ParseError::BodyNotAllowed)
        );
        // A zero-length body is not a body.
        assert!(parse("GET / HTTP/1.1\r\nHost: c\r\nContent-Length: 0\r\n\r\n").is_ok());
    }

    #[test]
    fn rejects_smuggling_shapes() {
        assert_eq!(
            parse("GET / HTTP/1.1\r\nHost: c\r\nContent-Length: 1\r\nContent-Length: 0\r\n\r\n"),
            Err(ParseError::Malformed("duplicate content-length"))
        );
        assert_eq!(
            parse("GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
            Err(ParseError::Malformed("duplicate host"))
        );
        assert_eq!(
            parse("GET / HTTP/1.1\r\nHost: c\r\nX: a\r\n b\r\n\r\n"),
            Err(ParseError::Malformed("folded header"))
        );
        assert_eq!(parse("GET / HTTP/1.1\r\n\r\n"), Err(ParseError::Malformed("missing host")));
    }

    #[test]
    fn rejects_bare_lf() {
        assert_eq!(
            Request::parse(b"GET / HTTP/1.1\nHost: c\r\n\r\n", &Limits::default()),
            Err(ParseError::Malformed("bare LF"))
        );
    }

    #[test]
    fn rejects_other_versions() {
        assert_eq!(
            parse("GET / HTTP/1.0\r\nHost: c\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
        assert_eq!(
            parse("GET / HTTP/2.0\r\nHost: c\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_non_origin_targets() {
        assert_eq!(
            parse("GET http://evil/ HTTP/1.1\r\nHost: c\r\n\r\n"),
            Err(ParseError::Malformed("target"))
        );
        assert_eq!(parse("OPTIONS * HTTP/1.1\r\nHost: c\r\n\r\n"), Err(ParseError::Malformed("target")));
    }

    #[test]
    fn enforces_bounds() {
        let limits = Limits { max_headers: 2, ..Limits::default() };
        let raw = "GET / HTTP/1.1\r\nHost: c\r\nA: 1\r\nB: 2\r\n\r\n";
        assert_eq!(
            Request::parse(raw.as_bytes(), &limits),
            Err(ParseError::TooLong("header count"))
        );

        let long = format!("GET /{} HTTP/1.1\r\nHost: c\r\n\r\n", "a".repeat(9000));
        assert_eq!(parse(&long), Err(ParseError::TooLong("request line")));

        let many = format!("GET / HTTP/1.1\r\nHost: c\r\nX: {}\r\n\r\n", "a".repeat(20000));
        assert_eq!(parse(&many), Err(ParseError::TooLong("headers")));
    }

    #[test]
    fn unknown_methods_survive_to_the_router() {
        let (r, _) = parse("DELETE / HTTP/1.1\r\nHost: c\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Other);
    }

    #[test]
    fn connection_close_is_honoured() {
        let (r, _) = parse("GET / HTTP/1.1\r\nHost: c\r\nConnection: close\r\n\r\n").unwrap();
        assert!(!r.keep_alive);
        let (r, _) =
            parse("GET / HTTP/1.1\r\nHost: c\r\nConnection: keep-alive, x\r\n\r\n").unwrap();
        assert!(r.keep_alive);
    }

    #[test]
    fn embedded_nul_and_control_bytes_are_refused() {
        assert!(Request::parse(b"GET /a\0b HTTP/1.1\r\nHost: c\r\n\r\n", &Limits::default()).is_err());
        assert!(
            Request::parse(b"GET / HTTP/1.1\r\nHost: c\r\nX: a\0b\r\n\r\n", &Limits::default())
                .is_err()
        );
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        let seeds: [&[u8]; 8] = [
            b"",
            b"\r\n\r\n",
            b" \r\n\r\n",
            b"GET\r\n\r\n",
            b"GET  HTTP/1.1\r\nHost: c\r\n\r\n",
            b"GET / HTTP/1.1\r\n:\r\n\r\n",
            b"\xff\xfe\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: c\r\n\r\nGET / HTTP/1.1\r\nHost: c\r\n\r\n",
        ];
        for s in seeds {
            let _ = Request::parse(s, &Limits::default());
        }
    }
}
