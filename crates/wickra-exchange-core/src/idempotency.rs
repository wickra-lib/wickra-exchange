//! Client order ids for idempotent placement.
//!
//! Attaching a client-supplied id to an order makes placement safe to retry: if
//! a network blip hides the response to a placed order, re-sending the same
//! client id lets the venue deduplicate instead of double-filling.
//!
//! [`ClientIdGenerator`] produces unique ids from a fixed prefix and a monotonic
//! counter — deterministic given its seed, so it is testable and reproducible,
//! with no hidden randomness or clock.

use std::sync::atomic::{AtomicU64, Ordering};

/// A monotonic generator of unique client order ids of the form
/// `"{prefix}-{counter}"`.
#[derive(Debug)]
pub struct ClientIdGenerator {
    prefix: String,
    counter: AtomicU64,
}

impl ClientIdGenerator {
    /// A generator with the given id prefix, starting its counter at zero.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            counter: AtomicU64::new(0),
        }
    }

    /// A generator whose counter starts at `seed` (for reproducible sequences).
    pub fn with_seed(prefix: impl Into<String>, seed: u64) -> Self {
        Self {
            prefix: prefix.into(),
            counter: AtomicU64::new(seed),
        }
    }

    /// The next unique id. Thread-safe and lock-free.
    pub fn next_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}-{n}", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_carry_the_prefix_and_increment() {
        let gen = ClientIdGenerator::new("wkex");
        assert_eq!(gen.next_id(), "wkex-0");
        assert_eq!(gen.next_id(), "wkex-1");
        assert_eq!(gen.next_id(), "wkex-2");
    }

    #[test]
    fn seed_makes_the_sequence_reproducible() {
        let gen = ClientIdGenerator::with_seed("x", 100);
        assert_eq!(gen.next_id(), "x-100");
        assert_eq!(gen.next_id(), "x-101");
    }

    #[test]
    fn ids_are_unique_over_a_run() {
        let gen = ClientIdGenerator::new("p");
        let ids: HashSet<String> = (0..1000).map(|_| gen.next_id()).collect();
        assert_eq!(ids.len(), 1000);
    }
}
