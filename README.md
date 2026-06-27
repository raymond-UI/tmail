# tmail

An **agent-first** disposable-email CLI. One binary an AI agent drives to:

1. **mint** a fresh, real, disposable inbox (via [mail.tm](https://mail.tm)),
2. **read** what arrives — including *blocking until* a message lands (OTP / verification flows), and
3. **send** outbound mail through your own SMTP account.

Every command prints exactly one JSON value to stdout, uses stable exit codes,
and never prompts. See [`DESIGN.md`](./DESIGN.md) for the full contract.

> Status: under active construction. See the [issue tracker](https://github.com/raymond-UI/tmail/issues) for progress.

## License

MIT
