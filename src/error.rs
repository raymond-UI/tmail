//! The typed error model and exit-code mapping (DESIGN.md §8).
//!
//! Every failure carries a machine-branchable [`ErrorCode`]; the `message` is
//! for humans only. The process exit code is derived from the code, so an agent
//! can branch without parsing prose.

use std::fmt;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AppError>;

/// Stable, machine-branchable error classes. The integer values are the process
/// exit codes and are part of the public contract — do not renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Unexpected / unclassified failure.
    Generic,
    /// Inbox or message id unknown.
    NotFound,
    /// Provider returned 429; `retry_after_ms` is set when known.
    RateLimited,
    /// No inbox could be minted (v1: mail.tm unreachable after retries).
    AllProvidersDown,
    /// Bad or missing SMTP / provider credentials, or a rejected `from`.
    Auth,
    /// `wait`/`otp` deadline passed with no matching message.
    Timeout,
    /// Malformed config or a missing required setting.
    Config,
    /// Connection / DNS / TLS failure.
    Network,
    /// `otp`: a message matched but no code could be extracted.
    NoMatch,
}

impl ErrorCode {
    /// The process exit code for this class (DESIGN.md §8).
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Generic => 1,
            ErrorCode::NotFound => 2,
            ErrorCode::RateLimited => 3,
            ErrorCode::AllProvidersDown => 4,
            ErrorCode::Auth => 5,
            ErrorCode::Timeout => 6,
            ErrorCode::Config => 7,
            ErrorCode::Network => 8,
            ErrorCode::NoMatch => 9,
        }
    }

    /// The stable string identifier emitted in the JSON `error.code` field.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Generic => "GENERIC",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::RateLimited => "RATE_LIMITED",
            ErrorCode::AllProvidersDown => "ALL_PROVIDERS_DOWN",
            ErrorCode::Auth => "AUTH",
            ErrorCode::Timeout => "TIMEOUT",
            ErrorCode::Config => "CONFIG",
            ErrorCode::Network => "NETWORK",
            ErrorCode::NoMatch => "NO_MATCH",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The single error type returned by every fallible operation in the crate.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    /// Machine-branchable class.
    pub code: ErrorCode,
    /// Human-readable explanation (never parsed by agents).
    pub message: String,
    /// Suggested backoff in milliseconds, when the provider supplied one.
    pub retry_after_ms: Option<u64>,
}

impl AppError {
    /// Construct an error with the given code and message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        AppError {
            code,
            message: message.into(),
            retry_after_ms: None,
        }
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }

    // Common constructors — keep call sites terse and intention-revealing.

    /// A `NOT_FOUND` error.
    pub fn not_found(message: impl Into<String>) -> Self {
        AppError::new(ErrorCode::NotFound, message)
    }

    /// A `CONFIG` error.
    pub fn config(message: impl Into<String>) -> Self {
        AppError::new(ErrorCode::Config, message)
    }

    /// An `AUTH` error.
    pub fn auth(message: impl Into<String>) -> Self {
        AppError::new(ErrorCode::Auth, message)
    }

    /// A `TIMEOUT` error.
    pub fn timeout(message: impl Into<String>) -> Self {
        AppError::new(ErrorCode::Timeout, message)
    }

    /// A `RATE_LIMITED` error with an optional backoff hint.
    pub fn rate_limited(message: impl Into<String>, retry_after_ms: Option<u64>) -> Self {
        AppError {
            code: ErrorCode::RateLimited,
            message: message.into(),
            retry_after_ms,
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest surfaces transport, TLS, DNS, and timeout failures here.
        let code = if e.is_timeout() || e.is_connect() {
            ErrorCode::Network
        } else if e.is_decode() {
            ErrorCode::Generic
        } else {
            ErrorCode::Network
        };
        AppError::new(code, format!("http: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new(ErrorCode::Generic, format!("json: {e}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new(ErrorCode::Generic, format!("io: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_design_table() {
        assert_eq!(ErrorCode::Generic.exit_code(), 1);
        assert_eq!(ErrorCode::NotFound.exit_code(), 2);
        assert_eq!(ErrorCode::RateLimited.exit_code(), 3);
        assert_eq!(ErrorCode::AllProvidersDown.exit_code(), 4);
        assert_eq!(ErrorCode::Auth.exit_code(), 5);
        assert_eq!(ErrorCode::Timeout.exit_code(), 6);
        assert_eq!(ErrorCode::Config.exit_code(), 7);
        assert_eq!(ErrorCode::Network.exit_code(), 8);
        assert_eq!(ErrorCode::NoMatch.exit_code(), 9);
    }

    #[test]
    fn code_strings_are_stable() {
        assert_eq!(ErrorCode::RateLimited.as_str(), "RATE_LIMITED");
        assert_eq!(ErrorCode::NoMatch.as_str(), "NO_MATCH");
    }

    #[test]
    fn retry_after_is_carried() {
        let e = AppError::rate_limited("cooling down", Some(30_000));
        assert_eq!(e.code, ErrorCode::RateLimited);
        assert_eq!(e.retry_after_ms, Some(30_000));
        assert_eq!(e.exit_code(), 3);
    }
}
