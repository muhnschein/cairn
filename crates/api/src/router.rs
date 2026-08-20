//! Routing and handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::catalog::{Catalog, CatalogError, Metadata};
use crate::fault::Fault;
use crate::json::Json;
use crate::limits::Limits;
use crate::percent;
use crate::range::{self, Range};
use crate::request::{Method, Request};
use crate::response::{Payload, Response};
use crate::status::Status;
use crate::token;

/// Policy the daemon hands to the router.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Shared bearer token, or `None` for an open socket.
    pub auth_token: Option<String>,
    /// `Content-Security-Policy` sent with entry content.
    pub content_security_policy: String,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            auth_token: None,
            content_security_policy: "default-src 'none'; sandbox".to_owned(),
        }
    }
}

/// Turns requests into responses. Holds no socket and no clock but its own.
pub struct Router {
    catalog: Arc<dyn Catalog>,
    limits: Limits,
    policy: Policy,
    status: Box<dyn Fn() -> Status + Send + Sync>,
    rng: AtomicU64,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Router {
    /// Build a router. `seed` comes from the operating system, once, at startup.
    pub fn new(
        catalog: Arc<dyn Catalog>,
        limits: Limits,
        policy: Policy,
        status: Box<dyn Fn() -> Status + Send + Sync>,
        seed: u64,
    ) -> Router {
        Router {
            catalog,
            limits,
            policy,
            status,
            rng: AtomicU64::new(seed),
        }
    }

    /// Limits in force.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Answer one request.
    pub fn handle(&self, req: &Request) -> Response {
        let mut response = self.route(req);
        response.keep_alive = req.keep_alive;
        if req.method == Method::Head {
            response.send_body = false;
        }
        response
    }

    fn route(&self, req: &Request) -> Response {
        // Authentication is checked before routing, so an unauthenticated
        // client cannot tell a missing archive from a forbidden one.
        if let Some(expected) = &self.policy.auth_token
            && !self.authenticated(req, expected)
        {
            return Fault::Unauthorized.response();
        }
        if !matches!(req.method, Method::Get | Method::Head) {
            return Fault::MethodNotAllowed.response();
        }

        let Some(rest) = req.path.strip_prefix("/v1/") else {
            return Fault::NotFound.response();
        };
        match rest {
            "status" => return self.status_response(),
            "archives" => return self.archives_response(),
            _ => {}
        }
        let Some(rest) = rest.strip_prefix("archives/") else {
            return Fault::NotFound.response();
        };
        let (uuid, tail) = match rest.split_once('/') {
            Some((u, t)) => (u, Some(t)),
            None => (rest, None),
        };
        if !is_canonical_uuid(uuid) {
            return Fault::BadUuid.response();
        }
        match tail {
            None => self.archive_response(uuid),
            Some("random") => self.random_response(uuid),
            Some("suggest") => self.suggest_response(uuid, req.query.as_deref()),
            Some(t) => match t.strip_prefix("entry/") {
                Some(path) => self.entry_response(uuid, path, req),
                None => Fault::NotFound.response(),
            },
        }
    }

    fn authenticated(&self, req: &Request, expected: &str) -> bool {
        let Some(value) = req.header("authorization") else {
            return false;
        };
        let Some(token) = value.strip_prefix("Bearer ") else {
            return false;
        };
        constant_time_eq(token.as_bytes(), expected.as_bytes())
    }

    fn status_response(&self) -> Response {
        let s = (self.status)();
        let mut j = Json::new();
        j.begin_object();
        j.field("version", &s.version);
        j.field_number("uptime_seconds", s.uptime_seconds);
        j.field("listener", &s.listener);
        j.field_number("archives", s.archive_count);
        j.field("auth", if s.auth_required { "bearer" } else { "none" });

        j.key("sandbox").begin_object();
        j.field_bool("required", s.sandbox.required);
        j.key("layers").begin_array();
        for layer in &s.sandbox.layers {
            j.begin_object();
            j.field("name", &layer.name);
            j.field("state", &layer.state);
            match &layer.detail {
                Some(d) => j.field("detail", d),
                None => j.key("detail").null(),
            };
            j.end_object();
        }
        j.end_array();
        j.end_object();

        j.key("cache").begin_object();
        j.field_number("budget_bytes", s.cache.budget_bytes);
        j.field_number("bytes", s.cache.bytes);
        j.field_number("entries", s.cache.entries);
        j.field_number("hits", s.cache.hits);
        j.field_number("misses", s.cache.misses);
        j.field_number("evictions", s.cache.evictions);
        j.end_object();

        j.key("connections").begin_object();
        j.field_number("max", s.connections.max);
        j.field_number("active", s.connections.active);
        j.field_number("served", s.connections.served);
        j.field_number("rejected", s.connections.rejected);
        j.end_object();

        j.key("limits").begin_object();
        j.field_number("max_request_line", self.limits.max_request_line as u64);
        j.field_number("max_header_bytes", self.limits.max_header_bytes as u64);
        j.field_number("max_headers", self.limits.max_headers as u64);
        j.field_number("max_path_bytes", self.limits.max_path_bytes as u64);
        j.field_number("suggest_max_query", self.limits.suggest_max_query as u64);
        j.field_number(
            "suggest_max_results",
            self.limits.suggest_max_results as u64,
        );
        j.end_object();

        j.end_object();
        ok_json(j)
    }

    fn archives_response(&self) -> Response {
        let mut j = Json::new();
        j.begin_object();
        j.key("archives").begin_array();
        for a in self.catalog.archives() {
            j.begin_object();
            summary_fields(&mut j, &a);
            j.end_object();
        }
        j.end_array();
        j.end_object();
        ok_json(j)
    }

    fn archive_response(&self, uuid: &str) -> Response {
        let summary = match self.catalog.summary(uuid) {
            Ok(s) => s,
            Err(e) => return fault_for(e).response(),
        };
        let metadata = self
            .catalog
            .metadata(uuid)
            .unwrap_or_else(|_| Metadata::default());
        let mut j = Json::new();
        j.begin_object();
        summary_fields(&mut j, &summary);
        j.key("metadata").begin_object();
        for (k, v) in &metadata.text {
            j.field(k, v);
        }
        j.end_object();
        j.key("binary_metadata").begin_array();
        for k in &metadata.binary {
            j.string(k);
        }
        j.end_array();
        j.end_object();
        ok_json(j)
    }

    fn random_response(&self, uuid: &str) -> Response {
        let pick = self.next_random();
        match self.catalog.random(uuid, pick) {
            Ok(path) => {
                let mut j = Json::new();
                j.begin_object();
                j.field("archive", uuid);
                j.field("path", &path);
                j.end_object();
                ok_json(j)
            }
            Err(e) => fault_for(e).response(),
        }
    }

    fn suggest_response(&self, uuid: &str, query: Option<&str>) -> Response {
        let mut prefix: Option<String> = None;
        let mut limit = self.limits.suggest_max_results;
        for (key, value) in query_pairs(query.unwrap_or("")) {
            match key.as_str() {
                "q" => {
                    let Ok(v) = percent::decode(&value) else {
                        return Fault::BadQuery.response();
                    };
                    if v.len() > self.limits.suggest_max_query {
                        return Fault::BadQuery.response();
                    }
                    prefix = Some(v);
                }
                "limit" => {
                    let Ok(n) = value.parse::<usize>() else {
                        return Fault::BadQuery.response();
                    };
                    limit = n.min(self.limits.suggest_max_results);
                }
                _ => {}
            }
        }
        let Some(prefix) = prefix else {
            return Fault::BadQuery.response();
        };
        match self.catalog.suggest(uuid, &prefix, limit) {
            Ok(hits) => {
                let mut j = Json::new();
                j.begin_object();
                j.field("archive", uuid);
                j.key("suggestions").begin_array();
                for s in hits {
                    j.begin_object();
                    j.field("title", &s.title);
                    j.field("path", &s.path);
                    j.end_object();
                }
                j.end_array();
                j.end_object();
                ok_json(j)
            }
            Err(e) => fault_for(e).response(),
        }
    }

    fn entry_response(&self, uuid: &str, raw_path: &str, req: &Request) -> Response {
        if raw_path.is_empty() {
            return Fault::NotFound.response();
        }
        let Ok(path) = percent::decode(raw_path) else {
            return Fault::BadPath.response();
        };
        if path.len() > self.limits.max_path_bytes {
            return Fault::UriTooLong.response();
        }
        let entry = match self.catalog.entry(uuid, &path) {
            Ok(e) => e,
            Err(e) => return fault_for(e).response(),
        };

        let len = entry.body.len() as u64;
        let requested = req
            .header("range")
            .map(|v| range::parse(v, len))
            .unwrap_or(Range::Whole);
        let mut response = match requested {
            Range::Unsatisfiable => Fault::RangeNotSatisfiable
                .response()
                .header("Content-Range", format!("bytes */{len}")),
            Range::Whole => Response::new(200).body(Payload::Shared(entry.body.clone())),
            Range::Partial { start, end } => Response::new(206)
                .header("Content-Range", format!("bytes {start}-{}/{len}", end - 1))
                .body(Payload::Shared(entry.body.subrange(start, end))),
        };

        if response.status != 416 {
            response = response.header("Content-Type", token::content_type(&entry.mime));
        }
        response
            .header("Accept-Ranges", "bytes")
            .header("X-Cairn-Archive", uuid)
            .header("X-Cairn-Path", percent::encode_header_value(&entry.path))
            .header("X-Content-Type-Options", "nosniff")
            .header("Cross-Origin-Resource-Policy", "same-origin")
            .header(
                "Content-Security-Policy",
                self.policy.content_security_policy.clone(),
            )
    }

    fn next_random(&self) -> u64 {
        // SplitMix64: enough for choosing an entry, and no dependency.
        let mut z = self.rng.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn summary_fields(j: &mut Json, a: &crate::catalog::ArchiveSummary) {
    j.field("uuid", &a.uuid);
    j.field("title", &a.title);
    j.field_number("entry_count", a.entry_count);
    j.field_number("cluster_count", a.cluster_count);
    match &a.main_page {
        Some(p) => j.field("main_page", p),
        None => j.key("main_page").null(),
    };
    j.field(
        "format_version",
        &format!("{}.{}", a.major_version, a.minor_version),
    );
    j.field("content_namespace", &a.content_namespace.to_string());
}

fn ok_json(j: Json) -> Response {
    Response::new(200)
        .header("X-Content-Type-Options", "nosniff")
        .json(j.into_bytes())
}

fn fault_for(e: CatalogError) -> Fault {
    match e {
        // A missing archive and a missing entry are the same answer: a client
        // learns nothing from the difference, and there is nothing to gain.
        CatalogError::NoSuchArchive | CatalogError::NoSuchEntry => Fault::NotFound,
        CatalogError::Corrupt => Fault::ArchiveUnavailable,
    }
}

/// Split `a=b&c=d`. Keys are compared raw; values are decoded by the caller.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => (p.to_owned(), String::new()),
        })
        .collect()
}

/// True only for the canonical lowercase hyphenated form.
pub fn is_canonical_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    b.iter().enumerate().all(|(i, &c)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            c == b'-'
        } else {
            c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
        }
    })
}

/// Compare without an early exit. Length mismatch is reported, not timed.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_uuid_only() {
        assert!(is_canonical_uuid("01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!is_canonical_uuid("01234567-89AB-CDEF-0123-456789ABCDEF"));
        assert!(!is_canonical_uuid("0123456789abcdef0123456789abcdef"));
        assert!(!is_canonical_uuid("01234567-89ab-cdef-0123-456789abcde"));
        assert!(!is_canonical_uuid("01234567_89ab_cdef_0123_456789abcdef"));
        assert!(!is_canonical_uuid(""));
        assert!(!is_canonical_uuid("01234567-89ab-cdef-0123-456789abcdeg"));
    }

    #[test]
    fn constant_time_eq_is_correct() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn query_pairs_are_split_not_decoded() {
        assert_eq!(
            query_pairs("q=a%20b&limit=5&flag"),
            vec![
                ("q".into(), "a%20b".into()),
                ("limit".into(), "5".into()),
                ("flag".into(), String::new()),
            ]
        );
        assert_eq!(query_pairs(""), Vec::new());
    }
}
