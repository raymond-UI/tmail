//! Command dispatch and shared run context.

use std::time::Duration;

use crate::cli::{Cli, Command, GlobalArgs};
use crate::commands;
use crate::config::Config;
use crate::error::{AppError, ErrorCode, Result};
use crate::http;
use crate::model::Handle;
use crate::receive::mailtm::MailTm;
use crate::store::Store;

/// Default request timeout for non-blocking commands when `--timeout` is unset.
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Everything a command handler needs: parsed globals and loaded config.
pub struct Ctx {
    pub globals: GlobalArgs,
    pub config: Config,
}

impl Ctx {
    /// Whether to pretty-print JSON output.
    pub fn pretty(&self) -> bool {
        self.globals.pretty
    }

    /// Per-request HTTP timeout (the global `--timeout`, else a default).
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.globals.timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS))
    }

    /// Build the mail.tm receive backend.
    pub fn receiver(&self) -> Result<MailTm> {
        Ok(MailTm::new(http::client(self.request_timeout())?))
    }

    /// Resolve the inbox handle for a receive command.
    ///
    /// Precedence: `--handle`/`TMAIL_HANDLE` wins; a positional `<id|address>`
    /// is then optional but, if given, must match the handle's address
    /// (DESIGN.md §3). Without a handle, the positional is looked up in the
    /// local store.
    pub fn resolve_handle(&self, target: Option<&str>) -> Result<Handle> {
        if let Some(blob) = &self.globals.handle {
            let handle = Handle::decode(blob).map_err(|e| explain_handle_error(blob, e))?;
            if let Some(t) = target {
                let matches = t.eq_ignore_ascii_case(&handle.address);
                if !matches {
                    return Err(AppError::new(
                        ErrorCode::Config,
                        "positional <id|address> does not match the supplied --handle",
                    ));
                }
            }
            return Ok(handle);
        }

        let store = Store::open_default()?;
        match target {
            Some(target) => store
                .find(target)?
                .map(|r| r.handle)
                .ok_or_else(|| AppError::not_found(format!("no local inbox for '{target}'"))),
            // No target and no handle: fall back to the sole inbox if unambiguous.
            None => {
                let mut records = store.load()?;
                match records.len() {
                    1 => Ok(records.remove(0).handle),
                    0 => Err(AppError::new(
                        ErrorCode::Config,
                        "no stored inbox; pass <id|address> or --handle/TMAIL_HANDLE",
                    )),
                    _ => Err(AppError::new(
                        ErrorCode::Config,
                        "multiple inboxes stored; specify <id|address> or --handle/TMAIL_HANDLE",
                    )),
                }
            }
        }
    }
}

/// A failed `--handle` decode is almost always an inbox id or address passed
/// where the base64 blob belongs — name the mistake instead of leaving a bare
/// "invalid handle blob".
fn explain_handle_error(blob: &str, err: AppError) -> AppError {
    // Best-effort: if the value resolves in the local store, we can confirm
    // (not just guess) what the caller meant. Store problems keep the original
    // error.
    let stored_address = Store::open_default()
        .ok()
        .and_then(|s| s.find(blob.trim()).ok().flatten())
        .map(|r| r.address);
    match handle_blob_hint(blob.trim(), stored_address.as_deref()) {
        Some(hint) => AppError::new(ErrorCode::Config, format!("{}; {hint}", err.message)),
        None => err,
    }
}

/// The hint text for [`explain_handle_error`], pure so it is unit-testable
/// without touching the real store.
fn handle_blob_hint(value: &str, stored_address: Option<&str>) -> Option<String> {
    const BLOB_HELP: &str =
        "--handle expects the opaque base64 blob printed by `tmail new --stateless`";
    let looks_like_id =
        !value.is_empty() && value.len() <= 16 && value.chars().all(|c| c.is_ascii_alphanumeric());
    if let Some(address) = stored_address {
        Some(format!(
            "'{value}' is a stored inbox ({address}) — pass it positionally instead (e.g. `tmail otp {value}`); {BLOB_HELP}"
        ))
    } else if value.contains('@') {
        Some(format!(
            "that looks like an inbox address — pass it positionally instead (e.g. `tmail otp {value}`); {BLOB_HELP}"
        ))
    } else if looks_like_id {
        Some(format!(
            "that looks like an inbox id — pass it positionally instead (e.g. `tmail otp {value}`); {BLOB_HELP}"
        ))
    } else {
        None
    }
}

/// Load shared state and route to the matching command handler.
pub async fn dispatch(cli: Cli) -> Result<()> {
    let config = Config::load(cli.globals.config.as_deref())?;
    let ctx = Ctx {
        globals: cli.globals,
        config,
    };

    match cli.command {
        Command::New(a) => commands::inbox::run_new(&ctx, &a).await,
        Command::Ls => commands::inbox::run_ls(&ctx),
        Command::Rm(a) => commands::inbox::run_rm(&ctx, &a).await,
        Command::Read(a) => commands::read::run_read(&ctx, &a).await,
        Command::Get(a) => commands::read::run_get(&ctx, &a).await,
        Command::Wait(a) => commands::wait::run_wait(&ctx, &a).await,
        Command::Otp(a) => commands::wait::run_otp(&ctx, &a).await,
        Command::Send(a) => commands::send::run_send(&ctx, &a).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_names_a_stored_inbox_when_found() {
        let hint = handle_blob_hint("a1b2c3", Some("k7f2x9@punkproof.com")).unwrap();
        assert!(hint.contains("stored inbox"));
        assert!(hint.contains("k7f2x9@punkproof.com"));
        assert!(hint.contains("tmail otp a1b2c3"));
    }

    #[test]
    fn hint_recognizes_an_address() {
        let hint = handle_blob_hint("k7f2x9@punkproof.com", None).unwrap();
        assert!(hint.contains("looks like an inbox address"));
    }

    #[test]
    fn hint_recognizes_a_short_id() {
        let hint = handle_blob_hint("a1b2c3", None).unwrap();
        assert!(hint.contains("looks like an inbox id"));
        assert!(hint.contains("--stateless"));
    }

    #[test]
    fn no_hint_for_a_corrupt_long_blob() {
        // A mangled real blob (long, base64-ish) is not id/address misuse; the
        // original decode error should pass through unchanged.
        assert!(handle_blob_hint(&"eyJhY2NvdW50X2lkIj".repeat(10), None).is_none());
    }
}
