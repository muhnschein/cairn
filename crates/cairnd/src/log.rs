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
