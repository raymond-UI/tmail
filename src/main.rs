//! tmail — an agent-first disposable-email CLI.
//!
//! Entrypoint: parse args, run the dispatcher on a Tokio runtime, and map the
//! `Result` to a stable process exit code (DESIGN.md §3, §8).

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

use clap::error::ErrorKind;
use clap::Parser;

use crate::cli::Cli;
use crate::error::{AppError, ErrorCode};

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => exit_from_clap_error(e),
    };
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

/// Honor the agent contract for argument parsing: `--help`/`--version` print
/// normally to stdout (exit 0); every other usage error becomes a JSON error
/// envelope on stdout with a distinct `CONFIG` exit code (7) — never the `2`
/// that collides with `NOT_FOUND`.
fn exit_from_clap_error(e: clap::Error) -> ! {
    match e.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            // Explicit --help/--version: clap renders it to stdout.
            let _ = e.print();
            std::process::exit(0);
        }
        _ => {
            let message = e
                .to_string()
                .lines()
                .next()
                .unwrap_or("invalid usage")
                .trim_start_matches("error: ")
                .to_string();
            let err = AppError::new(ErrorCode::Config, format!("usage: {message}"));
            output::emit_error(&err, false);
            std::process::exit(err.exit_code());
        }
    }
}

fn build_runtime() -> error::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::new(ErrorCode::Generic, format!("runtime init: {e}")))
}
