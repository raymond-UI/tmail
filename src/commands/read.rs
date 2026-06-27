//! Message retrieval commands: `read`, `get` (DESIGN.md §4).

use serde_json::json;

use crate::app::Ctx;
use crate::cli::{GetArgs, ReadArgs};
use crate::error::{AppError, Result};
use crate::model::Message;
use crate::output::emit_success;
use crate::receive::Receiver;
use crate::util::{parse_rfc3339, require_rfc3339};

/// `tmail read` — list messages newest-first; never blocks.
pub async fn run_read(ctx: &Ctx, args: &ReadArgs) -> Result<()> {
    // Validate user input before any network work (fail fast on bad --since).
    let since = match &args.since {
        Some(s) => Some(require_rfc3339("--since", s)?),
        None => None,
    };

    let handle = ctx.resolve_handle(args.target.as_deref())?;
    let mut messages = ctx.receiver()?.read(&handle).await?;

    // Newest-first; unparseable dates (None is the smallest) sort last.
    messages.sort_by_key(|m| std::cmp::Reverse(parse_rfc3339(&m.date)));

    let summaries: Vec<_> = messages
        .into_iter()
        .filter(|m| !args.unread || !m.seen)
        .filter(|m| match (since, parse_rfc3339(&m.date)) {
            (Some(since), Some(date)) => date >= since,
            // No `--since`, or a message we can't date — keep it.
            _ => true,
        })
        .take(args.limit as usize)
        .map(|m| m.summary())
        .collect();

    emit_success(&summaries, ctx.pretty())
}

/// `tmail get` — full body of one message.
pub async fn run_get(ctx: &Ctx, args: &GetArgs) -> Result<()> {
    // Accept `<target> <msgId>` or a single `<msgId>` (inbox via --handle / sole
    // inbox). With one positional, clap binds it to `target`, so shift it.
    let (target, msg_id) = match (&args.target, &args.msg_id) {
        (target, Some(msg_id)) => (target.as_deref(), msg_id.as_str()),
        (Some(msg_id), None) => (None, msg_id.as_str()),
        (None, None) => return Err(AppError::config("get needs a <msgId>")),
    };

    let handle = ctx.resolve_handle(target)?;
    let message: Message = ctx.receiver()?.get(&handle, msg_id).await?;

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
