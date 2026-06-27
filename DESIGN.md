# tmail — an agent-first email CLI

> Working name: `tmail`. Rename freely; it only appears in `Cargo.toml`, the
> binary name, and config paths.

A single command-line tool an **AI agent** drives to:

1. **generate** a fresh, real, disposable inbox (mail.tm),
2. **read** what arrives at it (including blocking until a mail lands), and
3. **send** outbound mail (SMTP).

The primary consumer is an LLM agent, not a human. Every design decision below
follows from that.

---

## 1. Goals & non-goals

**Goals**
- Mint a working inbox in one call, with zero human setup.
- Read messages, and *block until* an expected message arrives (verification /
  OTP flows are the main use case).
- Send outbound mail through the user's own SMTP account.
- Output that an agent can parse deterministically: JSON on stdout, stable exit
  codes, never an interactive prompt.
- Single static binary, no runtime deps.

**Non-goals**
- Not a mail client / TUI. No human-facing inbox UI.
- Not a mail server. We don't host domains or route inbound ourselves.
- No threading, labels, search infra, or long-term archival.
- No AI content generation — the *agent* writes the email; this tool is plumbing.

---

## 2. The core architectural fact

**mail.tm is receive-only.** Its entire public API is `/domains`, `/accounts`,
`/token`, `/messages` (+ SSE). There is no endpoint to send *from* a mail.tm
address. Therefore the tool has **two independent transports**:

| Capability        | Transport | Identity                          |
|-------------------|-----------|-----------------------------------|
| generate + read   | mail.tm   | the disposable address it mints   |
| send              | SMTP      | the user's own SMTP account/`from`|

These are decoupled by design. A `send` does **not** originate from the
disposable address; it originates from whatever SMTP account is configured.
(Future transports — e.g. an IMAP receive backend or an API send backend — slot
in behind the same two traits without touching the command layer.)

---

## 3. Agent-first contract

Hard rules. Treat these as the spec, not suggestions.

- **stdout is data only.** Every command prints exactly one JSON value to stdout
  — on success *or* error (§8). Nothing else goes to stdout — no logs, no
  progress, no prose.
- **stderr is diagnostics.** Human/debug logging, gated by `-v/--verbose`.
- **No interactive prompts, ever.** Missing/invalid input → error exit, never a
  blocking question. Agents can't answer prompts.
- **Stable exit codes** (§8) so the agent branches without parsing prose.
- **Addressable by id _or_ address.** Agents lose opaque ids between turns;
  `read agent7@punkproof.com` must work as well as `read <id>`. Addresses are
  normalized to lowercase before lookup. Resolving by id/address requires the
  local store (it holds the handle/token), so it is mutually exclusive with
  `--stateless`: a stateless caller must pass `--handle` instead.
- **Stateless mode available.** `--handle <blob>` / `TMAIL_HANDLE` env lets an
  agent carry its own inbox credential instead of relying on local disk
  (different process, container, or machine each turn).
- **Never fabricate success.** If every attempt fails, exit non-zero with a typed
  error. A fake inbox/address is worse than a clean failure — the agent trusts
  whatever we print.
- **`--json` is the default.** A `--pretty` flag may add human formatting; the
  default and canonical output is compact JSON.

---

## 4. Commands

All commands accept global flags: `--json` (default on), `--pretty`, `-v`,
`--handle <blob>`, `--config <path>`, `--timeout <secs>`.

Two cross-cutting rules:
- **`--handle`/`TMAIL_HANDLE` makes the `<id|address>` positional optional** for
  every receive command — the address is read from the handle. If both are
  given, the handle wins and the positional must match it (else `7 CONFIG`).
- **`--timeout` precedence:** a command-level `--timeout` (e.g. on `wait`/`otp`)
  overrides the global flag, which overrides `[defaults].wait_timeout_secs`.

### `tmail new`
Mint a disposable inbox via mail.tm. Persists the handle (token + credentials)
to the local store unless `--stateless` is passed.

