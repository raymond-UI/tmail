//! Outbound mail: `send` (DESIGN.md §4, §7).

use std::io::Read;
use std::path::Path;

use serde_json::json;

use crate::app::Ctx;
use crate::cli::SendArgs;
use crate::error::{AppError, ErrorCode, Result};
use crate::output::emit_success;
use crate::send::smtp::SmtpSender;
use crate::send::{Attachment, OutboundMessage, Sender};

/// `tmail send` — compose and send via the configured SMTP transport.
pub async fn run_send(ctx: &Ctx, args: &SendArgs) -> Result<()> {
    if args.to.is_empty() {
        return Err(AppError::config("send needs at least one --to recipient"));
    }

    let from = ctx
        .config
        .resolve_from(args.from.as_deref())
        .ok_or_else(|| AppError::config("no sender: pass --from or set [smtp].from in config"))?;

    let url = ctx
        .config
        .resolve_smtp_url(args.smtp_url.as_deref())
        .ok_or_else(|| {
            AppError::config(
                "no SMTP config: pass --smtp-url, set TMAIL_SMTP_URL, or [smtp].url in config",
            )
        })?;

    let body = resolve_body(args)?;
    let attachments = args
        .attach
        .iter()
        .map(|p| read_attachment(p))
        .collect::<Result<Vec<_>>>()?;

    let message = OutboundMessage {
        from,
        to: args.to.clone(),
        cc: args.cc.clone(),
        bcc: args.bcc.clone(),
        reply_to: args.reply_to.clone(),
        subject: args.subject.clone().unwrap_or_default(),
        body,
        html: args.html,
        attachments,
    };

    let sender = SmtpSender::from_url(&url)?;
    let receipt = sender.send(message).await?;

    emit_success(
        &json!({
            "messageId": receipt.message_id,
            "accepted": receipt.accepted,
            "transport": receipt.transport,
        }),
        ctx.pretty(),
    )
}

/// Body precedence: `--body` > `--body-file` > stdin (DESIGN.md §7).
fn resolve_body(args: &SendArgs) -> Result<String> {
    if let Some(body) = &args.body {
        return Ok(body.clone());
    }
    if let Some(path) = &args.body_file {
        return std::fs::read_to_string(path).map_err(|e| {
            AppError::config(format!("cannot read --body-file {}: {e}", path.display()))
        });
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| AppError::new(ErrorCode::Generic, format!("cannot read stdin body: {e}")))?;
    Ok(buf)
}

fn read_attachment(path: &Path) -> Result<Attachment> {
    let data = std::fs::read(path)
        .map_err(|e| AppError::config(format!("cannot read --attach {}: {e}", path.display())))?;
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string());
    Ok(Attachment {
        content_type: guess_mime(path).to_string(),
        filename,
        data,
    })
}

/// Guess a MIME type from the file extension (DESIGN.md §7).
fn guess_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "text" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mime_guessing_covers_common_types() {
        assert_eq!(guess_mime(&PathBuf::from("a.pdf")), "application/pdf");
        assert_eq!(guess_mime(&PathBuf::from("a.PNG")), "image/png");
        assert_eq!(
            guess_mime(&PathBuf::from("a.unknownext")),
            "application/octet-stream"
        );
        assert_eq!(
            guess_mime(&PathBuf::from("noext")),
            "application/octet-stream"
        );
    }
}
