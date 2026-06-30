//! Exponential backoff policy for transient failures.
//!
//! Transient failures (5xx, network blips, rate limits) are worth retrying with
//! an exponentially growing, capped delay and jitter to avoid thundering herds.
//! [`Backoff`] is the *policy* — it computes delays and the retry decision; the
//! actual sleep-and-retry loop lives in the real transport adapter and uses this
//! policy. Jitter takes a `[0, 1)` fraction as an argument so the policy stays
//! pure and testable (no hidden RNG).

/// An exponential-backoff policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    base_ms: u64,
    max_ms: u64,
    max_retries: u32,
}

impl Backoff {
    /// A policy starting at `base_ms`, doubling each attempt, capped at `max_ms`,
    /// giving up after `max_retries`.
    #[must_use]
    pub fn new(base_ms: u64, max_ms: u64, max_retries: u32) -> Self {
        Self {
            base_ms,
            max_ms,
            max_retries,
        }
    }

    /// The (un-jittered) delay before retry `attempt` (0-based): `base * 2^attempt`,
    /// saturating and capped at `max_ms`.
    #[must_use]
    pub fn delay_ms(&self, attempt: u32) -> u64 {
        let factor = 2u64.saturating_pow(attempt);
        self.base_ms.saturating_mul(factor).min(self.max_ms)
    }

    /// The full-jitter delay for `attempt`: a uniform sample in `[0, delay]`,
    /// given a random fraction `rand01` in `[0, 1)`.
    #[must_use]
    pub fn jittered_delay_ms(&self, attempt: u32, rand01: f64) -> u64 {
        let clamped = rand01.clamp(0.0, 1.0);
        #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
        let jittered = self.delay_ms(attempt) as f64 * clamped;
        jittered as u64
    }

    /// Whether an `attempt` (0-based) should be retried.
    #[must_use]
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }

    /// The configured maximum number of retries.
    #[must_use]
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_and_caps() {
        let b = Backoff::new(100, 2_000, 5);
        assert_eq!(b.delay_ms(0), 100);
        assert_eq!(b.delay_ms(1), 200);
        assert_eq!(b.delay_ms(2), 400);
        assert_eq!(b.delay_ms(3), 800);
        assert_eq!(b.delay_ms(4), 1_600);
        // Capped.
        assert_eq!(b.delay_ms(5), 2_000);
        assert_eq!(b.delay_ms(60), 2_000);
    }

    #[test]
    fn jitter_scales_within_the_delay() {
        let b = Backoff::new(100, 10_000, 5);
        assert_eq!(b.jittered_delay_ms(2, 0.0), 0);
        assert_eq!(b.jittered_delay_ms(2, 0.5), 200); // 400 * 0.5
        assert_eq!(b.jittered_delay_ms(2, 1.0), 400);
        // Out-of-range fractions are clamped.
        assert_eq!(b.jittered_delay_ms(2, -1.0), 0);
        assert_eq!(b.jittered_delay_ms(2, 2.0), 400);
    }

    #[test]
    fn retry_decision_respects_the_limit() {
        let b = Backoff::new(100, 1_000, 3);
        assert_eq!(b.max_retries(), 3);
        assert!(b.should_retry(0));
        assert!(b.should_retry(2));
        assert!(!b.should_retry(3));
        assert!(!b.should_retry(10));
    }
}
