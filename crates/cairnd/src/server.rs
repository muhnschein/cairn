//! The serving loop: a fixed worker pool over one listener.
//!
//! Workers are created before confinement and never after, so the seccomp
//! allowlist does not need thread creation. The pool size is the connection
//! ceiling; anything beyond it waits in the kernel's accept queue.

use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use api::{Fault, Limits, ParseError, RateLimiter, Request, Response, Router};

use crate::config::Config;
use crate::listener::{Listener, Stream};
use crate::{debug, info, warn};

/// Counters published by `/v1/status`.
#[derive(Debug, Default)]
pub struct Metrics {
    pub active: AtomicU64,
    pub served: AtomicU64,
    pub rejected: AtomicU64,
}

/// Everything a worker needs once confinement is done.
#[derive(Debug)]
pub struct Serving {
    pub router: Arc<Router>,
    pub config: Arc<Config>,
    pub metrics: Arc<Metrics>,
}

/// Holds workers between `spawn` and the end of confinement.
///
/// A request must never be answered by an unconfined process, so workers wait
/// here until the sandbox report exists.
#[derive(Debug)]
pub struct Gate {
    state: Mutex<Option<Arc<Serving>>>,
    ready: Condvar,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    /// A closed gate.
    pub fn new() -> Gate {
        Gate { state: Mutex::new(None), ready: Condvar::new() }
    }

    /// Let the workers through.
    pub fn open(&self, serving: Arc<Serving>) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(serving);
        self.ready.notify_all();
    }

    fn wait(&self) -> Arc<Serving> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(s) = guard.as_ref() {
                return Arc::clone(s);
            }
            guard = self.ready.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Start the worker pool. Call before applying the sandbox.
pub fn spawn_workers(
    listener: Arc<Listener>,
    gate: Arc<Gate>,
    count: usize,
) -> std::io::Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::with_capacity(count);
    for n in 0..count {
        let listener = Arc::clone(&listener);
        let gate = Arc::clone(&gate);
        handles.push(
            std::thread::Builder::new()
                .name(format!("cairnd-{n}"))
                .stack_size(1024 * 1024)
                .spawn(move || worker(&listener, &gate))?,
        );
    }
    Ok(handles)
}

fn worker(listener: &Listener, gate: &Gate) {
    let serving = gate.wait();
    loop {
        match listener.accept() {
            Ok(stream) => {
                serving.metrics.active.fetch_add(1, Ordering::Relaxed);
                serve_connection(&serving, stream);
                serving.metrics.active.fetch_sub(1, Ordering::Relaxed);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                // Out of descriptors, mostly. Backing off beats spinning.
                warn!("accept failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Serve one connection until the client leaves or a bound is crossed.
fn serve_connection(serving: &Serving, mut stream: Stream) {
    let config = &serving.config;
    let limits = *serving.router.limits();
    if stream.set_timeouts(config.read_timeout, config.write_timeout).is_err() {
        return;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut scanned = 0usize;
    let mut requests = 0u32;
    let mut rate = RateLimiter::new(config.request_rate, config.request_burst, Instant::now());

    loop {
        let request = match read_request(&mut stream, &mut buf, &mut scanned, &limits, requests) {
            Ok(Some((request, consumed))) => {
                buf.drain(..consumed);
                scanned = 0;
                request
            }
            Ok(None) => return,
            Err(fault) => {
                let mut response = fault.response();
                response.keep_alive = false;
                write_response(&mut stream, &response);
                stream.shutdown();
                return;
            }
        };

        let started = Instant::now();
        requests += 1;
        let mut response = if rate.allow(started) {
            serving.router.handle(&request)
        } else {
            serving.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            let mut r = Fault::TooManyRequests.response();
            r.keep_alive = false;
            r
        };
        if requests >= config.keepalive_requests {
            response.keep_alive = false;
        }
        let keep_alive = response.keep_alive;

        let wrote = write_response(&mut stream, &response);
        serving.metrics.served.fetch_add(1, Ordering::Relaxed);
        if config.access_log {
            // Method, outcome and size only: the target is client input.
            info!(
                "{} {} {} bytes {}us",
                request.method.as_str(),
                response.status,
                response.payload.len(),
                started.elapsed().as_micros()
            );
        }
        if !wrote || !keep_alive {
            stream.shutdown();
            return;
        }
    }
}

/// Read until one request parses. `Ok(None)` means the peer went away.
fn read_request(
    stream: &mut Stream,
    buf: &mut Vec<u8>,
    scanned: &mut usize,
    limits: &Limits,
    requests: u32,
) -> Result<Option<(Request, usize)>, Fault> {
    loop {
        match Request::parse_hinted(buf, limits, *scanned) {
            Ok(parsed) => return Ok(Some(parsed)),
            Err(ParseError::Incomplete) => {}
            Err(e) => return Err(api::fault_for_parse_error(e)),
        }
        *scanned = buf.len();
        if buf.len() > limits.max_request_line + limits.max_header_bytes {
            return Err(Fault::HeadersTooLarge);
        }

        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // An idle keep-alive connection is not a failed request.
                if buf.is_empty() && requests > 0 {
                    return Ok(None);
                }
                return Err(Fault::Timeout);
            }
            Err(e) => {
                debug!("read failed: {e}");
                return Ok(None);
            }
        }
    }
}

/// Write a response. Returns false if the peer stopped reading.
fn write_response(stream: &mut Stream, response: &Response) -> bool {
    let mut out = response.head_bytes();
    let body = if response.send_body { response.payload.as_slice() } else { &[][..] };
    // Small bodies ride along with the head so a response is one write.
    if body.len() <= 8 * 1024 {
        out.extend_from_slice(body);
        return stream.write_all(&out).and_then(|()| stream.flush()).is_ok();
    }
    stream.write_all(&out).is_ok()
        && stream.write_all(body).is_ok()
        && stream.flush().is_ok()
}
