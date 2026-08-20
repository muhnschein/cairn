//! The one error shape.
//!
//! Messages are fixed strings. Nothing a client sent is ever echoed back.

use crate::json::Json;
use crate::response::Response;

/// Every way a request can be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    BadRequest,
    BadUuid,
    BadPath,
    BadQuery,
    BodyNotAllowed,
    Unauthorized,
    NotFound,
    MethodNotAllowed,
    Timeout,
    UriTooLong,
    RangeNotSatisfiable,
    TooManyRequests,
    HeadersTooLarge,
    ArchiveUnavailable,
    VersionNotSupported,
    Internal,
}

impl Fault {
    /// HTTP status for this fault.
    pub fn status(self) -> u16 {
        match self {
            Fault::BadRequest
            | Fault::BadUuid
            | Fault::BadPath
            | Fault::BadQuery
            | Fault::BodyNotAllowed => 400,
            Fault::Unauthorized => 401,
            Fault::NotFound => 404,
            Fault::MethodNotAllowed => 405,
            Fault::Timeout => 408,
            Fault::UriTooLong => 414,
            Fault::RangeNotSatisfiable => 416,
            Fault::TooManyRequests => 429,
            Fault::HeadersTooLarge => 431,
            Fault::ArchiveUnavailable => 503,
            Fault::VersionNotSupported => 505,
            Fault::Internal => 500,
        }
    }

    /// Stable machine-readable code, documented in `cairn-api(7)`.
    pub fn code(self) -> &'static str {
        match self {
            Fault::BadRequest => "bad_request",
            Fault::BadUuid => "bad_uuid",
            Fault::BadPath => "bad_path",
            Fault::BadQuery => "bad_query",
            Fault::BodyNotAllowed => "body_not_allowed",
            Fault::Unauthorized => "unauthorized",
            Fault::NotFound => "not_found",
            Fault::MethodNotAllowed => "method_not_allowed",
            Fault::Timeout => "request_timeout",
            Fault::UriTooLong => "uri_too_long",
            Fault::RangeNotSatisfiable => "range_not_satisfiable",
            Fault::TooManyRequests => "too_many_requests",
            Fault::HeadersTooLarge => "headers_too_large",
            Fault::ArchiveUnavailable => "archive_unavailable",
            Fault::VersionNotSupported => "version_not_supported",
            Fault::Internal => "internal",
        }
    }

    /// Fixed message. Never derived from the request.
    pub fn message(self) -> &'static str {
        match self {
            Fault::BadRequest => "malformed request",
            Fault::BadUuid => "archive id must be a canonical lowercase uuid",
            Fault::BadPath => "entry path is not acceptable",
            Fault::BadQuery => "query parameters are not acceptable",
            Fault::BodyNotAllowed => "no endpoint accepts a request body",
            Fault::Unauthorized => "authentication required",
            Fault::NotFound => "no such resource",
            Fault::MethodNotAllowed => "method not allowed",
            Fault::Timeout => "request timed out",
            Fault::UriTooLong => "request target too long",
            Fault::RangeNotSatisfiable => "range cannot be satisfied",
            Fault::TooManyRequests => "request rate exceeded",
            Fault::HeadersTooLarge => "request headers too large",
            Fault::ArchiveUnavailable => "archive region is malformed",
            Fault::VersionNotSupported => "only HTTP/1.1 is supported",
            Fault::Internal => "internal error",
        }
    }

    /// The JSON error document.
    pub fn body(self) -> Vec<u8> {
        let mut j = Json::new();
        j.begin_object();
        j.key("error").begin_object();
        j.field("code", self.code());
        j.field("message", self.message());
        j.end_object();
        j.end_object();
        j.into_bytes()
    }

    /// A complete response, with the headers every cairn response carries.
    pub fn response(self) -> Response {
        let mut r = Response::new(self.status())
            .header("X-Content-Type-Options", "nosniff")
            .json(self.body());
        if self == Fault::MethodNotAllowed {
            r = r.header("Allow", "GET, HEAD");
        }
        if self == Fault::Unauthorized {
            r = r.header("WWW-Authenticate", "Bearer");
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fault_has_a_documented_shape() {
        let all = [
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
        ];
        for f in all {
            let body = String::from_utf8(f.body()).unwrap();
            assert!(body.starts_with(r#"{"error":{"code":"#), "{body}");
            assert!(body.contains(f.code()));
            assert!(f.status() >= 400);
        }
    }
}
