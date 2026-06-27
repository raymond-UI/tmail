//! Shared HTTP client and `Retry-After` parsing (DESIGN.md §6, §11).
//!
//! All TLS is via rustls — no OpenSSL — to keep the static-binary promise.

use std::time::Duration;

use reqwest::header::HeaderMap;

use crate::error::{AppError, ErrorCode, Result};

/// Build the shared reqwest client with a request timeout.
pub fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("tmail/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AppError::new(ErrorCode::Network, format!("http client: {e}")))
}

/// Parse a `Retry-After` header into milliseconds.
///
/// Supports the delta-seconds form (`Retry-After: 30`). The HTTP-date form is
/// uncommon from JSON APIs and is treated as "unknown" (returns `None`) so the
/// caller falls back to its own backoff rather than mis-parsing.
pub fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(secs.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderValue, RETRY_AFTER};

    fn headers_with(retry_after: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(retry_after).unwrap());
        h
    }

    #[test]
    fn parses_delta_seconds() {
        assert_eq!(parse_retry_after(&headers_with("30")), Some(30_000));
        assert_eq!(parse_retry_after(&headers_with("  5 ")), Some(5_000));
    }

    #[test]
    fn missing_header_is_none() {
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn http_date_is_none() {
        assert_eq!(
            parse_retry_after(&headers_with("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
    }
}
