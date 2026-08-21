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
use crate::{debug, error, info, warn};

/// Counters published by `/v1/status`.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Connections being served right now.
    pub active: AtomicU64,
    /// Connections accepted since startup.
    pub served: AtomicU64,
    /// Connections refused since startup.
    pub rejected: AtomicU64,
}

/// Everything a worker needs once confinement is done.
#[derive(Debug)]
pub struct Serving {
    /// The routing table, holding the open archives.
    pub router: Arc<Router>,
    /// Limits and timeouts, read on every request.
    pub config: Arc<Config>,
    /// Shared counters.
    pub metrics: Arc<Metrics>,
}

/// Holds workers between `spawn` and the end of confinement.
///
/// Two rendezvous, in this order: every worker reports that it has finished
/// starting up, then the main thread confines the process and opens the gate.
/// Confinement must not land in the middle of thread startup — a worker still
/// naming itself would take the filter's default action — and a request must
/// never be answered by an unconfined process.
#[derive(Debug)]
pub struct Gate {
    state: Mutex<State>,
    ready: Condvar,
    started: Condvar,
}

#[derive(Debug, Default)]
struct State {
    serving: Option<Arc<Serving>>,
    workers: usize,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    /// A closed gate with no workers yet.
    pub fn new() -> Gate {
        Gate {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
            started: Condvar::new(),
        }
    }

    /// Block until `count` workers have finished starting.
    pub fn wait_for_workers(&self, count: usize) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while guard.workers < count {
            guard = self
                .started
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Let the workers through.
    pub fn open(&self, serving: Arc<Serving>) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.serving = Some(serving);
        self.ready.notify_all();
    }

    fn arrive(&self) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.workers += 1;
        self.started.notify_all();
    }

    fn wait(&self) -> Arc<Serving> {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(s) = guard.serving.as_ref() {
                return Arc::clone(s);
            }
            guard = self
                .ready
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// Start the worker pool. Call before applying the sandbox.
pub fn spawn_workers(
    listener: &Arc<Listener>,
    gate: &Arc<Gate>,
    count: usize,
) -> std::io::Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::with_capacity(count);
    for n in 0..count {
        let listener = Arc::clone(listener);
        let gate = Arc::clone(gate);
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
    // Touch the allocator before reporting readiness: the first allocation in
    // a thread can initialize a malloc arena, which is startup work, not
    // serving work.
    drop(Vec::<u8>::with_capacity(8 * 1024));
    gate.arrive();
    let serving = gate.wait();
    loop {
        match listener.accept() {
            Ok(stream) => {
                serving.metrics.active.fetch_add(1, Ordering::Relaxed);
                // A panic while serving costs the connection, never the
                // worker: a pool that shrinks on hostile input is a denial of
                // service, and the pool cannot be refilled after confinement.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    serve_connection(&serving, stream);
                }));
                if outcome.is_err() {
                    error!("panic while serving a connection");
                }
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
    if stream
        .set_timeouts(config.read_timeout, config.write_timeout)
        .is_err()
    {
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
    let body = if response.send_body {
        response.payload.as_slice()
    } else {
        &[][..]
    };
    // Small bodies ride along with the head so a response is one write.
    if body.len() <= 8 * 1024 {
        out.extend_from_slice(body);
        return stream.write_all(&out).and_then(|()| stream.flush()).is_ok();
    }
    stream.write_all(&out).is_ok() && stream.write_all(body).is_ok() && stream.flush().is_ok()
}
