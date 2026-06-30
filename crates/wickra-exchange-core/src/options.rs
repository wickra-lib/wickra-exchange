//! Per-connection configuration.
//!
//! [`MarketType`] is the largest structural axis: a single venue such as Binance
//! is really three or four APIs — spot, USDⓈ-M futures, COIN-M futures, margin —
//! with different base URLs, endpoints and symbol filters. [`ExchangeOptions`]
//! carries that choice plus the transport knobs (testnet, receive window,
//! timeout, proxy) and the perpetual-specific position/margin modes.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which market of a venue to connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketType {
    /// Spot market.
    Spot,
    /// USDⓈ-margined linear perpetual / futures.
    UsdMFutures,
    /// Coin-margined inverse perpetual / futures.
    CoinMFutures,
    /// Cross/isolated margin spot.
    Margin,
}

impl MarketType {
    /// Whether this market is a derivatives (futures/perp) market, where
    /// leverage, funding and position mode apply.
    #[must_use]
    pub fn is_derivatives(self) -> bool {
        matches!(self, MarketType::UsdMFutures | MarketType::CoinMFutures)
    }
}

/// Position mode for derivatives accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionMode {
    /// A single net position per symbol.
    OneWay,
    /// Separate long and short positions per symbol.
    Hedge,
}

/// Margin mode for derivatives accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarginMode {
    /// Margin shared across positions.
    Cross,
    /// Margin isolated per position.
    Isolated,
}

/// Configuration for one exchange connection.
#[derive(Debug, Clone)]
pub struct ExchangeOptions {
    /// Which market of the venue to use.
    pub market_type: MarketType,
    /// Use the venue's testnet/sandbox rather than mainnet.
    pub testnet: bool,
    /// The signed-request receive window in milliseconds (how long a signed
    /// request stays valid against the server clock).
    pub recv_window_ms: u64,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Optional `User-Agent` header override.
    pub user_agent: Option<String>,
    /// Optional HTTP/HTTPS proxy URL.
    pub proxy: Option<String>,
    /// Derivatives position mode.
    pub position_mode: PositionMode,
    /// Derivatives margin mode.
    pub margin_mode: MarginMode,
}

impl ExchangeOptions {
    /// Mainnet options for the given market, with sensible defaults.
    #[must_use]
    pub fn mainnet(market_type: MarketType) -> Self {
        Self {
            market_type,
            testnet: false,
            recv_window_ms: 5_000,
            timeout: Duration::from_secs(10),
            user_agent: None,
            proxy: None,
            position_mode: PositionMode::OneWay,
            margin_mode: MarginMode::Cross,
        }
    }

    /// Testnet/sandbox options for the given market.
    #[must_use]
    pub fn testnet(market_type: MarketType) -> Self {
        Self {
            testnet: true,
            ..Self::mainnet(market_type)
        }
    }

    /// Override the receive window (milliseconds).
    #[must_use]
    pub fn with_recv_window(mut self, ms: u64) -> Self {
        self.recv_window_ms = ms;
        self
    }

    /// Override the per-request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Override the position mode (derivatives).
    #[must_use]
    pub fn with_position_mode(mut self, mode: PositionMode) -> Self {
        self.position_mode = mode;
        self
    }

    /// Override the margin mode (derivatives).
    #[must_use]
    pub fn with_margin_mode(mut self, mode: MarginMode) -> Self {
        self.margin_mode = mode;
        self
    }
}

impl Default for ExchangeOptions {
    /// Mainnet spot with default transport knobs.
    fn default() -> Self {
        Self::mainnet(MarketType::Spot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivatives_classification() {
        assert!(MarketType::UsdMFutures.is_derivatives());
        assert!(MarketType::CoinMFutures.is_derivatives());
        assert!(!MarketType::Spot.is_derivatives());
        assert!(!MarketType::Margin.is_derivatives());
    }

    #[test]
    fn testnet_flips_only_the_flag() {
        let main = ExchangeOptions::mainnet(MarketType::Spot);
        let test = ExchangeOptions::testnet(MarketType::Spot);
        assert!(!main.testnet);
        assert!(test.testnet);
        assert_eq!(main.recv_window_ms, test.recv_window_ms);
        assert_eq!(main.timeout, test.timeout);
    }

    #[test]
    fn builders_override_fields() {
        let opts = ExchangeOptions::mainnet(MarketType::UsdMFutures)
            .with_recv_window(10_000)
            .with_timeout(Duration::from_secs(30))
            .with_position_mode(PositionMode::Hedge)
            .with_margin_mode(MarginMode::Isolated);
        assert_eq!(opts.recv_window_ms, 10_000);
        assert_eq!(opts.timeout, Duration::from_secs(30));
        assert_eq!(opts.position_mode, PositionMode::Hedge);
        assert_eq!(opts.margin_mode, MarginMode::Isolated);
    }

    #[test]
    fn default_is_mainnet_spot() {
        let opts = ExchangeOptions::default();
        assert_eq!(opts.market_type, MarketType::Spot);
        assert!(!opts.testnet);
    }
}
