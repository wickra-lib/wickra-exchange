//! The unified exchange API.
//!
//! [`Exchange`] = [`MarketData`] + [`Execution`]. The shape is identical across
//! every venue; the implementation behind it — authentication, the WebSocket
//! state machine, symbol and filter mapping — is bespoke per exchange.
//!
//! The surface is synchronous: a real implementation drives an async client
//! internally, but consumers (and every language binding) call blocking methods.
//! Streams are **pull-based** — [`subscribe`](MarketData::subscribe_trades) opens
//! a subscription that fills an internal buffer, and [`poll_events`](MarketData::poll_events)
//! drains it. The consumer owns its loop, which is what lets the C ABI carry
//! streaming to every binding (including single-threaded R) as a plain call.
//!
//! All three traits are object-safe, so the factory can return `Box<dyn Exchange>`.

use crate::error::Result;
use crate::events::{Event, OrderBookSnapshot};
use crate::options::MarginMode;
use crate::positions::Position;
use crate::symbol::Symbol;
use crate::types::{Balance, Order, OrderRequest, Ticker};
use wickra_core::Candle;

/// Read-only market data: REST snapshots plus pull-based streaming.
pub trait MarketData {
    /// A point-in-time ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails or the symbol is unknown.
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker>;

    /// Up to `limit` historical candles for `symbol` at the given `interval`
    /// (e.g. `"1m"`, `"1h"`). Returns [`wickra_core::Candle`]s so the result
    /// feeds the indicator core directly.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails or the symbol/interval is unknown.
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>>;

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails or the symbol is unknown.
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot>;

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the subscription cannot be opened.
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()>;

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the subscription cannot be opened.
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()>;

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the subscription cannot be opened.
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()>;

    /// Drain all events buffered since the last call. Non-blocking: returns an
    /// empty vector when nothing is pending.
    fn poll_events(&mut self) -> Vec<Event>;
}

/// Signed order execution and account access.
pub trait Execution {
    /// Place an order and return it as accepted by the venue.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the order is invalid, violates a
    /// filter, is rejected, or the request fails.
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order>;

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the order is unknown or the request fails.
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()>;

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the order is unknown or the request fails.
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order>;

    /// All open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails.
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>>;

    /// Current account balances.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails.
    fn balances(&mut self) -> Result<Vec<Balance>>;
}

/// A full exchange connection: market data and execution behind one typed API.
pub trait Exchange: MarketData + Execution {
    /// The venue's lowercase identifier (e.g. `"binance"`).
    fn name(&self) -> &'static str;
}

/// Derivatives (perpetual / futures) account operations: positions, leverage and
/// margin mode. Implemented only by venues that offer derivatives markets and
/// meaningful only on a derivatives [`MarketType`](crate::MarketType); spot-only
/// venues (Coinbase, Upbit) do not implement it.
pub trait Derivatives {
    /// Open positions, optionally filtered to one `symbol`; flat positions are omitted.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if credentials are missing or the request fails.
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>>;

    /// Set the account leverage for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the leverage is rejected or the request fails.
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()>;

    /// Set the margin mode (cross / isolated) for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the change is rejected or the request fails.
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()>;

    /// Flatten the open position in `symbol` with a reduce-only market order.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`](crate::Error::NotFound) if there is no open
    /// position, or another [`Error`](crate::Error) if the request fails.
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order>;
}
