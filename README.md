# tmail

An **agent-first** disposable-email CLI. One binary an AI agent drives to:

1. **mint** a fresh, real, disposable inbox (via [mail.tm](https://mail.tm)),
2. **read** what arrives — including *blocking until* a message lands (OTP / verification flows), and
3. **send** outbound mail through your own SMTP account.

Every command prints exactly one JSON value to stdout, uses stable exit codes,
and never prompts. See [`DESIGN.md`](./DESIGN.md) for the full contract.

> Status: under active construction. See the [issue tracker](https://github.com/raymond-UI/tmail/issues) for progress.

## Install

**macOS / Linux** — one line (downloads a prebuilt binary, verifies its checksum):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/raymond-UI/tmail/main/install.sh | sh
```

Pin a version or install location with `TMAIL_VERSION` / `TMAIL_INSTALL_DIR`.

**Any platform** — grab the archive for your target (macOS arm64/x86_64, Linux
x86_64/aarch64 incl. musl, Windows x86_64) from the
[Releases page](https://github.com/raymond-UI/tmail/releases) and put the `tmail`
binary on your `PATH`.

**From source** (needs the Rust toolchain):

```sh
cargo install --git https://github.com/raymond-UI/tmail
```

## License

MIT
