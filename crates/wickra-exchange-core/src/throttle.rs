//! The sleep-and-retry loop that [`Backoff`] and [`WeightedRateLimiter`] were
//! written for.
//!
//! Both are pure policies: one decides how long to wait, the other decides
//! whether there is budget. Neither can act on its own, and until this existed
//! neither was called from anywhere outside its own file — `retry.rs` even says
//! "the actual sleep-and-retry loop lives in the real transport adapter", and
//! that adapter did not have one.
//!
//! [`ThrottledTransport`] is that loop, written as a decorator over any
//! [`HttpTransport`]. It sits *below* the venue clients, so it works for all ten
//! without any of them changing, and it stays testable: the clock, the sleep and
//! the jitter source are injected, so a test drives it with no real time passing.
//!
//! # What it retries, and what it deliberately does not
//!
//! Retrying a request that may already have been executed is how one order
//! becomes two. The rule is therefore not "retry what `Error::is_retryable`
//! allows":
//!
//! | outcome | GET | POST / PUT / PATCH / DELETE |
//! |---|---|---|
//! | HTTP 429 / 418 | retried | **retried** — the venue refused it, so nothing happened |
//! | timeout, network error | retried | **never** — the venue may have executed it |
//! | any other response | returned as-is | returned as-is |
//!
//! A timeout on an order placement is exactly the case where the caller cannot
//! know whether the order exists. This layer will not guess; it returns the
//! error and leaves the decision to code that can reconcile (see
//! [`crate::reconcile`]).
//!
//! # What it cannot see
//!
//! It observes raw responses, not the venue's mapped error. Binance, Gate,
//! Coinbase and Upbit signal a rate limit with an HTTP status, and those are
//! seen here. Bitget, Bybit, KuCoin, OKX, HTX and Kraken signal it with `200`
//! plus a code in their JSON envelope, which only the venue client can read —
//! for those, the reactive cool-off below does not engage, and the proactive
//! budget is what protects them.
//!
//! # What it says while doing it
//!
//! Everything above is invisible from outside: a caller that waited two seconds
//! cannot tell whether the budget held it, the venue refused it, or the network
//! dropped it, and those three call for different fixes. So each decision is
//! traced through the [`tracing`] facade, on the `wickra_exchange_core::throttle`
//! target:
//!
//! * `debug` — a budget wait, a venue cool-off, a retry and its delay
//! * `warn` — a repeat refused because the method is not safe to repeat, which
//!   is the case where a caller is left holding an order it cannot account for
//!
//! `tracing` costs a relaxed atomic load when no subscriber is installed, so a
//! consumer that wants none pays for none.

use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;
use crate::ratelimiter::{Acquire, WeightedRateLimiter};
use crate::retry::Backoff;
use crate::transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport};

/// HTTP statuses that mean "you are over the limit": 429 is standard, 418 is
/// Binance's escalation for an IP that ignored a 429.
const RATE_LIMIT_STATUSES: [u16; 2] = [429, 418];

/// The tracing target for this layer, so a consumer can filter it on its own.
///
/// Each call below is written on one line: `llvm-cov` attributes a multi-line
/// macro invocation with expression arguments across several lines and marks the
/// continuation lines uncovered even when the call runs.
///
/// The request URL is deliberately not logged. Every field here already
/// identifies the decision, and a signed URL carries its signature in the query
/// string -- a log line is the last place that should end up.
const TARGET: &str = "wickra_exchange_core::throttle";

/// The cool-off applied when a venue rate-limits without saying for how long.
const DEFAULT_COOL_OFF: Duration = Duration::from_secs(1);

/// A rate-limiting, retrying decorator over another [`HttpTransport`].
pub struct ThrottledTransport {
    inner: Box<dyn HttpTransport>,
    limiter: Option<Mutex<WeightedRateLimiter>>,
    backoff: Backoff,
    weigh: Box<dyn Fn(&HttpRequest) -> u32 + Send + Sync>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    jitter: Box<dyn Fn() -> f64 + Send + Sync>,
}

