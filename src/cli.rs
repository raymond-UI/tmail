//! clap command and flag definitions (DESIGN.md §4).
//!
//! Global flags apply to every subcommand. Per-command flags mirror the design
//! exactly so the agent contract is the source of truth.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// An agent-first disposable-email CLI.
#[derive(Debug, Parser)]
#[command(name = "tmail", version, about, long_about = None)]
pub struct Cli {
    /// Flags common to every subcommand.
    #[command(flatten)]
    pub globals: GlobalArgs,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Flags accepted by every subcommand.
#[derive(Debug, Args, Clone)]
pub struct GlobalArgs {
    /// Emit JSON (the default and canonical output).
    #[arg(long, global = true, default_value_t = true)]
    pub json: bool,

    /// Pretty-print the JSON output for humans.
    #[arg(long, global = true)]
    pub pretty: bool,

    /// Increase diagnostic verbosity on stderr (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Carry an inbox handle out-of-band instead of using the local store.
    #[arg(long, global = true, env = "TMAIL_HANDLE")]
    pub handle: Option<String>,

    /// Path to a config file (overrides the default location).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Request timeout in seconds (also the default for the blocking verbs).
    #[arg(long, global = true)]
    pub timeout: Option<u64>,
}

/// The tmail subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Mint a disposable inbox via mail.tm.
    New(NewArgs),
    /// List locally-stored inboxes (most recent first).
    Ls,
    /// List messages in an inbox (newest first). Does not block.
    Read(ReadArgs),
    /// Print the full body of one message.
    Get(GetArgs),
    /// Block until a new message arrives, then print it.
    Wait(WaitArgs),
    /// Wait for a message and extract a verification code from it.
    Otp(OtpArgs),
    /// Delete an inbox upstream (best-effort) and forget it locally.
    Rm(RmArgs),
    /// Send outbound mail via the configured SMTP transport.
    Send(SendArgs),
}

/// `tmail new`
#[derive(Debug, Args)]
pub struct NewArgs {
    /// Do not persist; include the full handle blob in the output.
    #[arg(long)]
    pub stateless: bool,
}

/// `tmail read <id|address>`
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Inbox id or address (optional when `--handle` is supplied).
    pub target: Option<String>,
    /// Only show unseen messages.
    #[arg(long)]
    pub unread: bool,
    /// Only show messages at/after this ISO-8601 timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Maximum number of messages to return.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

/// `tmail get <id|address> <msgId>` — or just `<msgId>` with `--handle`/one inbox.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Inbox id or address; omit (give only `<msgId>`) when using `--handle` or
    /// when exactly one inbox is stored.
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    /// The message id. When only one positional is given it is taken as the
    /// message id and the inbox is resolved from `--handle` / the sole inbox.
    #[arg(value_name = "MSG_ID")]
    pub msg_id: Option<String>,
    /// Include only the HTML body field in the JSON output.
    #[arg(long, conflicts_with = "text")]
    pub html: bool,
    /// Include only the plain-text body field in the JSON output (default
    /// includes both).
    #[arg(long)]
    pub text: bool,
}

/// `tmail wait <id|address>`
#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Inbox id or address (optional when `--handle` is supplied).
    pub target: Option<String>,
    /// Only resolve on a message whose sender contains this substring.
    #[arg(long)]
    pub from: Option<String>,
    /// Only resolve on a message whose subject contains this substring.
    #[arg(long)]
    pub subject: Option<String>,
    /// Override the baseline: resolve on any message at/after this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Deadline in seconds (overrides the global `--timeout`).
    #[arg(long)]
    pub timeout: Option<u64>,
}

/// `tmail otp <id|address>`
#[derive(Debug, Args)]
pub struct OtpArgs {
    /// Inbox id or address (optional when `--handle` is supplied).
    pub target: Option<String>,
    /// Only consider messages whose sender contains this substring.
    #[arg(long)]
    pub from: Option<String>,
    /// Only consider messages whose subject contains this substring.
    #[arg(long)]
    pub subject: Option<String>,
    /// Override the baseline: consider any message at/after this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Deadline in seconds (overrides the global `--timeout`).
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Override the default code-extraction regex (one capture group).
    #[arg(long)]
    pub pattern: Option<String>,
    /// Expected number of digits in the code.
    #[arg(long)]
    pub len: Option<usize>,
}

/// `tmail rm <id|address>`
#[derive(Debug, Args)]
pub struct RmArgs {
    /// Inbox id or address (optional when `--handle` is supplied).
    pub target: Option<String>,
}

/// `tmail send`
#[derive(Debug, Args)]
pub struct SendArgs {
    /// Recipient address (repeatable).
    #[arg(long)]
    pub to: Vec<String>,
    /// CC address (repeatable).
    #[arg(long)]
    pub cc: Vec<String>,
    /// BCC address (repeatable).
    #[arg(long)]
    pub bcc: Vec<String>,
    /// Sender address (falls back to `[smtp].from`).
    #[arg(long)]
    pub from: Option<String>,
    /// Reply-To address.
    #[arg(long)]
    pub reply_to: Option<String>,
    /// Subject line.
    #[arg(long)]
    pub subject: Option<String>,
    /// Body text (mutually exclusive with `--body-file`).
    #[arg(long, conflicts_with = "body_file")]
    pub body: Option<String>,
    /// Read the body from a file.
    #[arg(long)]
    pub body_file: Option<PathBuf>,
    /// Treat the body as HTML.
    #[arg(long)]
    pub html: bool,
    /// Attach a file (repeatable).
    #[arg(long)]
    pub attach: Vec<PathBuf>,
    /// SMTP URL override (`smtps://…` or `smtp://…`).
    #[arg(long)]
    pub smtp_url: Option<String>,
}
