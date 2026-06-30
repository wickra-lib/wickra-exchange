//! Weight-based rate limiting.
//!
//! Venues meter requests by **weight** per time window, not a flat count: a
//! `GET /depth?limit=1000` costs far more than a ticker. [`WeightedRateLimiter`]
//! is a windowed weight budget that also honours an explicit cool-off when the
//! venue returns `429`/`418` with a `Retry-After`. Like the rest of the crate it
//! takes `now_ms` as an argument, so it is deterministic under test.

/// The result of asking the limiter for budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquire {
    /// The request fits within budget; the weight has been charged.
    Allowed,
    /// The request must wait; `retry_after_ms` is the advised delay.
    Throttled {
        /// Milliseconds to wait before retrying.
        retry_after_ms: i64,
    },
}

/// A windowed, weight-based rate limiter with venue cool-off support.
#[derive(Debug)]
pub struct WeightedRateLimiter {
    capacity: u32,
    window_ms: i64,
    used: u32,
    window_start_ms: Option<i64>,
    banned_until_ms: Option<i64>,
}

impl WeightedRateLimiter {
    /// A limiter allowing `capacity` units of weight per `window_ms`.
    #[must_use]
    pub fn new(capacity: u32, window_ms: i64) -> Self {
        Self {
            capacity,
            window_ms,
            used: 0,
            window_start_ms: None,
            banned_until_ms: None,
        }
    }

    /// The weight charged in the current window.
    #[must_use]
    pub fn used(&self) -> u32 {
        self.used
    }

    /// Record that the venue rate-limited us (`429`/`418`): refuse all requests
    /// until `now_ms + retry_after_ms`.
    pub fn note_rate_limited(&mut self, retry_after_ms: i64, now_ms: i64) {
        self.banned_until_ms = Some(now_ms + retry_after_ms);
    }

    /// Try to charge `weight` against the budget at `now_ms`.
    pub fn try_acquire(&mut self, weight: u32, now_ms: i64) -> Acquire {
        // Honour an explicit venue cool-off first.
        if let Some(until) = self.banned_until_ms {
            if now_ms < until {
                return Acquire::Throttled {
                    retry_after_ms: until - now_ms,
                };
            }
            self.banned_until_ms = None;
        }

        // Roll the window if it has elapsed (or start it on first use).
        let start = match self.window_start_ms {
            Some(start) if now_ms < start + self.window_ms => start,
            _ => {
                self.window_start_ms = Some(now_ms);
                self.used = 0;
                now_ms
            }
        };

        if self.used + weight > self.capacity {
            return Acquire::Throttled {
                retry_after_ms: start + self.window_ms - now_ms,
            };
        }
        self.used += weight;
        Acquire::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_weight_until_the_budget_is_spent() {
        let mut rl = WeightedRateLimiter::new(100, 60_000);
        assert_eq!(rl.try_acquire(40, 1_000), Acquire::Allowed);
        assert_eq!(rl.used(), 40);
        assert_eq!(rl.try_acquire(40, 1_500), Acquire::Allowed);
        assert_eq!(rl.used(), 80);
        // 80 + 30 > 100 -> throttled, advised wait is to the window end.
        assert_eq!(
            rl.try_acquire(30, 2_000),
            Acquire::Throttled {
                retry_after_ms: 1_000 + 60_000 - 2_000
            }
        );
    }

    #[test]
    fn window_rolls_and_resets_budget() {
        let mut rl = WeightedRateLimiter::new(100, 60_000);
        assert_eq!(rl.try_acquire(100, 0), Acquire::Allowed);
        // Still inside the window: full.
        assert!(matches!(
            rl.try_acquire(1, 30_000),
            Acquire::Throttled { .. }
        ));
        // After the window: budget resets.
        assert_eq!(rl.try_acquire(50, 60_000), Acquire::Allowed);
        assert_eq!(rl.used(), 50);
    }

    #[test]
    fn venue_cool_off_blocks_until_it_expires() {
        let mut rl = WeightedRateLimiter::new(100, 60_000);
        rl.note_rate_limited(5_000, 1_000);
        assert_eq!(
            rl.try_acquire(1, 2_000),
            Acquire::Throttled {
                retry_after_ms: 4_000
            }
        );
        // After the cool-off, requests flow again.
        assert_eq!(rl.try_acquire(1, 6_000), Acquire::Allowed);
    }
}
