//! Optional real-time delivery via the mail.tm Mercure hub (feature `sse`).
//!
//! Polling (`wait`/`otp`) remains the always-available default; this is the
//! seam for the SSE optimization to be completed once the poll path is proven
//! (DESIGN.md §6, §14). It is intentionally not yet wired into the poll loop.

use reqwest_eventsource::EventSource;

use crate::error::{AppError, ErrorCode, Result};

/// Open a bearer-authenticated event stream against a Mercure topic URL.
pub fn open(client: &reqwest::Client, url: &str, token: &str) -> Result<EventSource> {
    let request = client.get(url).bearer_auth(token);
    EventSource::new(request)
        .map_err(|e| AppError::new(ErrorCode::Network, format!("sse: cannot open stream: {e}")))
}
