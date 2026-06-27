//! Message retrieval commands: `read`, `get` (DESIGN.md §4).

use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::app::Ctx;
use crate::cli::{GetArgs, ReadArgs};
use crate::error::{AppError, Result};
use crate::model::Message;
use crate::output::emit_success;
use crate::receive::Receiver;

/// `tmail read` — list messages newest-first; never blocks.
pub async fn run_read(ctx: &Ctx, args: &ReadArgs) -> Result<()> {
    let handle = ctx.resolve_handle(args.target.as_deref())?;
    let mut messages = ctx.receiver()?.read(&handle).await?;

    // Newest-first; unparseable dates (None is the smallest) sort last.
    messages.sort_by_key(|m| std::cmp::Reverse(parse_date(&m.date)));

    let since = match &args.since {
        Some(s) => Some(parse_required_date(s)?),
        None => None,
    };

    let summaries: Vec<_> = messages
        .into_iter()
        .filter(|m| !args.unread || !m.seen)
        .filter(|m| match (since, parse_date(&m.date)) {
            (Some(since), Some(date)) => date >= since,
            // No `--since`, or a message we can't date — keep it.
            _ => true,
        })
        .take(args.limit)
        .map(|m| m.summary())
        .collect();

    emit_success(&summaries, ctx.pretty())
}

/// `tmail get` — full body of one message.
pub async fn run_get(ctx: &Ctx, args: &GetArgs) -> Result<()> {
    let handle = ctx.resolve_handle(args.target.as_deref())?;
    let message: Message = ctx.receiver()?.get(&handle, &args.msg_id).await?;

    // Default emits both renderings; `--text`/`--html` narrow to one.
    let mut value = json!({
        "id": message.id,
        "from": message.from,
        "subject": message.subject,
        "date": message.date,
    });
    if !args.html {
        value["text"] = json!(message.text);
    }
    if !args.text {
        if let Some(html) = &message.html {
            value["html"] = json!(html);
        }
    }
    emit_success(&value, ctx.pretty())
}

/// Parse an ISO-8601 timestamp for sorting/filtering; `None` if unparseable.
fn parse_date(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// Parse a user-supplied `--since`, mapping a bad value to a `CONFIG` error.
fn parse_required_date(s: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|_| AppError::config(format!("--since must be ISO-8601 (RFC-3339): got '{s}'")))
}
