//! Health introspection and secret redaction.
//!
//! A long-running connection needs a readable health surface — is it connected,
//! how stale is the last message, what is the clock offset and rate budget — and
//! it must never leak secret material into logs. [`Health`] is the status
//! snapshot; [`redact`] removes known secrets from a string before it is logged.

use serde::{Deserialize, Serialize};

/// A snapshot of a connection's health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Health {
    /// Whether the stream is currently connected.
    pub connected: bool,
    /// Timestamp (ms since the Unix epoch) of the last received message.
    pub last_message_ms: Option<i64>,
    /// The current server-clock offset in milliseconds.
    pub clock_offset_ms: i64,
    /// Weight charged against the rate budget in the current window.
    pub rate_budget_used: u32,
    /// How many times the stream has reconnected.
    pub reconnects: u64,
}

impl Health {
    /// Milliseconds since the last message at `now_ms`, or `None` if no message
    /// has been received yet.
    #[must_use]
    pub fn staleness_ms(&self, now_ms: i64) -> Option<i64> {
        self.last_message_ms.map(|last| now_ms - last)
    }

    /// Whether the connection is healthy: connected and a message arrived within
    /// `max_staleness_ms`.
    #[must_use]
    pub fn is_healthy(&self, now_ms: i64, max_staleness_ms: i64) -> bool {
        self.connected
            && self
                .staleness_ms(now_ms)
                .is_some_and(|s| s <= max_staleness_ms)
    }
}

/// Replace every occurrence of `secret` in `text` with `"<redacted>"`. An empty
/// secret is ignored (it would otherwise match everywhere), so this is safe to
/// call unconditionally before logging.
#[must_use]
pub fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "<redacted>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_and_health() {
        let mut health = Health {
            connected: true,
            last_message_ms: Some(1_000),
            ..Health::default()
        };
        assert_eq!(health.staleness_ms(1_500), Some(500));
        assert!(health.is_healthy(1_500, 1_000));
        assert!(!health.is_healthy(5_000, 1_000)); // too stale

        health.connected = false;
        assert!(!health.is_healthy(1_100, 1_000)); // disconnected
    }

    #[test]
    fn health_with_no_messages_is_never_healthy() {
        let health = Health {
            connected: true,
            last_message_ms: None,
            ..Health::default()
        };
        assert_eq!(health.staleness_ms(1_000), None);
        assert!(!health.is_healthy(1_000, 10_000));
    }

    #[test]
    fn redaction_removes_secrets_and_ignores_empty() {
        let line = "auth header X-KEY=SECRET123 sent";
        assert_eq!(
            redact(line, "SECRET123"),
            "auth header X-KEY=<redacted> sent"
        );
        // Multiple occurrences.
        assert_eq!(
            redact("aSECRETbSECRETc", "SECRET"),
            "a<redacted>b<redacted>c"
        );
        // Empty secret is a no-op (does not redact everything).
        assert_eq!(redact(line, ""), line);
    }
}
