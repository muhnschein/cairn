//! Binding and accepting. Both socket kinds behave the same above this line.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use crate::config::Listen;

/// A bound listener.
#[derive(Debug)]
pub enum Listener {
    Unix(UnixListener),
    Tcp(TcpListener),
}

impl Listener {
    /// Bind, and for unix sockets set the mode after binding.
    ///
    /// A leftover socket file from a killed daemon is removed only after a
    /// connect proves nothing is listening on it.
    pub fn bind(listen: &Listen, mode: u32) -> io::Result<Listener> {
        match listen {
            Listen::Unix(path) => {
                clear_stale_socket(path)?;
                let l = UnixListener::bind(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
                Ok(Listener::Unix(l))
            }
            Listen::Tcp(addr) => Ok(Listener::Tcp(TcpListener::bind(addr)?)),
        }
    }

    /// Accept one connection.
    pub fn accept(&self) -> io::Result<Stream> {
        match self {
            Listener::Unix(l) => l.accept().map(|(s, _)| Stream::Unix(s)),
            Listener::Tcp(l) => l.accept().map(|(s, _)| Stream::Tcp(s)),
        }
    }
}

fn clear_stale_socket(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("{} is already served by a running daemon", path.display()),
        )),
        Err(_) => std::fs::remove_file(path),
    }
}

/// An accepted connection.
#[derive(Debug)]
pub enum Stream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Stream {
    /// Apply both timeouts. A stalled peer must not hold a worker.
    pub fn set_timeouts(&self, read: Duration, write: Duration) -> io::Result<()> {
        match self {
            Stream::Unix(s) => {
                s.set_read_timeout(Some(read))?;
                s.set_write_timeout(Some(write))
            }
            Stream::Tcp(s) => {
                s.set_read_timeout(Some(read))?;
                s.set_write_timeout(Some(write))
            }
        }
    }

    /// Half-close, ignoring errors from a peer that already left.
    pub fn shutdown(&self) {
        let _ = match self {
            Stream::Unix(s) => s.shutdown(std::net::Shutdown::Both),
            Stream::Tcp(s) => s.shutdown(std::net::Shutdown::Both),
        };
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Unix(s) => s.read(buf),
            Stream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Unix(s) => s.write(buf),
            Stream::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Unix(s) => s.flush(),
            Stream::Tcp(s) => s.flush(),
        }
    }
}
