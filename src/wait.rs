//! The generic block-until-message poll loop (DESIGN.md §4, §6, §13).
//!
//! Built on [`Receiver::read`] so any backend gets `wait`/`otp` for free. Time
//! is abstracted behind [`Clock`] so the baseline/backoff/timeout logic is
//! unit-testable without real sleeps.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::diag;
use crate::error::{AppError, ErrorCode, Result};
use crate::model::{Handle, Message};
use crate::receive::Receiver;
use crate::util::parse_rfc3339;

/// Abstracted, injectable time source.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Time elapsed since this clock was created.
    fn elapsed(&self) -> Duration;
    /// Sleep for `dur` (fake clocks advance `elapsed` instead).
    async fn sleep(&self, dur: Duration);
}

/// Production clock: monotonic `Instant` + real Tokio sleeps.
pub struct RealClock {
    start: Instant,
}

impl RealClock {
    /// Start a clock at "now".
    pub fn new() -> Self {
        RealClock {
            start: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for RealClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
    async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

/// Sender/subject substring filters (case-insensitive).
#[derive(Clone, Copy, Default)]
pub struct Filters<'a> {
    pub from: Option<&'a str>,
    pub subject: Option<&'a str>,
}

impl Filters<'_> {
    /// Whether any filter is set.
    pub fn active(&self) -> bool {
        self.from.is_some() || self.subject.is_some()
    }

    /// Whether `m` satisfies the active filters.
    pub fn matches(&self, m: &Message) -> bool {
        let from_ok = self
            .from
            .is_none_or(|f| m.from.to_lowercase().contains(&f.to_lowercase()));
        let subject_ok = self
            .subject
            .is_none_or(|s| m.subject.to_lowercase().contains(&s.to_lowercase()));
        from_ok && subject_ok
    }
}

/// What counts as a resolving message (DESIGN.md §4 "definition of new").
pub enum Baseline {
    /// Resolve on an id not present at start, or — when a filter is active —
    /// an already-present *unseen* message that matches.
    Snapshot(HashSet<String>),
    /// Resolve on any message dated at/after this instant (seen or not).
    Since(OffsetDateTime),
}

/// Find the first (newest-first) message that resolves the wait.
pub fn pick_match<'a>(
    messages: &'a [Message],
    baseline: &Baseline,
    filters: &Filters,
) -> Option<&'a Message> {
    messages.iter().find(|m| {
        if !filters.matches(m) {
            return false;
        }
        match baseline {
            Baseline::Since(since) => parse_rfc3339(&m.date).is_some_and(|d| d >= *since),
            Baseline::Snapshot(seen) => {
                let is_new = !seen.contains(&m.id);
                let immediate = filters.active() && !m.seen && seen.contains(&m.id);
                is_new || immediate
            }
        }
    })
}

