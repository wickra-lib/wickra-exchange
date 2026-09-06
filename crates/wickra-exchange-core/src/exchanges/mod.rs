//! Per-exchange implementations.
//!
//! Each venue is a module here, implementing the same surface behind its own
//! authentication, WebSocket state machine and symbol/filter mapping. Every
//! client is generic over the injected [`HttpTransport`](crate::HttpTransport),
//! so its request-build → parse → normalise logic is tested offline against the
//! mock transport.

use crate::error::{Error, Result};
use crate::options::MarketType;
use crate::transport::HttpResponse;

/// The markets a venue client with both a spot and a linear-futures path routes.
pub(crate) const SPOT_AND_LINEAR: &[MarketType] = &[MarketType::Spot, MarketType::UsdMFutures];

/// The markets a spot-only venue client routes.
pub(crate) const SPOT_ONLY: &[MarketType] = &[MarketType::Spot];

/// Refuse a market this client does not route, before anything is sent.
///
/// [`MarketType`] names four markets and no client here routes all four. The
/// ones that were not routed did not fail -- they resolved to whichever market
/// the client's URL builder happened to produce, and answered:
///
/// * Binance served `api/v3/ticker/24hr?symbol=BTCUSD` for a **coin-margined**
///   request. `BTCUSD` is a real Binance *spot* pair, so that is real spot data
///   returned for a futures question, with no error and nothing to notice. An
///   order would have bought spot BTC with USD instead of opening an inverse
///   position.
/// * Kraken asked for `PF_XBTUSD`, its *linear* multi-collateral perpetual,
///   where the coin-margined product is `PI_XBTUSD`. Both exist and both
///   answer.
/// * Every client routed `Margin` to its plain spot path, where a margin order
///   becomes an ordinary spot order: the borrow the caller asked for simply
///   does not happen.
/// * Bitget, Gate, HTX, KuCoin and Upbit at least failed loudly -- an empty
///   list, `CONTRACT_NOT_FOUND`, `invalid-parameter`, a 404 -- but failed
///   because of a URL the caller could not see, and could not act on.
///
/// Bybit and OKX are the two that *would* have routed coin-margined data
/// correctly (`category=inverse`, `BTC-USD-SWAP`, both verified against the
/// live venues). They are refused with the rest anyway, because an inverse
/// order's size is denominated differently -- Bybit's inverse `qty` is in USD,
/// not in the base coin -- so `quantity` would silently mean something else on
/// those two clients than on every other. Half a market is the defect this
/// refusal exists to prevent, not a smaller version of the feature.
pub(crate) fn ensure_market_is_routed(
    venue: &'static str,
    asked: MarketType,
    routed: &[MarketType],
) -> Result<()> {
    if routed.contains(&asked) {
        return Ok(());
    }
    Err(Error::unsupported_market(venue, asked))
}

/// How much of an unreadable body an error message carries. Enough to see what
/// kind of document arrived, short enough to stay one line in a CI log.
const BODY_EXCERPT_CHARS: usize = 200;

/// Blame the HTTP status for a reply the parser could not read, when the reply
/// was never the venue's API to begin with.
///
/// Bybit, OKX, Bitget, Kraken and HTX all answer HTTP 200 for their own errors
/// and carry the real code inside the envelope, so their clients read the body
/// and never look at the status. That is right about the venue and wrong about
/// everything standing in front of it: a geo-block, a gateway error or a CDN
/// challenge answers with a status of its own and a body that was never JSON.
/// Calling that a deserialization failure blames the parser for a document it
/// was never meant to read, and discards the one field that says what happened.
///
/// The nightly live suite is built on exactly that distinction -- it skips a
/// venue the runner cannot reach and fails on a parser that cannot read a real
/// reply -- so the misattribution turned the runner's location into a standing
/// red build, reported as `key must be a string at line 2 column 5` with
/// nothing in the log to tell the two cases apart.
///
/// A body that fails to parse under a *successful* status is drift and keeps
/// its error, with an excerpt appended: serde's position alone cannot say
/// whether a field was renamed or no JSON arrived at all, which is why the
/// first occurrence could not be diagnosed from the log it left behind.
pub(crate) fn attribute_to_http_status(response: &HttpResponse, error: Error) -> Error {
    let Error::Deserialization(detail) = error else {
        return error;
    };
    if response.is_success() {
        return Error::Deserialization(format!(
            "{detail} (body: {})",
            body_excerpt(&response.body)
        ));
    }
    Error::Exchange {
        code: response.status.to_string(),
        message: response.body.clone(),
    }
}

