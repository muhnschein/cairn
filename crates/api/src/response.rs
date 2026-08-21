//! Response construction and serialization.
//!
//! The daemon writes the bytes; this crate never touches a socket.

use std::sync::Arc;

/// Bytes kept alive by an `Arc`: a mapped archive, or a decoded cluster.
#[derive(Clone)]
pub struct SharedBytes {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    start: usize,
    end: usize,
}

impl SharedBytes {
    /// Reference `data[start..end]`, keeping `data` alive.
    pub fn new(data: Arc<dyn AsRef<[u8]> + Send + Sync>, start: usize, end: usize) -> SharedBytes {
        SharedBytes { data, start, end }
    }

    /// The bytes, empty if the range no longer fits its backing store.
    pub fn as_slice(&self) -> &[u8] {
        (*self.data)
            .as_ref()
            .get(self.start..self.end)
            .unwrap_or(&[])
    }

    /// Narrow to a sub-range, for `Range` responses.
    pub fn subrange(&self, from: u64, to: u64) -> SharedBytes {
        let from = usize::try_from(from).unwrap_or(usize::MAX);
        let to = usize::try_from(to).unwrap_or(usize::MAX);
        let start = self.start.saturating_add(from).min(self.end);
        let end = self.start.saturating_add(to).min(self.end);
        SharedBytes {
            data: Arc::clone(&self.data),
            start,
            end: end.max(start),
        }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// True for an empty range.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for SharedBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBytes")
            .field("len", &self.len())
            .finish()
    }
}

/// What follows the response head.
#[derive(Debug, Clone)]
pub enum Payload {
    /// No body.
    Empty,
    /// A body built by the API: JSON, mostly.
    Owned(Vec<u8>),
    /// Entry content, not copied.
    Shared(SharedBytes),
}

impl Payload {
    /// Body length in bytes.
    pub fn len(&self) -> usize {
        match self {
            Payload::Empty => 0,
            Payload::Owned(v) => v.len(),
            Payload::Shared(s) => s.len(),
        }
    }

    /// True when there is nothing to write.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The body bytes.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Payload::Empty => &[],
            Payload::Owned(v) => v,
            Payload::Shared(s) => s.as_slice(),
        }
    }
}

/// A complete response, ready for the daemon to write.
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Headers in the order they will be written.
    pub headers: Vec<(String, String)>,
    /// Body, owned or shared with the mapping.
    pub payload: Payload,
    /// False for `HEAD`: `Content-Length` still describes the entry.
    pub send_body: bool,
    /// Whether the connection may be reused after this response.
    pub keep_alive: bool,
}

impl Response {
    /// An empty response with the given status.
    pub fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            payload: Payload::Empty,
            send_body: true,
            keep_alive: true,
        }
    }

    /// Add a header. Values are written as given, so callers sanitize first.
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Response {
        self.headers.push((name.to_owned(), value.into()));
        self
    }

    /// Attach an owned body.
    pub fn body(mut self, payload: Payload) -> Response {
        self.payload = payload;
        self
    }

    /// A JSON body with the right content type.
    pub fn json(self, bytes: Vec<u8>) -> Response {
        self.header("Content-Type", "application/json")
            .body(Payload::Owned(bytes))
    }

    /// Serialize the status line and headers.
    pub fn head_bytes(&self) -> Vec<u8> {
        let mut out = String::with_capacity(256);
        out.push_str("HTTP/1.1 ");
        out.push_str(&self.status.to_string());
        out.push(' ');
        out.push_str(reason(self.status));
        out.push_str("\r\n");
        for (name, value) in &self.headers {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push_str("\r\n");
        }
        out.push_str("Content-Length: ");
        out.push_str(&self.payload.len().to_string());
        out.push_str("\r\n");
        out.push_str("Connection: ");
        out.push_str(if self.keep_alive {
            "keep-alive"
        } else {
            "close"
        });
        out.push_str("\r\n\r\n");
        out.into_bytes()
    }
}

