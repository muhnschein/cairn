//! Logging to stderr. systemd captures it; there is no log file to open.
//!
//! Nothing a client sent is ever logged, and no entry content is.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::Level;

static LEVEL: AtomicU8 = AtomicU8::new(2);

fn rank(level: Level) -> u8 {
    match level {
        Level::Error => 0,
        Level::Warn => 1,
        Level::Info => 2,
        Level::Debug => 3,
    }
}

/// Set the verbosity floor.
pub fn set_level(level: Level) {
    LEVEL.store(rank(level), Ordering::Relaxed);
}

/// True when a message at this level would be printed.
pub fn enabled(level: Level) -> bool {
    rank(level) <= LEVEL.load(Ordering::Relaxed)
}

/// Write one line. Called through the macros below.
pub fn emit(level: Level, args: std::fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let tag = match level {
        Level::Error => "error",
        Level::Warn => "warn",
        Level::Info => "info",
        Level::Debug => "debug",
    };
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "cairnd: {tag}: {args}");
}

/// Log an error.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log::emit($crate::config::Level::Error, format_args!($($arg)*)) };
}

/// Log a warning.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log::emit($crate::config::Level::Warn, format_args!($($arg)*)) };
}

/// Log an informational line.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log::emit($crate::config::Level::Info, format_args!($($arg)*)) };
}

/// Log a debug line.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::log::emit($crate::config::Level::Debug, format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The level is process-global, so this restores it rather than leaving
    /// the rest of the suite at whatever it set.
    struct Restore(Level);

    impl Drop for Restore {
        fn drop(&mut self) {
            set_level(self.0);
        }
    }

    #[test]
    fn a_level_admits_itself_and_everything_louder() {
        let _restore = Restore(Level::Info);
        set_level(Level::Warn);
        assert!(enabled(Level::Error));
        assert!(enabled(Level::Warn));
        assert!(!enabled(Level::Info));
        assert!(!enabled(Level::Debug));

        set_level(Level::Debug);
        for l in [Level::Error, Level::Warn, Level::Info, Level::Debug] {
            assert!(enabled(l), "{l:?} should print at debug");
        }

        set_level(Level::Error);
        assert!(enabled(Level::Error));
        assert!(!enabled(Level::Warn), "error is the quietest level");
    }
}