impl std::fmt::Debug for ThrottledTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThrottledTransport")
            .field("budget", &self.limiter.is_some())
            .field("backoff", &self.backoff)
            .finish_non_exhaustive()
    }
}

impl ThrottledTransport {
    /// Wrap `inner` with a retry policy and no request budget.
    ///
    /// No budget is the default because a wrong one is worse than none: a
    /// capacity invented for a venue that publishes a different number either
    /// throttles traffic the venue would have accepted, or fails to protect
    /// against the limit it was supposed to. The venue's own `Retry-After`
    /// still applies — that number comes from the venue rather than from us.
    pub fn new(inner: Box<dyn HttpTransport>, backoff: Backoff) -> Self {
        Self {
            inner,
            limiter: None,
            backoff,
            weigh: Box::new(|_| 1),
            now_ms: Box::new(system_now_ms),
            sleep: Box::new(std::thread::sleep),
            jitter: Box::new(clock_jitter),
        }
    }

    /// Charge every request against a windowed weight budget.
    #[must_use]
    pub fn with_budget(mut self, capacity: u32, window: Duration) -> Self {
        let window_ms = i64::try_from(window.as_millis()).unwrap_or(i64::MAX);
        self.limiter = Some(Mutex::new(WeightedRateLimiter::new(capacity, window_ms)));
        self
    }

    /// Weigh each request instead of charging one unit per call, for venues
    /// that meter a deep order-book read far above a ticker.
    #[must_use]
    pub fn with_weigher(
        mut self,
        weigh: impl Fn(&HttpRequest) -> u32 + Send + Sync + 'static,
    ) -> Self {
        self.weigh = Box::new(weigh);
        self
    }

    /// Replace the clock, the sleep and the jitter source.
    ///
    /// Used by tests to run the loop with no real time passing; production uses
    /// the defaults set by [`new`](Self::new).
    #[must_use]
    pub fn with_time(
        mut self,
        now_ms: impl Fn() -> i64 + Send + Sync + 'static,
        sleep: impl Fn(Duration) + Send + Sync + 'static,
        jitter: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        self.now_ms = Box::new(now_ms);
        self.sleep = Box::new(sleep);
        self.jitter = Box::new(jitter);
        self
    }

    /// Wait until the budget admits `request`, if a budget is configured.
    ///
    /// Bounded by the backoff's retry count so a misconfigured budget cannot
    /// park a caller forever.
    fn await_budget(&self, request: &HttpRequest) {
        let Some(limiter) = &self.limiter else {
            return;
        };
        let weight = (self.weigh)(request);
        for _ in 0..=self.backoff.max_retries() {
            let advice = {
                let mut limiter = limiter
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                limiter.try_acquire(weight, (self.now_ms)())
            };
            match advice {
                Acquire::Allowed => return,
                Acquire::Throttled { retry_after_ms } => {
                    tracing::debug!(target: TARGET, retry_after_ms, "budget exhausted; waiting");
                    (self.sleep)(millis(retry_after_ms));
                }
            }
        }
    }

    /// Record a venue cool-off so later requests wait for it too, not just this
    /// one.
    fn note_cool_off(&self, wait: Duration) {
        if let Some(limiter) = &self.limiter {
            let millis = i64::try_from(wait.as_millis()).unwrap_or(i64::MAX);
            let mut limiter = limiter
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            limiter.note_rate_limited(millis, (self.now_ms)());
        }
    }
}

/// Whether a failed request may be sent again.
///
/// A refusal is always safe to repeat: the venue rejected it before doing
/// anything. A timeout or a dropped connection is only safe to repeat when the
/// request could not have changed anything at the venue, which for this API
/// means a `GET`.
fn may_repeat(method: HttpMethod, refused: bool) -> bool {
    refused || matches!(method, HttpMethod::Get)
}