/// The leading [`BODY_EXCERPT_CHARS`] characters of `body`, cut on a character
/// boundary so a multi-byte document does not panic the error path.
fn body_excerpt(body: &str) -> String {
    match body.char_indices().nth(BODY_EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}...", &body[..cut]),
        None => body.to_string(),
    }
}

mod binance;
mod bitget;
mod bybit;
mod coinbase;
mod gate;
mod htx;
mod kraken;
mod kucoin;
mod okx;
mod paper;
mod replay;
mod upbit;

pub use binance::Binance;
pub use bitget::Bitget;
pub use bybit::Bybit;
pub use coinbase::Coinbase;
pub use gate::Gate;
pub use htx::Htx;
pub use kraken::Kraken;
pub use kucoin::KuCoin;
pub use okx::Okx;
pub use paper::PaperExchange;
pub use replay::ReplayExchange;
pub use upbit::Upbit;

#[cfg(test)]
mod tests {
    use super::{attribute_to_http_status, body_excerpt, BODY_EXCERPT_CHARS};
    use crate::error::Error;
    use crate::transport::HttpResponse;

    /// What a blocked runner actually receives: a status of its own, and a body
    /// that is not the venue's JSON.
    const BLOCKED: &str = "{\n    error: 'unavailable in your region'\n}";

    #[test]
    fn a_failed_status_is_reported_as_the_venue_refusing_not_the_parser_failing() {
        let response = HttpResponse::new(403, BLOCKED);
        let error = Error::Deserialization("key must be a string at line 2 column 5".to_string());
        let attributed = attribute_to_http_status(&response, error);
        let named = matches!(&attributed, Error::Exchange { code, .. } if code == "403");
        assert!(named, "a non-success status must not stay a parse failure");
        let kept = matches!(&attributed, Error::Exchange { message, .. } if message == BLOCKED);
        assert!(kept, "the body the venue sent is the evidence");
    }

    #[test]
    fn a_successful_status_keeps_the_drift_error_and_gains_the_body() {
        let response = HttpResponse::new(200, r#"{"retCode":0,"result":{"list":"moved"}}"#);
        let error = Error::Deserialization("invalid type: string".to_string());
        let attributed = attribute_to_http_status(&response, error);
        let drift = matches!(&attributed, Error::Deserialization(detail)
            if detail.starts_with("invalid type: string (body: "));
        assert!(drift, "drift under a successful status stays drift");
        let shown = matches!(&attributed, Error::Deserialization(detail)
            if detail.contains(r#""list":"moved""#));
        assert!(shown, "the body is what makes the drift diagnosable");
    }

    #[test]
    fn an_error_that_is_not_a_parse_failure_passes_through_untouched() {
        // A venue that named its own error already said what happened; the
        // status has nothing to add.
        let response = HttpResponse::new(429, "");
        let attributed =
            attribute_to_http_status(&response, Error::RateLimited { retry_after: None });
        assert!(matches!(
            attributed,
            Error::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn a_short_body_is_carried_whole() {
        assert_eq!(body_excerpt("not json"), "not json");
    }

    #[test]
    fn a_long_body_is_cut_and_marked() {
        let cut = body_excerpt(&"x".repeat(BODY_EXCERPT_CHARS + 50));
        assert_eq!(cut.len(), BODY_EXCERPT_CHARS + 3);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn a_multibyte_body_is_cut_on_a_character_boundary() {
        // A CDN error page in a non-Latin script must not panic the error path
        // that exists to describe it.
        let cut = body_excerpt(&"ß".repeat(BODY_EXCERPT_CHARS + 10));
        assert_eq!(cut.chars().count(), BODY_EXCERPT_CHARS + 3);
    }
}