/// Block until a message resolves, then return it fully hydrated.
///
/// `since` selects the [`Baseline`]: `Some` → time-based; `None` → snapshot the
/// ids present on the first read.
pub async fn wait_for_match(
    receiver: &dyn Receiver,
    handle: &Handle,
    since: Option<OffsetDateTime>,
    filters: Filters<'_>,
    interval: Duration,
    deadline: Duration,
    clock: &dyn Clock,
) -> Result<Message> {
    let mut snapshot: Option<HashSet<String>> = None;

    loop {
        // A transient provider hiccup (429/network) must not abort a long wait:
        // back off and keep polling until the real deadline (DESIGN.md §6).
        let mut messages = match receiver.read(handle).await {
            Ok(m) => m,
            Err(e) if is_retryable(&e) => {
                if clock.elapsed() >= deadline {
                    return Err(e);
                }
                back_off(&e, interval, deadline, clock).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        messages.sort_by_key(|m| std::cmp::Reverse(parse_rfc3339(&m.date)));

        let baseline = match since {
            Some(s) => Baseline::Since(s),
            None => {
                let ids =
                    snapshot.get_or_insert_with(|| messages.iter().map(|m| m.id.clone()).collect());
                Baseline::Snapshot(ids.clone())
            }
        };

        if let Some(m) = pick_match(&messages, &baseline, &filters) {
            let id = m.id.clone();
            // We found the match; don't lose it to a transient error on hydrate.
            return get_with_retry(receiver, handle, &id, interval, deadline, clock).await;
        }

        if clock.elapsed() >= deadline {
            return Err(AppError::timeout(format!(
                "no matching message within {}s",
                deadline.as_secs()
            )));
        }
        let remaining = deadline.saturating_sub(clock.elapsed());
        clock.sleep(interval.min(remaining)).await;
    }
}

/// Errors worth retrying mid-poll rather than aborting the wait: rate limiting
/// and transient transport failures. Everything else (auth, not-found, config)
/// is terminal.
fn is_retryable(e: &AppError) -> bool {
    matches!(
        e.code,
        ErrorCode::RateLimited | ErrorCode::Network | ErrorCode::Timeout
    )
}

/// Sleep after a retryable error: honor the provider's `Retry-After` when given,
/// else the normal poll interval, and never overshoot the remaining deadline.
async fn back_off(e: &AppError, interval: Duration, deadline: Duration, clock: &dyn Clock) {
    let wait = e
        .retry_after_ms
        .map(Duration::from_millis)
        .unwrap_or(interval);
    let remaining = deadline.saturating_sub(clock.elapsed());
    diag::log(1, || {
        format!(
            "wait: transient {} ({}); backing off {}ms",
            e.code,
            e.message,
            wait.min(remaining).as_millis()
        )
    });
    clock.sleep(wait.min(remaining)).await;
}

/// Hydrate the matched message, tolerating transient errors until the deadline.
async fn get_with_retry(
    receiver: &dyn Receiver,
    handle: &Handle,
    id: &str,
    interval: Duration,
    deadline: Duration,
    clock: &dyn Clock,
) -> Result<Message> {
    loop {
        match receiver.get(handle, id).await {
            Ok(m) => return Ok(m),
            Err(e) if is_retryable(&e) && clock.elapsed() < deadline => {
                back_off(&e, interval, deadline, clock).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    fn msg(id: &str, from: &str, subject: &str, seen: bool, date: &str) -> Message {
        Message {
            id: id.into(),
            from: from.into(),
            subject: subject.into(),
            intro: String::new(),
            text: format!("body of {id}"),
            html: None,
            date: date.into(),
            seen,
        }
    }

    // --- pure pick_match tests ---------------------------------------------

    #[test]
    fn snapshot_resolves_on_new_arrival_only() {
        let seen: HashSet<String> = ["old".to_string()].into_iter().collect();
        let baseline = Baseline::Snapshot(seen);
        let f = Filters::default();
        let old = vec![msg("old", "a@x", "s", false, "2026-06-27T10:00:00Z")];
        assert!(pick_match(&old, &baseline, &f).is_none());
        let with_new = vec![
            msg("new", "a@x", "s", false, "2026-06-27T11:00:00Z"),
            msg("old", "a@x", "s", false, "2026-06-27T10:00:00Z"),
        ];
        assert_eq!(pick_match(&with_new, &baseline, &f).unwrap().id, "new");
    }

    #[test]
    fn snapshot_immediate_match_for_present_unseen_with_filter() {
        // The form-submit race: mail already present at start, unseen, matches.
        let seen: HashSet<String> = ["m1".to_string()].into_iter().collect();
        let baseline = Baseline::Snapshot(seen);
        let f = Filters {
            from: Some("github"),
            subject: None,
        };
        let msgs = vec![msg(
            "m1",
            "noreply@github.com",
            "Verify",
            false,
            "2026-06-27T10:00:00Z",
        )];
        assert_eq!(pick_match(&msgs, &baseline, &f).unwrap().id, "m1");
    }

    #[test]
    fn snapshot_present_seen_does_not_match_even_with_filter() {
        let seen: HashSet<String> = ["m1".to_string()].into_iter().collect();
        let baseline = Baseline::Snapshot(seen);
        let f = Filters {
            from: Some("github"),
            subject: None,
        };
        let msgs = vec![msg(
            "m1",
            "noreply@github.com",
            "Verify",
            true,
            "2026-06-27T10:00:00Z",
        )];
        assert!(pick_match(&msgs, &baseline, &f).is_none());
    }

    #[test]
    fn since_resolves_on_or_after_timestamp() {
        let since = parse_rfc3339("2026-06-27T10:30:00Z").unwrap();
        let baseline = Baseline::Since(since);
        let f = Filters::default();
        let msgs = vec![
            msg("late", "a@x", "s", true, "2026-06-27T11:00:00Z"),
            msg("early", "a@x", "s", true, "2026-06-27T10:00:00Z"),
        ];
        assert_eq!(pick_match(&msgs, &baseline, &f).unwrap().id, "late");
    }

    // --- loop tests with a fake clock + fake receiver ----------------------

    struct FakeClock {
        elapsed_ms: AtomicU64,
    }
    #[async_trait]
    impl Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            Duration::from_millis(self.elapsed_ms.load(Ordering::Relaxed))
        }
        async fn sleep(&self, dur: Duration) {
            self.elapsed_ms
                .fetch_add(dur.as_millis() as u64, Ordering::Relaxed);
        }
    }

    struct FakeReceiver {
        // One result per successive read() call (an Err injects a transient
        // failure); an exhausted queue reads as empty.
        reads: Mutex<std::collections::VecDeque<Result<Vec<Message>>>>,
    }
    #[async_trait]
    impl Receiver for FakeReceiver {
        async fn new_inbox(&self) -> Result<crate::model::InboxRecord> {
            unimplemented!()
        }
        async fn read(&self, _h: &Handle) -> Result<Vec<Message>> {
            let mut q = self.reads.lock().unwrap();
            q.pop_front().unwrap_or_else(|| Ok(Vec::new()))
        }
        async fn get(&self, _h: &Handle, msg_id: &str) -> Result<Message> {
            Ok(msg(msg_id, "a@x", "s", false, "2026-06-27T11:00:00Z"))
        }
        async fn delete(&self, _h: &Handle) -> Result<bool> {
            Ok(true)
        }
    }

    fn handle() -> Handle {
        Handle {
            account_id: "acc".into(),
            address: "a@x.com".into(),
            password: "pw".into(),
            token: "tok".into(),
            created_at: Some("2026-06-27T09:00:00Z".into()),
        }
    }

    #[tokio::test]
    async fn returns_message_once_a_new_one_arrives() {
        let reads = [
            Ok(vec![]), // empty at start -> snapshot is empty
            Ok(vec![]), // still nothing
            Ok(vec![msg("new", "a@x", "s", false, "2026-06-27T11:00:00Z")]),
        ]
        .into_iter()
        .collect();
        let receiver = FakeReceiver {
            reads: Mutex::new(reads),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let got = wait_for_match(
            &receiver,
            &handle(),
            None,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(60),
            &clock,
        )
        .await
        .unwrap();
        assert_eq!(got.id, "new");
    }

    #[tokio::test]
    async fn since_baseline_resolves_on_already_present_message() {
        // The pre-start race: the code is already in the inbox on the very first
        // read. A `since` baseline (as seeded from the inbox's creation time)
        // must resolve immediately — no snapshot exclusion, seen or not.
        let reads = [Ok(vec![msg(
            "code",
            "noreply@acme.com",
            "Your code",
            true, // already marked seen by a prior debug read — must not matter
            "2026-06-27T10:00:00Z",
        )])]
        .into_iter()
        .collect();
        let receiver = FakeReceiver {
            reads: Mutex::new(reads),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let since = parse_rfc3339("2026-06-27T09:00:00Z");
        let got = wait_for_match(
            &receiver,
            &handle(),
            since,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(60),
            &clock,
        )
        .await
        .unwrap();
        assert_eq!(got.id, "code");
    }

    #[tokio::test]
    async fn transient_rate_limit_is_retried_not_fatal() {
        // First read is a 429 with a backoff hint; the wait must survive it and
        // resolve on the message that follows.
        let reads = [
            Err(AppError::rate_limited("slow down", Some(500))),
            Ok(vec![]), // recovered; empty -> seeds an empty snapshot
            Ok(vec![msg("new", "a@x", "s", false, "2026-06-27T11:00:00Z")]),
        ]
        .into_iter()
        .collect();
        let receiver = FakeReceiver {
            reads: Mutex::new(reads),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let got = wait_for_match(
            &receiver,
            &handle(),
            None,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(60),
            &clock,
        )
        .await
        .unwrap();
        assert_eq!(got.id, "new");
    }

    #[tokio::test]
    async fn persistent_rate_limit_surfaces_after_deadline() {
        // Nothing but 429s until the deadline: the caller should learn it was
        // rate-limited (exit 3), not a generic timeout.
        let reads = std::iter::repeat_with(|| Err(AppError::rate_limited("nope", Some(1000))))
            .take(10)
            .collect();
        let receiver = FakeReceiver {
            reads: Mutex::new(reads),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let err = wait_for_match(
            &receiver,
            &handle(),
            None,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(3),
            &clock,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::RateLimited);
    }

    #[tokio::test]
    async fn terminal_error_is_not_retried() {
        // A non-transient error (auth) must abort the wait immediately.
        let reads = [Err(AppError::auth("bad token"))].into_iter().collect();
        let receiver = FakeReceiver {
            reads: Mutex::new(reads),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let err = wait_for_match(
            &receiver,
            &handle(),
            None,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(60),
            &clock,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Auth);
    }

    #[tokio::test]
    async fn times_out_when_nothing_matches() {
        // Always empty; the fake clock advances on each sleep until the deadline.
        let receiver = FakeReceiver {
            reads: Mutex::new(std::collections::VecDeque::new()),
        };
        let clock = FakeClock {
            elapsed_ms: AtomicU64::new(0),
        };
        let err = wait_for_match(
            &receiver,
            &handle(),
            None,
            Filters::default(),
            Duration::from_secs(1),
            Duration::from_secs(3),
            &clock,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::Timeout);
    }
}
