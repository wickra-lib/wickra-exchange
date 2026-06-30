//! Time and nonce correctness for signed requests.
//!
//! Signed requests carry a timestamp that the venue checks against its own clock
//! within a receive window; local clock drift silently gets orders rejected.
//! [`ServerClock`] tracks the offset against the venue's `/time` endpoint and
//! produces adjusted timestamps. [`NonceGenerator`] yields the strictly
//! increasing nonce some venues (Kraken) require, and [`TokenTtl`] tracks the
//! lifetime of a per-request JWT (Coinbase/Upbit).
//!
//! Every method that needs "now" takes it as an argument, so the module has no
//! hidden wall-clock and is fully deterministic under test.

use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks the offset between the local clock and a venue's server clock.
#[derive(Debug, Default)]
pub struct ServerClock {
    offset_ms: i64,
}

impl ServerClock {
    /// A clock with zero offset (assumes local time until synced).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a sync point: the local time at which the venue reported
    /// `server_ms`. The offset becomes `server_ms - local_ms`.
    pub fn sync(&mut self, local_ms: i64, server_ms: i64) {
        self.offset_ms = server_ms - local_ms;
    }

    /// The current offset (`server - local`) in milliseconds.
    #[must_use]
    pub fn offset_ms(&self) -> i64 {
        self.offset_ms
    }

    /// The server-adjusted timestamp for a given local time.
    #[must_use]
    pub fn server_time_ms(&self, local_ms: i64) -> i64 {
        local_ms + self.offset_ms
    }
}

/// A strictly-increasing nonce generator. The nonce tracks wall-clock
/// milliseconds but is forced monotonic, so calls faster than millisecond
/// resolution still strictly increase.
#[derive(Debug)]
pub struct NonceGenerator {
    last: AtomicU64,
}

impl NonceGenerator {
    /// A generator that will produce values strictly greater than `start`.
    #[must_use]
    pub fn new(start: u64) -> Self {
        Self {
            last: AtomicU64::new(start),
        }
    }

    /// The next nonce: the greater of `candidate_ms` and `last + 1`. Thread-safe.
    pub fn next(&self, candidate_ms: u64) -> u64 {
        let mut prev = self.last.load(Ordering::Relaxed);
        loop {
            let next = candidate_ms.max(prev + 1);
            match self
                .last
                .compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(actual) => prev = actual,
            }
        }
    }
}

/// The lifetime of a short-lived token (a per-request JWT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenTtl {
    issued_at_ms: i64,
    ttl_ms: i64,
}

impl TokenTtl {
    /// A token issued at `issued_at_ms`, valid for `ttl_ms`.
    #[must_use]
    pub fn new(issued_at_ms: i64, ttl_ms: i64) -> Self {
        Self {
            issued_at_ms,
            ttl_ms,
        }
    }

    /// The absolute expiry time in milliseconds.
    #[must_use]
    pub fn expires_at_ms(&self) -> i64 {
        self.issued_at_ms + self.ttl_ms
    }

    /// Whether the token is expired at `now_ms`.
    #[must_use]
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_at_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_clock_offsets_both_directions() {
        let mut clock = ServerClock::new();
        assert_eq!(clock.offset_ms(), 0);

        // Server is 250 ms ahead of local.
        clock.sync(1_000, 1_250);
        assert_eq!(clock.offset_ms(), 250);
        assert_eq!(clock.server_time_ms(2_000), 2_250);

        // Server is behind local.
        clock.sync(5_000, 4_900);
        assert_eq!(clock.offset_ms(), -100);
        assert_eq!(clock.server_time_ms(5_000), 4_900);
    }

    #[test]
    fn nonce_is_strictly_increasing_even_within_a_millisecond() {
        let gen = NonceGenerator::new(0);
        // Same candidate repeated: still strictly increases.
        assert_eq!(gen.next(1_000), 1_000);
        assert_eq!(gen.next(1_000), 1_001);
        assert_eq!(gen.next(1_000), 1_002);
        // A jump forward is honored.
        assert_eq!(gen.next(5_000), 5_000);
        // A candidate below the last is still pushed above it.
        assert_eq!(gen.next(10), 5_001);
    }

    #[test]
    fn nonce_starts_above_seed() {
        let gen = NonceGenerator::new(100);
        assert_eq!(gen.next(0), 101);
    }

    #[test]
    fn token_ttl_expiry() {
        let token = TokenTtl::new(1_000, 30_000);
        assert_eq!(token.expires_at_ms(), 31_000);
        assert!(!token.is_expired(30_999));
        assert!(token.is_expired(31_000));
        assert!(token.is_expired(40_000));
    }
}
