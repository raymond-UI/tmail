//! Blocking verbs: `wait`, `otp` (DESIGN.md §4).

use std::time::Duration;

use serde_json::json;

use crate::app::Ctx;
use crate::cli::{OtpArgs, WaitArgs};
use crate::error::{AppError, ErrorCode, Result};
use crate::model::Message;
use crate::otp;
use crate::output::emit_success;
use crate::util::require_rfc3339;
use crate::wait::{wait_for_match, Filters, RealClock};

/// Shared block-until-match used by both `wait` and `otp`.
async fn await_message(
    ctx: &Ctx,
    target: Option<&str>,
    from: Option<&str>,
    subject: Option<&str>,
    since: Option<&str>,
    timeout: Option<u64>,
) -> Result<Message> {
    let handle = ctx.resolve_handle(target)?;
    let receiver = ctx.receiver()?;

    let since = match since {
        Some(s) => Some(require_rfc3339("--since", s)?),
        None => None,
    };
    // Command --timeout > global --timeout > config > default (DESIGN.md §4).
    let deadline = Duration::from_secs(ctx.config.wait_timeout_secs(timeout));
    let interval = Duration::from_secs(ctx.config.poll_interval_secs());
    let filters = Filters { from, subject };
    let clock = RealClock::new();

    wait_for_match(
        &receiver, &handle, since, filters, interval, deadline, &clock,
    )
    .await
}

/// `tmail wait` — block until a matching message arrives, then print it.
pub async fn run_wait(ctx: &Ctx, args: &WaitArgs) -> Result<()> {
    let message = await_message(
        ctx,
        args.target.as_deref(),
        args.from.as_deref(),
        args.subject.as_deref(),
        args.since.as_deref(),
        args.timeout.or(ctx.globals.timeout),
    )
    .await?;
    emit_success(&message.full(), ctx.pretty())
}

/// `tmail otp` — wait, then extract a verification code (NO_MATCH if none).
pub async fn run_otp(ctx: &Ctx, args: &OtpArgs) -> Result<()> {
    let message = await_message(
        ctx,
        args.target.as_deref(),
        args.from.as_deref(),
        args.subject.as_deref(),
        args.since.as_deref(),
        args.timeout.or(ctx.globals.timeout),
    )
    .await?;

    match otp::extract_code(&message.text, args.pattern.as_deref(), args.len) {
        Some(found) => emit_success(
            &json!({
                "code": found.code,
                "msgId": message.id,
                "from": message.from,
                "matchedBy": found.matched_by,
            }),
            ctx.pretty(),
        ),
        None => Err(AppError::new(
            ErrorCode::NoMatch,
            format!(
                "matched message {} from {} but no code could be extracted",
                message.id, message.from
            ),
        )),
    }
}
