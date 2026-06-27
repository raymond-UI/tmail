//! The agent-first I/O contract (DESIGN.md §3, §8).
//!
//! Exactly one JSON value is written to **stdout** — on success *or* error.
//! Diagnostics go to **stderr**. Nothing else ever touches stdout.

use serde::Serialize;
use serde_json::json;

use crate::error::AppError;

/// Write one JSON value to stdout. Compact by default; `--pretty` enables
/// human-friendly indentation.
pub fn emit_success<T: Serialize>(value: &T, pretty: bool) -> crate::error::Result<()> {
    let s = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    println!("{s}");
    Ok(())
}

/// Write the error as the single JSON value on stdout *and* a human line on
/// stderr. Shape: `{ "error": { "code", "message", "retryAfterMs"? } }`.
pub fn emit_error(err: &AppError, pretty: bool) {
    let mut obj = json!({
        "error": {
            "code": err.code.as_str(),
            "message": err.message,
        }
    });
    if let Some(ms) = err.retry_after_ms {
        obj["error"]["retryAfterMs"] = json!(ms);
    }
    let s = if pretty {
        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| obj.to_string())
    } else {
        obj.to_string()
    };
    println!("{s}");
    eprintln!("error[{}]: {}", err.code, err.message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn error_json_includes_retry_after_when_present() {
        let err = AppError::rate_limited("cooling down", Some(30_000));
        let obj = json!({
            "error": { "code": err.code.as_str(), "message": err.message, "retryAfterMs": 30_000u64 }
        });
        assert_eq!(obj["error"]["code"], "RATE_LIMITED");
        assert_eq!(obj["error"]["retryAfterMs"], 30_000);
    }

    #[test]
    fn error_code_string_round_trips() {
        assert_eq!(ErrorCode::Auth.as_str(), "AUTH");
    }
}
