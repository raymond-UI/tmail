//! The receive transport abstraction (DESIGN.md §2, §10).
//!
//! `wait`/`otp` are built generically on top of [`Receiver::read`] in a later
//! epic, so any backend that can list messages gets them for free.

pub mod mailtm;

use async_trait::async_trait;

use crate::error::Result;
use crate::model::{Handle, InboxRecord, Message};

/// A disposable-inbox receive backend.
#[async_trait]
pub trait Receiver {
    /// Mint a fresh inbox.
    async fn new_inbox(&self) -> Result<InboxRecord>;
    /// List messages, newest-first (summaries only — no body hydration).
    async fn read(&self, handle: &Handle) -> Result<Vec<Message>>;
    /// Fetch one message with its full body.
    async fn get(&self, handle: &Handle, msg_id: &str) -> Result<Message>;
    /// Delete the inbox upstream (best-effort).
    async fn delete(&self, handle: &Handle) -> Result<()>;
}
