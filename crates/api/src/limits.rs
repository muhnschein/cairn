//! Bounds on everything a client controls. None of them is optional.

/// Request-side limits. Defaults are the documented ones in `cairn.conf(5)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest request line, method and target included.
    pub max_request_line: usize,
    /// Longest header block.
    pub max_header_bytes: usize,
    /// Most headers in one request.
    pub max_headers: usize,
    /// Longest decoded entry path.
    pub max_path_bytes: usize,
    /// Longest `q` value accepted by `/suggest`.
    pub suggest_max_query: usize,
    /// Most suggestions returned.
    pub suggest_max_results: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_request_line: 8 * 1024,
            max_header_bytes: 16 * 1024,
            max_headers: 64,
            max_path_bytes: 1024,
            suggest_max_query: 128,
            suggest_max_results: 32,
        }
    }
}

/// Token bucket over request counts on one connection.
///
/// A tiny entry inside a large compressed cluster makes repeated decompression
/// cheap for the client and expensive for the daemon; this is the ceiling on
/// that.
#[derive(Debug, Clone, Copy)]
pub struct RateLimiter {
    tokens: f64,
    burst: f64,
    per_second: f64,
    last: std::time::Instant,
}

impl RateLimiter {
    /// A bucket of `burst` requests refilling at `per_second`.
    pub fn new(per_second: f64, burst: f64, now: std::time::Instant) -> RateLimiter {
        RateLimiter { tokens: burst, burst, per_second, last: now }
    }

    /// Take one request's worth of budget, if there is any.
    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        if self.per_second <= 0.0 {
            return true; // disabled
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn burst_then_refill() {
        let t0 = Instant::now();
        let mut r = RateLimiter::new(10.0, 3.0, t0);
        assert!(r.allow(t0));
        assert!(r.allow(t0));
        assert!(r.allow(t0));
        assert!(!r.allow(t0), "burst is spent");
        assert!(r.allow(t0 + Duration::from_millis(150)), "refills over time");
    }

    #[test]
    fn zero_rate_disables() {
        let t0 = Instant::now();
        let mut r = RateLimiter::new(0.0, 0.0, t0);
        for _ in 0..1000 {
            assert!(r.allow(t0));
        }
    }
}
