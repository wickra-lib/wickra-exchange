//! The unified error taxonomy.
//!
//! Every exchange maps its own numeric/string error codes onto this single enum,
//! so a consumer can react to a class of failure (`RateLimited`, `InsufficientBalance`,
//! `InvalidSymbol`, …) without knowing which venue produced it. Variants that
//! originate at a venue keep the raw `code`/`message` for diagnostics.

use std::time::Duration;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// A unified connectivity / execution error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No exchange is registered under the given name.
    #[error("unsupported exchange: {0}")]
    UnsupportedExchange(String),

    /// A symbol string could not be parsed or is not listed on the venue.
    #[error("invalid symbol: {0}")]
    InvalidSymbol(String),

    /// Credentials are missing a field the venue requires (e.g. a passphrase).
    #[error("invalid credentials: {0}")]
    InvalidCredentials(&'static str),

    /// An order request is malformed before it is ever sent.
    #[error("invalid order: {0}")]
    InvalidOrder(&'static str),

    /// An order would violate one of the venue's symbol filters
    /// (lot size, price tick, min-notional, …).
    #[error("order violates exchange filter: {0}")]
    Filter(String),

    /// Request signing or authentication was rejected by the venue.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The account has insufficient balance for the request.
    #[error("insufficient balance")]
    InsufficientBalance,

    /// The venue rate-limited the request; `retry_after` is the advised wait if
    /// the venue supplied one (`Retry-After`).
    #[error("rate limited{}", .retry_after.map(|d| format!(" (retry after {:.1}s)", d.as_secs_f64())).unwrap_or_default())]
    RateLimited {
        /// The advised back-off, if the venue provided one.
        retry_after: Option<Duration>,
    },

    /// The venue rejected the order after it was accepted for processing.
    #[error("order rejected by exchange (code {code}): {message}")]
    OrderRejected {
        /// The venue's error code, verbatim.
        code: String,
        /// The venue's human-readable message.
        message: String,
    },

    /// Any other error reported by the venue, kept verbatim for diagnostics.
    #[error("exchange error (code {code}): {message}")]
    Exchange {
        /// The venue's error code, verbatim.
        code: String,
        /// The venue's human-readable message.
        message: String,
    },

    /// A requested entity (order, symbol metadata, …) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// The request timed out before a response was received.
    #[error("request timed out")]
    Timeout,

    /// A stream or client operation was attempted before connecting.
    #[error("not connected")]
    NotConnected,

    /// A transport-level (socket/TLS/DNS) failure.
    #[error("network error: {0}")]
    Network(String),

    /// A response could not be parsed into the expected shape.
    #[error("deserialization error: {0}")]
    Deserialization(String),
}

impl Error {
    /// A trigger order on a venue path that cannot carry the trigger price.
    ///
    /// Placing it anyway is not a smaller version of the order the caller
    /// asked for -- a stop-loss with no trigger executes at once, at the price
    /// it was meant to protect against. So the paths that cannot express the
    /// trigger refuse, and this is the refusal.
    pub(crate) fn unsupported_trigger(venue: &str) -> Self {
        Error::Exchange {
            code: "unsupported".to_string(),
            message: format!(
                "{venue}: trigger (stop) orders are not implemented on this path;                  sending one without its trigger price would execute immediately                  instead of resting"
            ),
        }
    }

    /// An order field the caller set that this venue path cannot express.
    ///
    /// The same reasoning as [`Error::unsupported_trigger`], generalised to the
    /// rest of the request. A field the caller set is part of what the order
    /// *is*: an `Ioc` that is sent as a `Gtc` rests in the book the caller
    /// asked it never to rest in, and a `post_only` that is dropped can take
    /// liquidity at the taker fee. Sending the order without the field is not
    /// a smaller version of it, so the paths that cannot express a field refuse
    /// instead of quietly weakening the order.
    pub(crate) fn unsupported_field(venue: &str, field: &str, detail: &str) -> Self {
        Error::Exchange {
            code: "unsupported".to_string(),
            message: format!(
                "{venue}: {field} cannot be expressed on this order path ({detail});                  sending the order without it would place a different order than requested"
            ),
        }
    }

    /// Whether retrying the operation could plausibly succeed: transient
    /// transport failures, timeouts and rate limits. Permanent failures
    /// (invalid order, auth, insufficient balance, …) return `false`.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::RateLimited { .. } | Error::Timeout | Error::Network(_)
        )
    }

    /// The venue-supplied error code, if this error originated at an exchange.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::OrderRejected { code, .. } | Error::Exchange { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Attach the venue's advised wait to a rate-limit error that was raised
    /// without one.
    ///
    /// The venue clients recognise a rate limit from the *body* -- a numeric
    /// code in the error envelope -- while the wait arrives in the
    /// `Retry-After` *header*. The two are read in different places, so this
    /// joins them at the call site that has both.
    ///
    /// Every other variant is returned untouched, and a `RateLimited` that
    /// already carries a wait keeps it: a venue that states the delay in its
    /// body is more specific than the header, and this must not overwrite it.
    #[must_use]
    pub fn with_retry_after(self, retry_after: Option<Duration>) -> Self {
        match self {
            Error::RateLimited { retry_after: None } => Error::RateLimited { retry_after },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_classification() {
        assert!(Error::Timeout.is_retryable());
        assert!(Error::Network("reset".into()).is_retryable());
        assert!(Error::RateLimited { retry_after: None }.is_retryable());
        assert!(!Error::InsufficientBalance.is_retryable());
        assert!(!Error::InvalidOrder("no price").is_retryable());
        assert!(!Error::Auth("bad signature".into()).is_retryable());
    }

    #[test]
    fn code_is_exposed_only_for_venue_errors() {
        let rejected = Error::OrderRejected {
            code: "-2010".into(),
            message: "insufficient balance".into(),
        };
        assert_eq!(rejected.code(), Some("-2010"));
        let other = Error::Exchange {
            code: "1021".into(),
            message: "timestamp outside recvWindow".into(),
        };
        assert_eq!(other.code(), Some("1021"));
        assert_eq!(Error::Timeout.code(), None);
        assert_eq!(Error::InvalidSymbol("??".into()).code(), None);
    }

    #[test]
    fn display_renders_each_variant() {
        assert_eq!(
            Error::UnsupportedExchange("foo".into()).to_string(),
            "unsupported exchange: foo"
        );
        assert_eq!(
            Error::InvalidSymbol("BTC".into()).to_string(),
            "invalid symbol: BTC"
        );
        assert_eq!(
            Error::InvalidCredentials("passphrase required").to_string(),
            "invalid credentials: passphrase required"
        );
        assert_eq!(
            Error::InvalidOrder("limit order needs a price").to_string(),
            "invalid order: limit order needs a price"
        );
        assert_eq!(
            Error::Filter("LOT_SIZE: step 0.001".into()).to_string(),
            "order violates exchange filter: LOT_SIZE: step 0.001"
        );
        assert_eq!(
            Error::Auth("bad signature".into()).to_string(),
            "authentication failed: bad signature"
        );
        assert_eq!(
            Error::InsufficientBalance.to_string(),
            "insufficient balance"
        );
        assert_eq!(
            Error::RateLimited { retry_after: None }.to_string(),
            "rate limited"
        );
        assert_eq!(
            Error::RateLimited {
                retry_after: Some(Duration::from_millis(1500))
            }
            .to_string(),
            "rate limited (retry after 1.5s)"
        );
        assert_eq!(
            Error::OrderRejected {
                code: "-2010".into(),
                message: "rejected".into()
            }
            .to_string(),
            "order rejected by exchange (code -2010): rejected"
        );
        assert_eq!(
            Error::Exchange {
                code: "1".into(),
                message: "oops".into()
            }
            .to_string(),
            "exchange error (code 1): oops"
        );
        assert_eq!(
            Error::NotFound("order 7".into()).to_string(),
            "not found: order 7"
        );
        assert_eq!(Error::Timeout.to_string(), "request timed out");
        assert_eq!(Error::NotConnected.to_string(), "not connected");
        assert_eq!(
            Error::Network("eof".into()).to_string(),
            "network error: eof"
        );
        assert_eq!(
            Error::Deserialization("bad json".into()).to_string(),
            "deserialization error: bad json"
        );
    }

    #[test]
    fn with_retry_after_fills_an_empty_rate_limit() {
        let filled = Error::RateLimited { retry_after: None }
            .with_retry_after(Some(Duration::from_millis(2500)));
        let expected = Some(Duration::from_millis(2500));
        assert!(matches!(filled, Error::RateLimited { retry_after } if retry_after == expected));
    }

    #[test]
    fn with_retry_after_does_not_overwrite_what_the_venue_already_said() {
        // A venue that states the delay in its body is more specific than the
        // header, so the header must not replace it.
        let body_said = Error::RateLimited {
            retry_after: Some(Duration::from_secs(60)),
        };
        let kept = body_said.with_retry_after(Some(Duration::from_secs(1)));
        let expected = Some(Duration::from_secs(60));
        assert!(matches!(kept, Error::RateLimited { retry_after } if retry_after == expected));
    }

    #[test]
    fn with_retry_after_leaves_every_other_variant_alone() {
        let timeout = Error::Timeout.with_retry_after(Some(Duration::from_secs(5)));
        assert!(matches!(timeout, Error::Timeout));
        let balance = Error::InsufficientBalance.with_retry_after(Some(Duration::from_secs(5)));
        assert!(matches!(balance, Error::InsufficientBalance));
    }
}
