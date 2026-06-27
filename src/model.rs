//! Provider-agnostic data model (DESIGN.md §5).
//!
//! [`InboxRecord`] is what we persist; [`InboxView`] is the secret-free
//! projection that `ls`/`new` print. [`Message`] is the normalized message used
//! internally, with `read`/`get` projecting their own output shapes.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, ErrorCode, Result};

/// Provider secrets for one inbox. Never appears in `ls`/`new` output unless
/// explicitly requested via `--stateless` (as an opaque base64 blob).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handle {
    pub account_id: String,
    pub address: String,
    pub password: String,
    pub token: String,
}

impl Handle {
    /// Encode the handle as the opaque base64 blob carried by `--stateless` /
    /// `--handle` / `TMAIL_HANDLE`.
    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_vec(self)?;
        Ok(STANDARD.encode(json))
    }

    /// Decode a base64 handle blob produced by [`Handle::encode`].
    pub fn decode(blob: &str) -> Result<Handle> {
        let bytes = STANDARD
            .decode(blob.trim())
            .map_err(|e| AppError::new(ErrorCode::Config, format!("invalid handle blob: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::new(ErrorCode::Config, format!("invalid handle blob: {e}")))
    }
}

/// A stored inbox, including its secret [`Handle`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxRecord {
    /// Our short id.
    pub id: String,
    pub address: String,
    pub provider: String,
    pub handle: Handle,
    /// ISO-8601 creation time.
    pub created_at: String,
}

impl InboxRecord {
    /// Project to the secret-free view for output.
    pub fn view(&self) -> InboxView {
        InboxView {
            id: self.id.clone(),
            address: self.address.clone(),
            provider: self.provider.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

/// The secret-free projection printed by `ls` and `new`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxView {
    pub id: String,
    pub address: String,
    pub provider: String,
    pub created_at: String,
}

/// A normalized, provider-agnostic message.
#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub intro: String,
    /// Best-effort plain rendering (empty for list summaries).
    pub text: String,
    pub html: Option<String>,
    pub date: String,
    pub seen: bool,
}

impl Message {
    /// The shape `read` prints (newest-first list items).
    pub fn summary(&self) -> MessageSummary {
        MessageSummary {
            id: self.id.clone(),
            from: self.from.clone(),
            subject: self.subject.clone(),
            intro: self.intro.clone(),
            date: self.date.clone(),
            seen: self.seen,
        }
    }

    /// The shape `get`/`wait` print (full body).
    pub fn full(&self) -> MessageFull {
        MessageFull {
            id: self.id.clone(),
            from: self.from.clone(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            html: self.html.clone(),
            date: self.date.clone(),
        }
    }
}

/// Output projection for `read`.
#[derive(Debug, Clone, Serialize)]
pub struct MessageSummary {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub intro: String,
    pub date: String,
    pub seen: bool,
}

/// Output projection for `get` / `wait`.
#[derive(Debug, Clone, Serialize)]
pub struct MessageFull {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    pub date: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> Handle {
        Handle {
            account_id: "acc_1".into(),
            address: "a@b.com".into(),
            password: "pw".into(),
            token: "tok".into(),
        }
    }

    #[test]
    fn handle_round_trips_through_base64() {
        let h = handle();
        let blob = h.encode().unwrap();
        let back = Handle::decode(&blob).unwrap();
        assert_eq!(back.account_id, h.account_id);
        assert_eq!(back.token, h.token);
    }

    #[test]
    fn bad_handle_blob_is_config_error() {
        let err = Handle::decode("!!!not-base64!!!").unwrap_err();
        assert_eq!(err.code, ErrorCode::Config);
    }

    #[test]
    fn view_omits_handle() {
        let rec = InboxRecord {
            id: "a1b2c3".into(),
            address: "a@b.com".into(),
            provider: "mail.tm".into(),
            handle: handle(),
            created_at: "2026-06-27T18:40:00Z".into(),
        };
        let json = serde_json::to_string(&rec.view()).unwrap();
        assert!(!json.contains("password"));
        assert!(!json.contains("token"));
        assert!(json.contains("createdAt"));
    }
}