Flags: `--stateless` (don't persist; include the full handle in output).

```json
{ "id": "a1b2c3", "address": "k7f2x9@punkproof.com", "provider": "mail.tm",
  "createdAt": "2026-06-27T18:40:00Z",
  "handle": "<opaque base64; present only with --stateless>" }
```

### `tmail ls`
List locally-stored inboxes (most recent first). Empty array if none.

```json
[ { "id": "a1b2c3", "address": "k7f2x9@punkproof.com",
    "provider": "mail.tm", "createdAt": "2026-06-27T18:40:00Z" } ]
```

### `tmail read <id|address>`
List messages in an inbox (newest first). Does **not** block.

Flags: `--unread`, `--since <iso>`, `--limit <n>` (default 50).

```json
[ { "id": "msg_01", "from": "noreply@github.com", "subject": "Verify your email",
    "intro": "Your code is …", "date": "2026-06-27T18:41:12Z", "seen": false } ]
```

### `tmail get <id|address> <msgId>`
Full body of one message. Prefers HTML, also returns a text rendering.

Flags: `--html` (raw HTML), `--text` (plain only, default for agents).

```json
{ "id": "msg_01", "from": "noreply@github.com", "subject": "Verify your email",
  "text": "Your code is 481920 …", "html": "<html>…</html>",
  "date": "2026-06-27T18:41:12Z" }
```

### `tmail wait <id|address>`
**Block** until a new message arrives, then print it (same shape as `get`).
The core agent verb. Uses mail.tm SSE when available, else polls with backoff.

**Definition of "new" (resolves the OTP race):** at command start, `wait`
snapshots the set of message ids already in the inbox. It resolves on the first
message whose id is **not** in that snapshot. *Exception:* any message already
present at start that is **unseen** *and* matches an active `--from`/`--subject`
filter resolves immediately — this catches the common case where the mail lands
between the agent submitting a form and calling `wait`. Passing `--since <iso>`
replaces the snapshot baseline with a timestamp: any message dated at/after it
qualifies, seen or not.

Flags: `--from <substr>`, `--subject <substr>` (only resolve on a match),
`--since <iso>` (override the baseline), `--timeout <secs>` (default 120; exit
`6 TIMEOUT` if nothing matches in time).

### `tmail otp <id|address>`
`wait` + extract a verification code from the matched message. The single most
useful command for signup automation.

Flags: same as `wait`, plus `--pattern <regex>` (override the default
code-extraction regex), `--len <n>` (expected digits, default tries 4–8).

```json
{ "code": "481920", "msgId": "msg_01", "from": "noreply@github.com",
  "matchedBy": "default-digits" }
```

Default extraction: scan text body for the first run matching
`\b(\d{4,8})\b` near keywords (`code`, `otp`, `verify`, `pin`), falling back to
the first standalone 4–8 digit run. `--pattern` with one capture group overrides.

Before scanning: render HTML-only bodies to text (`html2text`) and collapse
internal separators so codes printed as `481 920` or `481-920` (or split across
markup) still match. **If a message matches the filter but no code can be
extracted, exit `9 NO_MATCH`** (distinct from `6 TIMEOUT`, which means no message
matched in time) — the agent can then fall back to `get` and parse itself.

### `tmail rm <id|address>`
Best-effort delete upstream (mail.tm `DELETE /accounts/{id}`), then forget
locally. Idempotent: removing an unknown id still exits `0`.

```json
{ "removed": "a1b2c3" }
```

### `tmail send`
Send outbound mail via the configured SMTP transport (§7). Body can come from a
flag, a file, or stdin (so an agent can pipe a generated body in).

Flags: `--to <addr>` (repeatable), `--cc`, `--bcc`, `--from <addr>`,
`--subject <s>`, `--body <s>` | `--body-file <path>` | (stdin if neither),
`--html` (treat body as HTML), `--attach <path>` (repeatable), `--reply-to`.

```json
{ "messageId": "<generated-id@host>", "accepted": ["dest@example.com"],
  "transport": "smtp" }
```

---

## 5. Data model

```rust
// A stored inbox. `handle` holds provider secrets and never appears in `ls`.
struct InboxRecord {
    id: String,            // our short id
    address: String,
    provider: String,      // "mail.tm"
    handle: Handle,        // { account_id, address, password, token }
    created_at: String,    // ISO-8601
}

// Normalized, provider-agnostic message.
struct Message {
    id: String,
    from: String,
    subject: String,
    intro: String,
    text: String,          // best-effort plain rendering
    html: Option<String>,
    date: String,          // ISO-8601
    seen: bool,
}
```

`InboxView` (what `ls`/`new` print) is `InboxRecord` minus `handle`.
The `--stateless` handle blob is a base64 of the serialized `Handle`.

---

## 6. mail.tm integration notes

Endpoints used: `GET /domains`, `POST /accounts`, `POST /token`,
`GET /messages`, `GET /messages/{id}`, `DELETE /accounts/{id}`.

Hard-won details to bake in from day one:

- **Token refresh.** Tokens expire. On a `401` from a `messages` call, re-auth
  once with the stored `address` + `password`, then retry.
- **Dual response shape.** mail.tm returns a plain JSON array *or* a Hydra
  collection (`{ "hydra:member": [...] }`) depending on the `Accept` header.
  Deserialize tolerantly (serde `#[serde(untagged)]` enum, or a custom
  `Deserialize` that unwraps `hydra:member` when present). Do not assume one.
- **Rate limits.** Account creation is rate-limited (`429`). Honor `Retry-After`;
  back off and surface `RATE_LIMITED` (don't crash, don't fake an address). Agents
  minting inboxes in bulk will hit this — `new` retries within the request
  `--timeout`, then exits `3 RATE_LIMITED` with `retryAfterMs` so the caller can
  pace itself rather than hammering.
- **SSE vs poll.** Real-time delivery is via a Mercure hub (bearer-authed,
  topic-scoped). It's the better path for `wait`/`otp` but fiddly. **Default to
  polling** `GET /messages` on an interval with backoff; treat SSE as an
  optimization behind a flag/feature once the poll path is solid.
- **Body hydration.** The list endpoint gives `intro`; fetch `GET /messages/{id}`
  for full `text`/`html`. `html` is an array of strings — join it.
- **Address local-part** must start with a letter; generate from a random
  alphanumeric. Pick an `isActive && !isPrivate` domain from `/domains`.

---

## 7. SMTP send

Transport configured once; reused by every `send`.

- **Crate:** `lettre` (async, `tokio1` + `rustls` features; no OpenSSL).
- **Config source, in precedence order:**
  1. `--smtp-url` flag,
  2. `TMAIL_SMTP_URL` env,
  3. `config.toml` `[smtp]` section.
- **URL form:** `smtps://user:pass@smtp.host:465` (implicit TLS) or
  `smtp://user:pass@smtp.host:587` (STARTTLS). Percent-encode credentials.
- **Default `from`:** falls back to `[smtp].from` when `--from` is omitted.
  Sender authorization can't be checked locally — most providers reject a `from`
  the account isn't allowed to use, so detect that server rejection and surface a
  clear `AUTH`/`PROVIDER_ERROR` rather than pretending to pre-validate.
- **Attachments:** `--attach` reads the file, guesses MIME from extension.
- **Body from stdin** when neither `--body` nor `--body-file` is given, so an
  agent can pipe a composed message straight in.

Known good setups to document for users: Gmail (app-password + `smtp.gmail.com`),
Fastmail, Resend SMTP, Mailgun SMTP.

---

## 8. Error model & exit codes

Every failure prints exactly one JSON error value to **stdout** (so the agent
always gets structured output, same invariant as success) *and* a human line to
stderr.

```json
{ "error": { "code": "RATE_LIMITED", "message": "mail.tm is cooling down",
             "retryAfterMs": 30000 } }
```

| Exit | Code                | Meaning                                         |
|------|---------------------|-------------------------------------------------|
| 0    | —                   | success                                         |
| 1    | `GENERIC`           | unexpected/unclassified                         |
| 2    | `NOT_FOUND`         | inbox/message id unknown                         |
| 3    | `RATE_LIMITED`      | provider 429; `retryAfterMs` included           |
| 4    | `ALL_PROVIDERS_DOWN`| no inbox could be minted (v1: mail.tm unreachable after retries) |
| 5    | `AUTH`              | bad/missing SMTP or provider credentials        |
| 6    | `TIMEOUT`           | `wait`/`otp` deadline passed with no matching message |
| 7    | `CONFIG`            | malformed config / missing required setting      |
| 8    | `NETWORK`           | connection/DNS/TLS failure                       |
| 9    | `NO_MATCH`          | `otp`: message matched but no code could be extracted |

Rule: distinct, machine-branchable codes; the message field is for humans only.

---

## 9. Config & secrets

- **Config file:** `~/.config/tmail/config.toml` (resolved via the `directories`
  crate; respects `XDG_CONFIG_HOME`).
- **Inbox store:** `~/.local/share/tmail/inboxes.json`, **`chmod 0600`** — it
  holds mail.tm tokens and passwords. The dataset is tiny, but write **atomically**
  (write a sibling temp file, `chmod 0600`, then `rename` over the target) so
  concurrent invocations can't clobber each other or briefly expose the file at
  default perms.
- **Never log secrets.** Tokens/passwords must not appear in `-v` output.
- **`--stateless` handle is a credential in the clear.** The base64 blob carries
  the account password + token unencrypted and is printed to stdout, so it can
  land in agent transcripts/logs. That's inherent to carrying state out-of-band;
  callers must treat the handle as a secret.

```toml
# ~/.config/tmail/config.toml
[smtp]
url  = "smtps://me%40gmail.com:app-password@smtp.gmail.com:465"
from = "me@gmail.com"

[provider]
# reserved for future multi-provider support; mail.tm is the only one for now.
default = "mail.tm"

[defaults]
wait_timeout_secs = 120
poll_interval_secs = 3
```

---

## 10. Proposed crate layout

```
tmail/
├── Cargo.toml
├── DESIGN.md
└── src/
    ├── main.rs            # clap parsing → dispatch; maps Result → exit code
    ├── cli.rs             # clap command/flag definitions
    ├── output.rs          # JSON-to-stdout, error-to-stdout+stderr, exit codes
    ├── config.rs          # load/merge config.toml + env + flags
    ├── store.rs           # InboxRecord persistence (0600 JSON file)
    ├── error.rs           # AppError { code, message, retry_after } + From impls
    ├── receive/
    │   ├── mod.rs         # `Receiver` trait (new/read/get/wait/delete)
    │   └── mailtm.rs      # mail.tm impl (dual-shape, token refresh, poll/SSE)
    ├── send/
    │   ├── mod.rs         # `Sender` trait
    │   └── smtp.rs        # lettre impl
    ├── otp.rs             # code extraction
    └── http.rs            # reqwest client: timeout, retry-after parsing
```

**Trait seams** (so backends are swappable and testable):

```rust
#[async_trait]
trait Receiver {
    async fn new_inbox(&self) -> Result<InboxRecord>;
    async fn read(&self, h: &Handle) -> Result<Vec<Message>>;
    async fn get(&self, h: &Handle, msg_id: &str) -> Result<Message>;
    async fn delete(&self, h: &Handle) -> Result<()>;
    // `wait`/`otp` are built on top of `read` in a generic poll loop.
}

#[async_trait]
trait Sender {
    async fn send(&self, msg: OutboundMessage) -> Result<SendReceipt>;
}
```

---

## 11. Dependencies (proposed)

| Concern        | Crate                                            |
|----------------|--------------------------------------------------|
| CLI parsing    | `clap` (derive)                                  |
| async runtime  | `tokio`                                           |
| HTTP           | `reqwest` (`rustls-tls`, `json`)                 |
| JSON / models  | `serde`, `serde_json`                            |
| SMTP send      | `lettre` (`tokio1-rustls-tls`)                   |
| HTML → text    | `html2text` (for readable `text`/otp scanning)   |
| regex (otp)    | `regex`                                           |
| config paths   | `directories`                                    |
| config format  | `toml`                                            |
| time           | `time` (ISO-8601, no `chrono` C deps)            |
| randomness     | `rand`                                            |
| errors         | `thiserror` (define `AppError`)                  |
| async traits   | `async-trait`                                    |
| base64 handle  | `base64`                                          |
| SSE (optional) | `reqwest-eventsource` (later, behind a feature)  |

All TLS via `rustls` — no OpenSSL/system deps, keeps the static-binary promise.

---

## 12. Example agent workflows

**Sign up somewhere and confirm via OTP**
```bash
addr=$(tmail new | jq -r .address)
# … agent submits `addr` to the target signup form …
code=$(tmail otp "$addr" --from noreply@target.com --timeout 180 | jq -r .code)
# … agent submits `code` …
```

**Read everything that arrived, then clean up**
```bash
tmail read "$addr" --json | jq '.[] | {from, subject}'
tmail rm "$addr"
```

**Send a composed message (body piped from the agent)**
```bash
echo "$body" | tmail send --to user@example.com --subject "Re: your request" --html
```

**Stateless (no local disk; carry the handle)**
```bash
out=$(tmail new --stateless); h=$(echo "$out" | jq -r .handle)
tmail wait --handle "$h" --timeout 120
```

---

## 13. Testing strategy

- **Pure units:** otp extraction, the Hydra/array dual-shape deserializer,
  config precedence merge, exit-code mapping. No network.
- **Receiver against a mock:** `wiremock` to stand in for api.mail.tm; assert
  token-refresh-on-401, 429 → `RATE_LIMITED`, body hydration, dual-shape parse.
- **Sender:** lettre against a local SMTP sink (`mailtutan`/`mailpit` in CI, or
  lettre's stub transport) to assert envelope, attachments, STARTTLS vs implicit.
- **CLI contract tests:** `assert_cmd` to verify stdout-is-only-JSON, stderr
  carries logs, and each error path returns its documented exit code.
- **Poll loop with an injected clock:** abstract time/sleep behind a trait so
  `wait`/`otp` baseline, backoff, and timeout (`6 TIMEOUT`) logic — and the
  no-code `9 NO_MATCH` path — are unit-testable without real sleeps.

---

## 14. Open questions / future

- **Multi-provider receive** — keep the `Receiver` trait so guerrilla/maildrop/
  others can be added with failover later; out of scope for v1.
- **SSE** — ship polling first; add Mercure SSE behind a feature once stable.
- **Send-as-disposable** — only possible by switching the send transport to an
  API tied to a domain you own; explicitly not mail.tm.
- **Concurrent `wait`** on many inboxes — a `watch` subcommand could multiplex;
  defer until there's a real need.
