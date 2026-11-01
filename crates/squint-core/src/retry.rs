//! Transport retries (SPEC "Retries"): connection errors, timeouts before any byte, HTTP 408, 409,
//! 429 and 5xx are retried with exponential backoff and full jitter from 1 s, capped at 30 s,
//! honouring `retry-after`, at most `budget.retries` times after the first attempt. Any other
//! failure is `error: provider` at once. Adapters never retry; the state machine owns it.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::provider::{Provider, TurnRequest, TurnResponse};
use crate::{Error, Result};

/// A transport-level failure an adapter reports instead of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    /// The HTTP status, `None` for a connection error or a timeout before any byte.
    pub status: Option<u16>,
    /// The server's `retry-after`, when present.
    pub retry_after: Option<Duration>,
    pub message: String,
    /// Whether the request left this process: false when the connection itself failed, so the
    /// provider never saw it (table-manners 2.3: not sent, costs nothing).
    pub sent: bool,
}

impl TransportError {
    /// Whether no answer settles what this call cost, so it counts at its worst case
    /// (table-manners 2.3): it was sent and the answer, if any, reported no usage. A call that
    /// never left, and an authentication rejection (refused before any work), cost nothing.
    pub fn unresolved(&self) -> bool {
        self.sent && !matches!(self.status, Some(401 | 403))
    }

