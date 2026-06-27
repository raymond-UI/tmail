//! Small shared helpers: time formatting and random identifier generation.

use rand::Rng;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::error::{AppError, Result};

const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const ALNUM_MIXED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Current UTC time as an ISO-8601 / RFC-3339 string.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// Random lowercase-alphanumeric string of `n` characters.
fn random_alnum(n: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| ALNUM[rng.gen_range(0..ALNUM.len())] as char)
        .collect()
}

/// A mail.tm local-part: must start with a letter, then alphanumerics
/// (DESIGN.md §6).
pub fn gen_local_part() -> String {
    let mut rng = rand::thread_rng();
    let first = (b'a' + rng.gen_range(0..26)) as char;
    format!("{first}{}", random_alnum(9))
}

/// A strong-enough account password (mixed-case alphanumeric, no symbols to
/// avoid URL/encoding surprises).
pub fn gen_password() -> String {
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| ALNUM_MIXED[rng.gen_range(0..ALNUM_MIXED.len())] as char)
        .collect()
}

/// Our short, user-facing inbox id (DESIGN.md §4 shows e.g. `a1b2c3`).
pub fn gen_short_id() -> String {
    random_alnum(6)
}

/// Parse an ISO-8601 / RFC-3339 timestamp; `None` if unparseable.
pub fn parse_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// Parse a user-supplied timestamp, mapping a bad value to a `CONFIG` error.
pub fn require_rfc3339(flag: &str, s: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|_| AppError::config(format!("{flag} must be ISO-8601 (RFC-3339): got '{s}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_part_starts_with_letter() {
        for _ in 0..50 {
            let lp = gen_local_part();
            assert!(lp.chars().next().unwrap().is_ascii_alphabetic());
            assert!(lp.chars().all(|c| c.is_ascii_alphanumeric()));
            assert_eq!(lp.len(), 10);
        }
    }

    #[test]
    fn short_id_is_six_alnum() {
        let id = gen_short_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn now_is_rfc3339() {
        let s = now_rfc3339();
        // Round-trips through the RFC-3339 parser.
        assert!(OffsetDateTime::parse(&s, &Rfc3339).is_ok());
    }
}
