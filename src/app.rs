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
            let handle = Handle::decode(blob)?;
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

        let target = target.ok_or_else(|| {
            AppError::new(
                ErrorCode::Config,
                "this command needs <id|address> or --handle/TMAIL_HANDLE",
            )
        })?;
        Store::open_default()?
            .find(target)?
            .map(|r| r.handle)
            .ok_or_else(|| AppError::not_found(format!("no local inbox for '{target}'")))
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

fn not_implemented(cmd: &str) -> Result<()> {
    Err(AppError::new(
        ErrorCode::Generic,
        format!("command not yet implemented: {cmd}"),
    ))
}
