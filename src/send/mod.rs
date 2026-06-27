//! The send transport abstraction (DESIGN.md §2, §7, §10).

pub mod smtp;

use async_trait::async_trait;

use crate::error::Result;

/// A file attached to an outbound message.
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// A composed outbound message, transport-agnostic.
pub struct OutboundMessage {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub reply_to: Option<String>,
    pub subject: String,
    pub body: String,
    /// Treat `body` as HTML rather than plain text.
    pub html: bool,
    pub attachments: Vec<Attachment>,
}

impl OutboundMessage {
    /// All envelope recipients (to + cc + bcc).
    pub fn recipients(&self) -> Vec<String> {
        self.to
            .iter()
            .chain(&self.cc)
            .chain(&self.bcc)
            .cloned()
            .collect()
    }
}

/// The outcome of a successful send.
pub struct SendReceipt {
    pub message_id: String,
    pub accepted: Vec<String>,
    pub transport: &'static str,
}

/// An outbound mail transport.
#[async_trait]
pub trait Sender {
    async fn send(&self, message: OutboundMessage) -> Result<SendReceipt>;
}
