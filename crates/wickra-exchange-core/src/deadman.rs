//! Dead-man's-switch (cancel-on-disconnect) — a live-trading safety primitive.
//!
//! When a trading loop loses contact with a venue, any resting orders keep
//! working unattended. A dead-man's-switch guards against that: arm it and feed
//! it a heartbeat on every successful exchange message (or WebSocket ping); if
//! the deadline passes without one, [`DeadMansSwitch::is_expired`] fires and the
//! caller cancels every resting order (via the venue's cancel-all endpoint, or
//! [`PaperExchange::cancel_all`](crate::PaperExchange::cancel_all) in simulation).
//!
//! Every method takes an explicit millisecond timestamp, so the switch is fully
//! deterministic and testable with no hidden clock.

use std::time::Duration;

/// A heartbeat-driven deadline. Starts disarmed; the first [`heartbeat`] arms it.
///
/// [`heartbeat`]: DeadMansSwitch::heartbeat
#[derive(Debug, Clone)]
pub struct DeadMansSwitch {
    timeout_ms: i64,
    deadline_ms: Option<i64>,
}

impl DeadMansSwitch {
    /// A switch that trips `timeout` after the most recent heartbeat.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout_ms: timeout.as_millis() as i64,
            deadline_ms: None,
        }
    }

    /// Record a heartbeat at `now_ms`, arming (or extending) the deadline.
    pub fn heartbeat(&mut self, now_ms: i64) {
        self.deadline_ms = Some(now_ms + self.timeout_ms);
    }

    /// Whether the deadline has passed without a heartbeat. Always `false` while
    /// the switch is disarmed.
    #[must_use]
    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.deadline_ms.is_some_and(|deadline| now_ms >= deadline)
    }

    /// Whether the switch is armed (has seen a heartbeat and not been disarmed).
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.deadline_ms.is_some()
    }

    /// The current deadline timestamp, or `None` while disarmed.
    #[must_use]
    pub fn deadline_ms(&self) -> Option<i64> {
        self.deadline_ms
    }

    /// Disarm the switch (e.g. after a clean shutdown or once cancel-all ran).
    pub fn disarm(&mut self) {
        self.deadline_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_disarmed_and_never_expires() {
        let switch = DeadMansSwitch::new(Duration::from_secs(5));
        assert!(!switch.is_armed());
        assert!(!switch.is_expired(1_000_000));
        assert_eq!(switch.deadline_ms(), None);
    }

    #[test]
    fn heartbeat_arms_and_sets_the_deadline() {
        let mut switch = DeadMansSwitch::new(Duration::from_secs(5));
        switch.heartbeat(1_000);
        assert!(switch.is_armed());
        assert_eq!(switch.deadline_ms(), Some(6_000));
        // Before the deadline: alive; at/after it: expired.
        assert!(!switch.is_expired(5_999));
        assert!(switch.is_expired(6_000));
        assert!(switch.is_expired(10_000));
    }

    #[test]
    fn a_fresh_heartbeat_extends_the_deadline() {
        let mut switch = DeadMansSwitch::new(Duration::from_secs(5));
        switch.heartbeat(1_000);
        assert!(switch.is_expired(6_000));
        switch.heartbeat(5_000); // renew before it would have tripped-relative
        assert_eq!(switch.deadline_ms(), Some(10_000));
        assert!(!switch.is_expired(6_000));
        assert!(switch.is_expired(10_000));
    }

    #[test]
    fn disarm_clears_the_deadline() {
        let mut switch = DeadMansSwitch::new(Duration::from_millis(100));
        switch.heartbeat(0);
        assert!(switch.is_expired(200));
        switch.disarm();
        assert!(!switch.is_armed());
        assert!(!switch.is_expired(1_000_000));
    }
}
