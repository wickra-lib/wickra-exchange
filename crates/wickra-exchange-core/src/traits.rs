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
use crate::types::{Balance, OcoRequest, Order, OrderRequest, Ticker};
use rust_decimal::Decimal;
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

/// Advanced order operations beyond single placement: amend-in-place, batched
/// placement and cancellation, and one-cancels-other (OCO) brackets. Implemented
/// per venue where the underlying API supports each operation; venues document
/// any operation they cannot express natively.
pub trait AdvancedOrders {
    /// Amend a resting order's price and/or quantity in place, returning the
    /// order as the venue reports it afterwards. `None` leaves that field
    /// unchanged. Venues without a native amend emulate it as cancel-replace.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the order is unknown, the amend is
    /// rejected, or the request fails.
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order>;

    /// Place several orders in one request. The outer [`Result`] covers a
    /// transport/auth failure; each inner [`Result`] is that order's own outcome,
    /// so a partially-accepted batch still returns the successes.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the whole request fails.
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>>;

    /// Cancel several orders on one `symbol` in a single request.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the request fails.
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()>;

    /// Place a one-cancels-other bracket, returning the resulting order legs.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the OCO is invalid, rejected, or the
    /// request fails.
    fn place_oco(&mut self, request: &OcoRequest) -> Result<Vec<Order>>;
}

/// Private user-data streaming: subscribe to the account's own order and balance
/// updates so [`poll_events`](MarketData::poll_events) surfaces
/// [`Event::OrderUpdate`](crate::Event::OrderUpdate) and
/// [`Event::BalanceUpdate`](crate::Event::BalanceUpdate). Implemented by venues
/// that expose a private WebSocket stream (a listen-key / login handshake).
///
/// [`MarketData`] is a supertrait, so a `WsUserData` client (including the
/// `Box<dyn WsUserData>` returned by the facade's `connect_user_data`) can
/// [`poll_events`](MarketData::poll_events) directly after subscribing.
pub trait WsUserData: MarketData {
    /// Open the private user-data stream. After it returns,
    /// [`poll_events`](MarketData::poll_events) on the same client also drains the
    /// user's order and balance events alongside the public market-data stream.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if credentials are missing, no WebSocket
    /// transport is configured, or the stream cannot be opened.
    fn subscribe_user_data(&mut self) -> Result<()>;

    /// Keep the private user-data stream alive: refresh the venue's session so it
    /// is not dropped for inactivity. The consumer calls this periodically (e.g.
    /// on a timer). Venues that need a REST keepalive refresh their listen key;
    /// venues that need an application-level heartbeat send a ping frame. The
    /// default is a no-op for venues that need neither.
    ///
    /// A dropped stream is also recovered automatically on the next
    /// [`poll_events`](MarketData::poll_events), which re-subscribes with fresh
    /// credentials and emits [`Event::Disconnected`](crate::Event::Disconnected)
    /// then [`Event::Reconnected`](crate::Event::Reconnected).
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if the refresh request fails.
    fn keepalive_user_data(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Order placement and cancellation over a venue's WebSocket API (`ws-api`),
/// where a signed request frame is sent on a dedicated connection and the
/// matching response frame is read back. Lower-latency than REST; implemented by
/// venues that expose a WebSocket order API.
pub trait WsExecution {
    /// Place an order over the WebSocket API and return it as the venue reports it.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if no WebSocket transport is configured,
    /// the order is invalid, or the venue rejects it.
    fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order>;

    /// Cancel an order over the WebSocket API by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`](crate::Error) if no WebSocket transport is configured,
    /// the order is unknown, or the request fails.
    fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()>;
}
