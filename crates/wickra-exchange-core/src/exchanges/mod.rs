//! Per-exchange implementations.
//!
//! Each venue is a module here, implementing the same surface behind its own
//! authentication, WebSocket state machine and symbol/filter mapping. Every
//! client is generic over the injected [`HttpTransport`](crate::HttpTransport),
//! so its request-build → parse → normalise logic is tested offline against the
//! mock transport.

use crate::error::{Error, Result};
use crate::options::MarketType;

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
