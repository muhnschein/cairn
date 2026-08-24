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
    /// A bound unix socket.
    Unix(UnixListener),
    /// A bound loopback TCP socket.
    Tcp(TcpListener),
}

impl Listener {
    /// Bind, and for unix sockets set the mode after binding.
    ///
    /// A leftover socket file from a killed daemon is removed only after a
    /// connect proves nothing is listening on it.
    pub fn bind(listen: &Listen, mode: u32) -> io::Result<Listener> {
        Listener::preflight(listen)?;
        match listen {
            Listen::Unix(path) => {
                clear_stale_socket(path)?;
                let l = UnixListener::bind(path)
                    .map_err(|e| context(&format!("cannot listen on {listen}"), &e))?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| context(&format!("cannot set mode on {}", path.display()), &e))?;
                Ok(Listener::Unix(l))
            }
            Listen::Tcp(addr) => TcpListener::bind(addr)
                .map(Listener::Tcp)
                .map_err(|e| context(&format!("cannot listen on {listen}"), &e)),
        }
    }

    /// What can be checked about a listener without binding it.
    ///
    /// `cairnd --check` runs this too: a configuration that opens every archive
    /// and then cannot bind is not an ok configuration.
    pub fn preflight(listen: &Listen) -> io::Result<()> {
        let Listen::Unix(path) = listen else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or(Path::new("."));
        if parent.as_os_str().is_empty() {
            return Ok(());
        }
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "cannot listen on {listen}: the directory {} does not exist \
                     (systemd creates it from RuntimeDirectory=cairn; otherwise \
                     create it, or point `listen` somewhere that exists)",
                    parent.display()
                ),
            ));
        }
        Ok(())
    }

    /// Accept one connection.
    pub fn accept(&self) -> io::Result<Stream> {
        match self {
            Listener::Unix(l) => l.accept().map(|(s, _)| Stream::Unix(s)),
            Listener::Tcp(l) => l.accept().map(|(s, _)| Stream::Tcp(s)),
        }
    }
}

/// Keep the subject of a failure attached to it: a bare ENOENT names nothing.
fn context(what: &str, e: &io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{what}: {e}"))
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
    /// An accepted unix connection.
    Unix(UnixStream),
    /// An accepted TCP connection.
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

    /// Replace the read timeout, leaving the write timeout alone.
    ///
    /// A connection between requests is idle, not slow, and `keepalive_timeout`
    /// governs that wait; `read_timeout` governs a request already in flight.
    pub fn set_read_timeout(&self, read: Duration) -> io::Result<()> {
        match self {
            Stream::Unix(s) => s.set_read_timeout(Some(read)),
            Stream::Tcp(s) => s.set_read_timeout(Some(read)),
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A directory under the target dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!(
                "cairnd-listener-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preflight_names_the_missing_directory() {
        let listen = Listen::Unix(PathBuf::from("/nonexistent-cairn-dir/cairn.sock"));
        let e = Listener::preflight(&listen).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        // A bare ENOENT sends an operator looking at the socket, not the
        // directory that does not exist.
        assert!(e.to_string().contains("/nonexistent-cairn-dir"), "{e}");
        assert!(e.to_string().contains("RuntimeDirectory"), "{e}");
    }

    #[test]
    fn preflight_passes_for_a_tcp_listener_and_an_existing_directory() {
        let dir = Dir::new("preflight");
        let sock = dir.0.join("cairn.sock");
        Listener::preflight(&Listen::Unix(sock)).expect("directory exists");
        let addr = "127.0.0.1:0".parse().expect("literal address");
        Listener::preflight(&Listen::Tcp(addr)).expect("tcp needs no directory");
    }

    #[test]
    fn a_socket_left_by_a_dead_daemon_is_removed() {
        let dir = Dir::new("stale");
        let sock = dir.0.join("cairn.sock");
        // A bound-then-dropped listener leaves the file behind, which is what
        // a killed daemon leaves.
        drop(UnixListener::bind(&sock).expect("bind"));
        assert!(sock.exists());

        let l = Listener::bind(&Listen::Unix(sock.clone()), 0o600).expect("rebind over the stale");
        assert!(matches!(l, Listener::Unix(_)));
        let mode = std::fs::metadata(&sock).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the mode is set after binding");
    }

    #[test]
    fn a_socket_a_live_daemon_holds_is_not_removed() {
        let dir = Dir::new("live");
        let sock = dir.0.join("cairn.sock");
        let held = UnixListener::bind(&sock).expect("bind");

        let e = Listener::bind(&Listen::Unix(sock.clone()), 0o600).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AddrInUse, "{e}");
        assert!(sock.exists(), "a live daemon's socket must survive");
        drop(held);
    }
}
