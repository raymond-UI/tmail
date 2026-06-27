//! Inbox lifecycle commands: `new`, `ls`, `rm` (DESIGN.md §4).

use serde_json::json;

use crate::app::Ctx;
use crate::cli::{NewArgs, RmArgs};
use crate::error::{AppError, ErrorCode, Result};
use crate::model::Handle;
use crate::output::emit_success;
use crate::receive::Receiver;
use crate::store::Store;

/// `tmail new` — mint an inbox; persist it unless `--stateless`.
pub async fn run_new(ctx: &Ctx, args: &NewArgs) -> Result<()> {
    let record = ctx.receiver()?.new_inbox().await?;

    if args.stateless {
        // Emit the view plus the opaque handle blob; do not touch local disk.
        let mut value = serde_json::to_value(record.view())?;
        value["handle"] = json!(record.handle.encode()?);
        emit_success(&value, ctx.pretty())
    } else {
        Store::open_default()?.add(record.clone())?;
        emit_success(&record.view(), ctx.pretty())
    }
}

/// `tmail ls` — list locally-stored inboxes, newest-first.
pub fn run_ls(ctx: &Ctx) -> Result<()> {
    let views: Vec<_> = Store::open_default()?
        .load()?
        .iter()
        .map(|r| r.view())
        .collect();
    emit_success(&views, ctx.pretty())
}

/// `tmail rm` — best-effort upstream delete, then forget locally. Idempotent.
pub async fn run_rm(ctx: &Ctx, args: &RmArgs) -> Result<()> {
    // Stateless: delete using the supplied handle, nothing local to forget.
    if let Some(blob) = &ctx.globals.handle {
        let handle = Handle::decode(blob)?;
        let existed = ctx.receiver()?.delete(&handle).await.is_ok();
        return emit_success(
            &json!({ "removed": handle.address, "existed": existed }),
            ctx.pretty(),
        );
    }

    let target = args.target.as_deref().ok_or_else(|| {
        AppError::new(
            ErrorCode::Config,
            "rm needs <id|address> or --handle/TMAIL_HANDLE",
        )
    })?;

    let removed = Store::open_default()?.remove(target)?;
    if let Some(record) = &removed {
        let _ = ctx.receiver()?.delete(&record.handle).await; // best-effort
    }
    // Idempotent: unknown target still exits 0, but `existed` tells the caller
    // whether anything was actually there (DESIGN.md §4).
    let existed = removed.is_some();
    let id = removed.map(|r| r.id).unwrap_or_else(|| target.to_string());
    emit_success(&json!({ "removed": id, "existed": existed }), ctx.pretty())
}