    /// Whether the spec allows a retry of this failure.
    pub fn retryable(&self) -> bool {
        match self.status {
            None => true,
            Some(s) => matches!(s, 408 | 409 | 429) || (500..600).contains(&s),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(s) => write!(f, "HTTP {s}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for TransportError {}

/// The backoff schedule: full jitter over `base × 2^(attempt-1)`, capped.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// The wait before retry number `attempt` (1-based). `retry_after` wins when present; otherwise
    /// `unit` in `[0, 1)` scales the ceiling (full jitter). Both are capped.
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>, unit: f64) -> Duration {
        if let Some(ra) = retry_after {
            return ra.min(self.cap);
        }
        let exponent = attempt.saturating_sub(1).min(16);
        let ceiling = self.base.saturating_mul(1u32 << exponent).min(self.cap);
        ceiling.mul_f64(unit.clamp(0.0, 1.0))
    }
}

/// How the harness waits; the real clock in production, a recorder in tests.
pub trait Sleeper: Send + Sync {
    fn sleep<'a>(&'a self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// The real clock.
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep<'a>(&'a self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(tokio::time::sleep(d))
    }
}

/// One retry, for the journal and the ledger.
#[derive(Debug, Clone)]
pub struct Attempt {
    /// 1 for the first retry.
    pub attempt: u32,
    pub status: Option<u16>,
    pub delay: Duration,
    pub message: String,
    /// Whether the failed attempt counts at its worst case ([`TransportError::unresolved`]).
    pub unresolved: bool,
}

/// A response and how many billable attempts it took.
pub struct Sent {
    pub response: TurnResponse,
    pub attempts: u32,
}

/// Sends a request, retrying transport failures. `on_retry` runs before each wait so the journal
/// holds the attempt even if the process dies while sleeping.
pub async fn send_with_retries<'a>(
    provider: &'a dyn Provider,
    request: TurnRequest<'a>,
    retries: u32,
    backoff: &Backoff,
    sleeper: &dyn Sleeper,
    jitter: &mut dyn FnMut() -> f64,
    on_retry: &mut dyn FnMut(&Attempt) -> Result<()>,
) -> Result<Sent> {
    let mut attempt: u32 = 0;
    loop {
        match provider.send(request.clone()).await {
            Ok(response) => {
                return Ok(Sent {
                    response,
                    attempts: attempt + 1,
                });
            }
            Err(Error::Transport(t)) if t.retryable() && attempt < retries => {
                attempt += 1;
                let delay = backoff.delay(attempt, t.retry_after, jitter());
                on_retry(&Attempt {
                    attempt,
                    status: t.status,
                    delay,
                    message: t.message.clone(),
                    unresolved: t.unresolved(),
                })?;
                sleeper.sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{SendFuture, StopReason};
    use std::sync::Mutex;

    struct Scripted(Mutex<std::collections::VecDeque<Result<TurnResponse>>>);

    impl Provider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }
        fn send<'a>(&'a self, _: TurnRequest<'a>) -> SendFuture<'a> {
            Box::pin(async move {
                self.0
                    .lock()
                    .ok()
                    .and_then(|mut q| q.pop_front())
                    .unwrap_or_else(|| Err(Error::Provider("script exhausted".into())))
            })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Duration>>);

    impl Sleeper for Recorder {
        fn sleep<'a>(&'a self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            if let Ok(mut v) = self.0.lock() {
                v.push(d);
            }
            Box::pin(async {})
        }
    }

    fn ok() -> Result<TurnResponse> {
        Ok(TurnResponse {
            text: vec![],
            tool_calls: vec![],
            content: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Default::default(),
            provider_request_id: None,
        })
    }

    fn transport(status: Option<u16>, retry_after: Option<u64>) -> Result<TurnResponse> {
        Err(Error::Transport(TransportError {
            status,
            retry_after: retry_after.map(Duration::from_secs),
            message: "scripted".into(),
            sent: true,
        }))
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    fn run(
        script: Vec<Result<TurnResponse>>,
        retries: u32,
    ) -> (Result<Sent>, Vec<Duration>, Vec<Attempt>) {
        let provider = Scripted(Mutex::new(script.into()));
        let sleeper = Recorder::default();
        let mut seen = Vec::new();
        let images = std::collections::BTreeMap::new();
        let request = TurnRequest {
            system: "",
            history: &[],
            tools: &[],
            max_output_tokens: 1,
            images: &images,
        };
        let result = block_on(send_with_retries(
            &provider,
            request,
            retries,
            &Backoff::default(),
            &sleeper,
            &mut || 0.5,
            &mut |a| {
                seen.push(a.clone());
                Ok(())
            },
        ));
        let slept = sleeper.0.lock().map(|v| v.clone()).unwrap_or_default();
        (result, slept, seen)
    }

    #[test]
    fn retry_waits_the_retry_after_on_a_429() {
        let (result, slept, seen) = run(vec![transport(Some(429), Some(2)), ok()], 3);
        let sent = result.expect("second attempt succeeds");
        assert_eq!(sent.attempts, 2);
        assert_eq!(slept, [Duration::from_secs(2)]);
        assert_eq!(seen[0].status, Some(429));
    }

    #[test]
    fn retry_stops_after_the_budget_with_jittered_backoff() {
        let script = (0..4).map(|_| transport(Some(503), None)).collect();
        let (result, slept, seen) = run(script, 3);
        assert!(matches!(result, Err(Error::Transport(t)) if t.status == Some(503)));
        assert_eq!(seen.len(), 3);
        assert_eq!(
            slept,
            [
                Duration::from_millis(500),
                Duration::from_millis(1000),
                Duration::from_millis(2000)
            ],
            "half of 1 s, 2 s, 4 s"
        );
    }

    #[test]
    fn retry_never_happens_on_a_400() {
        let (result, slept, seen) = run(vec![transport(Some(400), None), ok()], 3);
        assert!(matches!(result, Err(Error::Transport(t)) if t.status == Some(400)));
        assert!(slept.is_empty() && seen.is_empty());
    }

    #[test]
    fn retry_classifies_statuses_as_the_spec_lists_them() {
        let t = |status| TransportError {
            status,
            retry_after: None,
            message: String::new(),
            sent: true,
        };
        for s in [408, 409, 429, 500, 502, 503, 599] {
            assert!(t(Some(s)).retryable(), "{s}");
        }
        for s in [400, 401, 403, 404, 422] {
            assert!(!t(Some(s)).retryable(), "{s}");
        }
        assert!(t(None).retryable());
    }

    #[test]
    fn retry_covers_connection_errors_and_timeouts() {
        let (result, slept, _) = run(vec![transport(None, None), ok()], 1);
        assert!(result.is_ok());
        assert_eq!(slept.len(), 1);
    }

    #[test]
    fn retry_delay_is_capped_at_thirty_seconds() {
        let b = Backoff::default();
        assert_eq!(b.delay(10, None, 1.0), Duration::from_secs(30));
        assert_eq!(
            b.delay(1, Some(Duration::from_secs(120)), 0.0),
            Duration::from_secs(30)
        );
        assert_eq!(
            b.delay(3, None, 0.25),
            Duration::from_secs(1),
            "a quarter of 4 s"
        );
    }

    #[test]
    fn transport_error_display_and_class() {
        let e = Error::Transport(TransportError {
            status: Some(429),
            retry_after: None,
            message: "slow down".into(),
            sent: true,
        });
        assert_eq!(e.to_string(), "provider: HTTP 429: slow down");
        assert_eq!(e.class(), "provider");
    }
}
