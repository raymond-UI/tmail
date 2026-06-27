//! tmail — an agent-first disposable-email CLI.
//!
//! Entrypoint: parse args, run the dispatcher on a Tokio runtime, and map the
//! `Result` to a stable process exit code (DESIGN.md §3, §8).

// The crate is built epic by epic; foundational APIs land before their first
// caller. This allow is removed in the final epic, which re-validates that
// nothing is genuinely dead once every command is wired.
#![allow(dead_code)]

mod app;
mod cli;
mod commands;
mod config;
mod error;
mod http;
mod model;
mod otp;
mod output;
mod receive;
mod send;
mod store;
mod util;
mod wait;

use clap::Parser;

use crate::cli::Cli;
use crate::error::{AppError, ErrorCode};

fn main() {
    let cli = Cli::parse();
    let pretty = cli.globals.pretty;

    let result = build_runtime().and_then(|rt| rt.block_on(app::dispatch(cli)));

    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            output::emit_error(&e, pretty);
            e.exit_code()
        }
    };
    std::process::exit(code);
}

fn build_runtime() -> error::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::new(ErrorCode::Generic, format!("runtime init: {e}")))
}
