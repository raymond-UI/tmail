//! Command dispatch and shared run context.
//!
//! Handlers are filled in epic by epic; until then they return a typed
//! `GENERIC` "not implemented" error so the binary always honors the contract.

use crate::cli::{Cli, Command, GlobalArgs};
use crate::config::Config;
use crate::error::{AppError, ErrorCode, Result};

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
}

/// Load shared state and route to the matching command handler.
pub async fn dispatch(cli: Cli) -> Result<()> {
    let config = Config::load(cli.globals.config.as_deref())?;
    let _ctx = Ctx {
        globals: cli.globals,
        config,
    };

    match cli.command {
        Command::New(_) => not_implemented("new"),
        Command::Ls => not_implemented("ls"),
        Command::Read(_) => not_implemented("read"),
        Command::Get(_) => not_implemented("get"),
        Command::Wait(_) => not_implemented("wait"),
        Command::Otp(_) => not_implemented("otp"),
        Command::Rm(_) => not_implemented("rm"),
        Command::Send(_) => not_implemented("send"),
    }
}

fn not_implemented(cmd: &str) -> Result<()> {
    Err(AppError::new(
        ErrorCode::Generic,
        format!("command not yet implemented: {cmd}"),
    ))
}