fn millis(value: i64) -> Duration {
    Duration::from_millis(u64::try_from(value).unwrap_or(0))
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A `[0, 1)` fraction taken from the clock's sub-millisecond digits.
///
/// Full jitter needs a spread, not cryptographic randomness, and this avoids a
/// random-number dependency in a crate that otherwise has none.
fn clock_jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

impl HttpTransport for ThrottledTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        let mut attempt = 0;
        loop {
            self.await_budget(request);

            let outcome = self.inner.execute(request);
            // `cool_off` is `Some` exactly when the venue refused the request
            // for rate reasons, which is also what makes a repeat safe on any
            // method.
            let cool_off = match &outcome {
                Ok(response) if RATE_LIMIT_STATUSES.contains(&response.status) => {
                    Some(response.retry_after().unwrap_or(DEFAULT_COOL_OFF))
                }
                Err(e) if e.is_retryable() => None,
                // Anything else -- a normal response, or a failure repeating
                // cannot help -- is the caller's to handle.
                _ => return outcome,
            };

            if let Some(wait) = cool_off {
                tracing::debug!(target: TARGET, wait_ms = wait.as_millis(), "venue rate-limited; cooling off");
                self.note_cool_off(wait);
            }

            if !may_repeat(request.method, cool_off.is_some()) {
                // The distinction that matters most in a log: this request may
                // already have been executed, so it is not repeated, and the
                // caller is left to reconcile.
                tracing::warn!(target: TARGET, method = ?request.method, "not repeating a request the venue may have executed; reconcile the order state");
                return outcome;
            }
            if !self.backoff.should_retry(attempt) {
                tracing::debug!(target: TARGET, attempt, "retry budget exhausted; returning the outcome");
                return outcome;
            }

            // A venue that named a wait is more specific than the policy's
            // curve, so its own advice wins on the attempt after it.
            let delay = cool_off.unwrap_or_else(|| {
                Duration::from_millis(self.backoff.jittered_delay_ms(attempt, (self.jitter)()))
            });
            tracing::debug!(target: TARGET, attempt, delay_ms = delay.as_millis(), venue_advised = cool_off.is_some(), "retrying after delay");
            (self.sleep)(delay);
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::transport::MockHttpTransport;
    use std::sync::Arc;

    /// Lets a test keep a handle on the mock while the decorator owns one.
    struct ArcTransport(Arc<MockHttpTransport>);
    impl HttpTransport for ArcTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.0.execute(request)
        }
    }

    /// A recording sleep: the loop is driven with no real time passing, and the
    /// test asserts on how long it *would* have waited.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Duration>>>);
    impl Recorder {
        fn waits(&self) -> Vec<Duration> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        fn total_ms(&self) -> u128 {
            self.waits().iter().map(Duration::as_millis).sum()
        }
    }

    /// A transport whose clock stands still, whose sleeps are recorded, and
    /// whose jitter is fixed so the backoff curve is exact.
    fn throttled(
        mock: &Arc<MockHttpTransport>,
        backoff: Backoff,
    ) -> (ThrottledTransport, Recorder) {
        let recorder = Recorder::default();
        let sink = recorder.clone();
        let transport = ThrottledTransport::new(Box::new(ArcTransport(Arc::clone(mock))), backoff)
            .with_time(
                || 0,
                move |d| {
                    sink.0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(d);
                },
                || 1.0,
            );
        (transport, recorder)
    }

    fn rate_limited(retry_after: Option<&str>) -> HttpResponse {
        let response = HttpResponse::new(429, r#"{"msg":"slow down"}"#);
        match retry_after {
            Some(value) => response.with_header("Retry-After", value),
            None => response,
        }
    }

    #[test]
    fn a_normal_response_passes_straight_through() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_json(200, r#"{"ok":true}"#);
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 3));

        let response = transport.execute(&HttpRequest::get("https://x/y")).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(mock.recorded_requests().len(), 1);
        assert!(recorder.waits().is_empty(), "nothing should have waited");
    }

    #[test]
    fn a_refusal_is_retried_and_waits_exactly_what_the_venue_advised() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_response(rate_limited(Some("2.5")));
        mock.push_json(200, r#"{"ok":true}"#);
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 3));

        let response = transport.execute(&HttpRequest::get("https://x/y")).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(mock.recorded_requests().len(), 2);
        // The venue's own number, not the policy curve, which would be 100ms.
        assert_eq!(recorder.waits(), vec![Duration::from_millis(2500)]);
    }

    #[test]
    fn a_refusal_without_a_stated_wait_falls_back_to_the_default_cool_off() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_response(rate_limited(None));
        mock.push_json(200, r#"{"ok":true}"#);
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 3));

        transport.execute(&HttpRequest::get("https://x/y")).unwrap();

        assert_eq!(recorder.waits(), vec![DEFAULT_COOL_OFF]);
    }

    #[test]
    fn an_order_placement_is_retried_after_a_refusal() {
        // A 429 means the venue rejected the request before acting on it, so
        // repeating it cannot place a second order.
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_response(rate_limited(Some("1")));
        mock.push_json(200, r#"{"orderId":"1"}"#);
        let (transport, _) = throttled(&mock, Backoff::new(100, 1_000, 3));

        let response = transport
            .execute(&HttpRequest::post("https://x/order"))
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(mock.recorded_requests().len(), 2);
    }

    #[test]
    fn an_order_placement_is_never_retried_after_a_timeout() {
        // The venue may have executed it. This is the case where a retry turns
        // one order into two, and no policy setting may enable it.
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_error(Error::Timeout);
        mock.push_json(200, r#"{"orderId":"2"}"#);
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 5));

        let error = transport
            .execute(&HttpRequest::post("https://x/order"))
            .unwrap_err();

        assert!(matches!(error, Error::Timeout));
        assert_eq!(mock.recorded_requests().len(), 1, "it must not be re-sent");
        assert!(recorder.waits().is_empty());
    }

    #[test]
    fn a_read_is_retried_after_a_timeout() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_error(Error::Timeout);
        mock.push_json(200, r#"{"ok":true}"#);
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 3));

        let response = transport.execute(&HttpRequest::get("https://x/y")).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(mock.recorded_requests().len(), 2);
        // Full jitter fixed at 1.0, so the first delay is the base.
        assert_eq!(recorder.waits(), vec![Duration::from_millis(100)]);
    }

    #[test]
    fn a_permanent_failure_is_returned_at_once() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_error(Error::Auth("bad signature".into()));
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 3));

        let error = transport
            .execute(&HttpRequest::get("https://x/y"))
            .unwrap_err();

        assert!(matches!(error, Error::Auth(_)));
        assert_eq!(mock.recorded_requests().len(), 1);
        assert!(recorder.waits().is_empty());
    }

    #[test]
    fn retries_stop_at_the_policy_limit_and_the_last_answer_is_returned() {
        let mock = Arc::new(MockHttpTransport::new());
        for _ in 0..6 {
            mock.push_response(rate_limited(Some("1")));
        }
        let (transport, recorder) = throttled(&mock, Backoff::new(100, 1_000, 2));

        let response = transport.execute(&HttpRequest::get("https://x/y")).unwrap();

        // Two retries after the first attempt: three calls, two waits.
        assert_eq!(response.status, 429);
        assert_eq!(mock.recorded_requests().len(), 3);
        assert_eq!(recorder.waits().len(), 2);
    }

    #[test]
    fn a_budget_throttles_once_its_capacity_is_spent() {
        let mock = Arc::new(MockHttpTransport::new());
        for _ in 0..3 {
            mock.push_json(200, "{}");
        }
        let recorder = Recorder::default();
        let sink = recorder.clone();
        // Capacity 2 per minute, on a clock that never advances: the third
        // request cannot fit and is made to wait out the window.
        let transport = ThrottledTransport::new(
            Box::new(ArcTransport(Arc::clone(&mock))),
            Backoff::new(10, 10, 4),
        )
        .with_budget(2, Duration::from_secs(60))
        .with_time(
            || 0,
            move |d| {
                sink.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(d);
            },
            || 1.0,
        );

        transport.execute(&HttpRequest::get("https://x/1")).unwrap();
        transport.execute(&HttpRequest::get("https://x/2")).unwrap();
        assert!(recorder.waits().is_empty(), "the budget covered both");

        transport.execute(&HttpRequest::get("https://x/3")).unwrap();
        assert_eq!(
            recorder.total_ms(),
            60_000 * 5,
            "waited out the window each try"
        );
    }

    #[test]
    fn a_weigher_spends_the_budget_faster_on_expensive_calls() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_json(200, "{}");
        mock.push_json(200, "{}");
        let recorder = Recorder::default();
        let sink = recorder.clone();
        let transport = ThrottledTransport::new(
            Box::new(ArcTransport(Arc::clone(&mock))),
            Backoff::new(10, 10, 1),
        )
        .with_budget(10, Duration::from_secs(60))
        // A deep book read costs what a venue says it costs.
        .with_weigher(|request| if request.url.contains("depth") { 10 } else { 1 })
        .with_time(
            || 0,
            move |d| {
                sink.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(d);
            },
            || 1.0,
        );

        transport
            .execute(&HttpRequest::get("https://x/depth"))
            .unwrap();
        assert!(recorder.waits().is_empty(), "the first fits exactly");

        transport
            .execute(&HttpRequest::get("https://x/ticker"))
            .unwrap();
        assert!(!recorder.waits().is_empty(), "the budget is spent");
    }

    #[test]
    fn a_venue_cool_off_also_holds_back_the_next_request() {
        let mock = Arc::new(MockHttpTransport::new());
        mock.push_response(rate_limited(Some("5")));
        mock.push_json(200, "{}");
        mock.push_json(200, "{}");
        let recorder = Recorder::default();
        let sink = recorder.clone();
        let transport = ThrottledTransport::new(
            Box::new(ArcTransport(Arc::clone(&mock))),
            Backoff::new(10, 10, 2),
        )
        .with_budget(100, Duration::from_secs(60))
        .with_time(
            || 0,
            move |d| {
                sink.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(d);
            },
            || 1.0,
        );

        transport.execute(&HttpRequest::get("https://x/1")).unwrap();
        let after_first = recorder.waits().len();

        // The cool-off was recorded against the limiter, not just slept off
        // once, so a later call on the same frozen clock waits for it too.
        transport.execute(&HttpRequest::get("https://x/2")).unwrap();
        assert!(recorder.waits().len() > after_first);
    }

    #[test]
    fn only_a_refusal_makes_a_write_repeatable() {
        assert!(may_repeat(HttpMethod::Get, false));
        assert!(may_repeat(HttpMethod::Get, true));
        assert!(may_repeat(HttpMethod::Post, true));
        assert!(!may_repeat(HttpMethod::Post, false));
        assert!(!may_repeat(HttpMethod::Delete, false));
        assert!(!may_repeat(HttpMethod::Put, false));
        assert!(!may_repeat(HttpMethod::Patch, false));
    }

    #[test]
    fn debug_reports_whether_a_budget_is_configured() {
        let mock = Arc::new(MockHttpTransport::new());
        let plain = ThrottledTransport::new(
            Box::new(ArcTransport(Arc::clone(&mock))),
            Backoff::new(10, 10, 1),
        );
        assert!(format!("{plain:?}").contains("budget: false"));
        let budgeted = plain.with_budget(10, Duration::from_secs(1));
        assert!(format!("{budgeted:?}").contains("budget: true"));
    }
}
