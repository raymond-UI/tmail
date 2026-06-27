//! End-to-end agent-contract tests (DESIGN.md §3, §8).
//!
//! Drives the built binary and asserts the stable exit codes and the
//! stdout-is-only-JSON guarantee for representative paths. Each run is isolated
//! to a throwaway HOME so the real store/config never interferes.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_tmail");

/// A handle blob for `{account_id:a, address:x@y.com, password:p, token:t}`,
/// used to exercise input validation without touching the local store.
const HANDLE_BLOB: &str =
    "eyJhY2NvdW50X2lkIjoiYSIsImFkZHJlc3MiOiJ4QHkuY29tIiwicGFzc3dvcmQiOiJwIiwidG9rZW4iOiJ0In0=";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run the binary with an isolated environment and null stdin.
fn run(args: &[&str]) -> Output {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let home: PathBuf = std::env::temp_dir().join(format!("tmail-it-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();

    let out = Command::new(BIN)
        .args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("TMAIL_SMTP_URL")
        .env_remove("TMAIL_HANDLE")
        .stdin(Stdio::null())
        .output()
        .expect("run tmail");

    let _ = std::fs::remove_dir_all(&home);
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// stdout must be exactly one JSON value (single line, starts with `{` or `[`).
fn assert_stdout_is_one_json_value(stdout: &str) {
    let trimmed = stdout.trim_end_matches('\n');
    assert!(!trimmed.is_empty(), "stdout was empty");
    assert!(
        !trimmed.contains('\n'),
        "stdout had multiple lines: {trimmed:?}"
    );
    let first = trimmed.chars().next().unwrap();
    assert!(
        first == '{' || first == '[',
        "stdout not a JSON value: {trimmed:?}"
    );
}

#[test]
fn unknown_inbox_is_not_found_exit_2() {
    let out = run(&["read", "definitely-not-a-real-inbox-xyz"]);
    assert_eq!(out.code, 2, "stderr: {}", out.stderr);
    assert_stdout_is_one_json_value(&out.stdout);
    assert!(out.stdout.contains("\"code\":\"NOT_FOUND\""));
    // Diagnostics go to stderr, not stdout.
    assert!(out.stderr.contains("NOT_FOUND"));
}

#[test]
fn send_without_recipients_is_config_exit_7() {
    let out = run(&["send", "--body", "hi"]);
    assert_eq!(out.code, 7, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("\"code\":\"CONFIG\""));
}

#[test]
fn send_without_smtp_config_is_config_exit_7() {
    let out = run(&[
        "send", "--to", "a@b.com", "--from", "me@x.com", "--body", "hi",
    ]);
    // from is supplied, but there is no SMTP URL anywhere -> CONFIG.
    assert_eq!(out.code, 7, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("\"code\":\"CONFIG\""));
}

#[test]
fn ls_empty_is_json_array_exit_0() {
    let out = run(&["ls"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout.trim_end(), "[]");
}

#[test]
fn rm_unknown_is_idempotent_exit_0() {
    let out = run(&["rm", "nope"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("\"removed\":\"nope\""));
}

#[test]
fn bad_since_is_config_exit_7_before_network() {
    // With a handle, resolution succeeds; the bad --since must fail fast (CONFIG)
    // before any network call.
    let out = run(&["read", "--handle", HANDLE_BLOB, "--since", "not-a-date"]);
    assert_eq!(out.code, 7, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("\"code\":\"CONFIG\""));
}

#[test]
fn missing_subcommand_is_usage_error() {
    // clap usage errors are distinct from our runtime contract codes.
    let out = run(&[]);
    assert_ne!(out.code, 0);
}
