//! Verbosity-gated diagnostics to stderr (DESIGN.md §3).
//!
//! `stdout` is reserved for the single JSON value; all human/diagnostic output
//! goes to stderr and only when `-v`/`-vv` raise the level. Call sites must
//! never pass secrets (tokens, passwords, SMTP credentials) into these messages.

use std::sync::atomic::{AtomicU8, Ordering};

static LEVEL: AtomicU8 = AtomicU8::new(0);

/// Set the global verbosity from the parsed `-v` count (called once at startup).
pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

/// Whether diagnostics at `min` verbosity should be emitted.
pub fn enabled(min: u8) -> bool {
    LEVEL.load(Ordering::Relaxed) >= min
}

/// Emit a diagnostic line to stderr when the level is high enough. The message
/// is built lazily so disabled diagnostics cost nothing on the hot path.
pub fn log(min: u8, msg: impl FnOnce() -> String) {
    if enabled(min) {
        eprintln!("tmail[v]: {}", msg());
    }
}
