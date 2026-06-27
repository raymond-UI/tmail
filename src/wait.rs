//! The generic block-until-message poll loop (DESIGN.md §4, §6, §13).
//!
//! Built on [`Receiver::read`] so any backend gets `wait`/`otp` for free. Time
//! is abstracted behind [`Clock`] so the baseline/backoff/timeout logic is
//! unit-testable without real sleeps.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::error::{AppError, Result};
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
        let mut messages = receiver.read(handle).await?;
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
            return receiver.get(handle, &id).await;
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
        // One Vec<Message> per successive read() call.
        reads: Mutex<std::collections::VecDeque<Vec<Message>>>,
    }
    #[async_trait]
    impl Receiver for FakeReceiver {
        async fn new_inbox(&self) -> Result<crate::model::InboxRecord> {
            unimplemented!()
        }
        async fn read(&self, _h: &Handle) -> Result<Vec<Message>> {
            let mut q = self.reads.lock().unwrap();
            Ok(q.pop_front().unwrap_or_default())
        }
        async fn get(&self, _h: &Handle, msg_id: &str) -> Result<Message> {
            Ok(msg(msg_id, "a@x", "s", false, "2026-06-27T11:00:00Z"))
        }
        async fn delete(&self, _h: &Handle) -> Result<()> {
            Ok(())
        }
    }

    fn handle() -> Handle {
        Handle {
            account_id: "acc".into(),
            address: "a@x.com".into(),
            password: "pw".into(),
            token: "tok".into(),
        }
    }

    #[tokio::test]
    async fn returns_message_once_a_new_one_arrives() {
        let reads = [
            vec![], // empty at start -> snapshot is empty
            vec![], // still nothing
            vec![msg("new", "a@x", "s", false, "2026-06-27T11:00:00Z")],
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
