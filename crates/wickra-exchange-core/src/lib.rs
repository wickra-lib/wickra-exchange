//! # wickra-exchange-core
//!
//! Core traits, types and shared machinery for [`wickra-exchange`], a
//! streaming-native crypto-exchange connectivity library built on the Wickra
//! core.
//!
//! The crate exposes one typed, unified API — `Exchange` = `MarketData` +
//! `Execution` — implemented per exchange behind bespoke auth and WebSocket
//! state machines. Market-data streams are **pull-based** (`poll_events`), so
//! the same surface crosses the C ABI to every binding (including single-threaded
//! R) as trivially as a synchronous call.
//!
//! Order-layer quantities use [`rust_decimal::Decimal`], never `f64`: exchanges
//! reject mis-rounded prices and quantities, and float drift loses money.
//!
//! [`wickra-exchange`]: https://github.com/wickra-lib/wickra-exchange

mod credentials;
mod error;
mod events;
mod idempotency;
mod instruments;
mod options;
mod symbol;
mod traits;
mod transport;
mod types;

pub use credentials::Credentials;
pub use error::{Error, Result};
pub use events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
pub use idempotency::ClientIdGenerator;
pub use instruments::{Instrument, InstrumentCache, InstrumentFilters};
pub use options::{ExchangeOptions, MarginMode, MarketType, PositionMode};
pub use symbol::Symbol;
pub use traits::{Exchange, Execution, MarketData};
pub use transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, MockHttpTransport, MockWsConnection,
    MockWsTransport, WsConnection, WsTransport,
};
pub use types::{
    Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker, TimeInForce,
};

/// Re-export of [`wickra_core::Candle`], the candle type returned by
/// [`MarketData::klines`] so market data feeds the indicator core directly.
pub use wickra_core::Candle;

/// The crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