/// Reason phrase for the status codes cairn emits.
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        414 => "URI Too Long",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fault;

    #[test]
    fn serializes_a_head() {
        let r = Response::new(200).header("Content-Type", "text/html");
        let head = String::from_utf8(r.head_bytes()).unwrap();
        assert_eq!(
            head,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n"
        );
    }

    /// `HEAD` must describe the entry it would have sent, so a client can
    /// size a download without making it. The body is suppressed at the
    /// writer, not by zeroing the length.
    #[test]
    fn a_head_response_keeps_the_length_of_the_body_it_withholds() {
        let mut r = Response::new(200).body(Payload::Owned(b"0123456789".to_vec()));
        r.send_body = false;
        let head = String::from_utf8(r.head_bytes()).unwrap();
        assert!(head.contains("Content-Length: 10\r\n"), "{head}");
        assert_eq!(r.payload.len(), 10);
    }

    #[test]
    fn a_closing_response_says_so() {
        let mut r = Response::new(503);
        r.keep_alive = false;
        let head = String::from_utf8(r.head_bytes()).unwrap();
        assert!(
            head.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "{head}"
        );
        assert!(head.contains("Connection: close\r\n"), "{head}");
        assert!(head.ends_with("\r\n\r\n"), "{head}");
    }

    #[test]
    fn an_empty_body_still_carries_a_length() {
        // Absent `Content-Length` on a keep-alive connection makes the next
        // response look like this one's body.
        let head = String::from_utf8(Response::new(404).head_bytes()).unwrap();
        assert!(head.contains("Content-Length: 0\r\n"), "{head}");
    }

    #[test]
    fn every_status_the_api_emits_has_a_reason_phrase() {
        for f in [
            Fault::BadRequest,
            Fault::BadUuid,
            Fault::BadPath,
            Fault::BadQuery,
            Fault::BodyNotAllowed,
            Fault::Unauthorized,
            Fault::NotFound,
            Fault::MethodNotAllowed,
            Fault::Timeout,
            Fault::UriTooLong,
            Fault::RangeNotSatisfiable,
            Fault::TooManyRequests,
            Fault::HeadersTooLarge,
            Fault::ArchiveUnavailable,
            Fault::VersionNotSupported,
            Fault::Internal,
        ] {
            assert_ne!(reason(f.status()), "Unknown", "{f:?} -> {}", f.status());
        }
        for ok in [200, 206] {
            assert_ne!(reason(ok), "Unknown");
        }
    }

    #[test]
    fn a_json_body_carries_its_content_type() {
        let r = Response::new(200).json(b"{}".to_vec());
        let head = String::from_utf8(r.head_bytes()).unwrap();
        assert!(
            head.contains("Content-Type: application/json\r\n"),
            "{head}"
        );
        assert_eq!(r.payload.as_slice(), b"{}");
    }

    #[test]
    fn a_subrange_of_a_subrange_stays_inside_the_original() {
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(b"0123456789".to_vec());
        let whole = SharedBytes::new(data, 2, 8); // "234567"
        let mid = whole.subrange(1, 5); // "3456"
        assert_eq!(mid.as_slice(), b"3456");
        // Asking past the end of a narrowed view clamps rather than reaching
        // back into the mapping.
        let past = mid.subrange(2, 999);
        assert_eq!(past.as_slice(), b"56");
        assert!(mid.subrange(9, 9).is_empty());
    }

    #[test]
    fn shared_bytes_subrange_stays_inside() {
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(b"0123456789".to_vec());
        let s = SharedBytes::new(data, 2, 8);
        assert_eq!(s.as_slice(), b"234567");
        assert_eq!(s.subrange(1, 3).as_slice(), b"34");
        assert_eq!(s.subrange(0, 100).as_slice(), b"234567");
        assert_eq!(s.subrange(100, 200).as_slice(), b"");
        assert_eq!(s.subrange(5, 1).as_slice(), b"");
    }

    #[test]
    fn out_of_range_shared_bytes_are_empty_not_a_panic() {
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(b"abc".to_vec());
        let s = SharedBytes::new(data, 10, 20);
        assert_eq!(s.as_slice(), b"");
    }
}
