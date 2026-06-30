//! # wickra-exchange-core
//!
//! Core traits, types and shared machinery for [`wickra-exchange`], a
//! streaming-native crypto-exchange connectivity library built on the Wickra
//! core.
//!
//! The crate exposes one typed, unified API — [`Exchange`] = [`MarketData`] +
//! [`Execution`] — implemented per exchange behind bespoke auth and WebSocket
//! state machines. Market-data streams are **pull-based** (`poll_events`), so
//! the same surface crosses the C ABI to every binding (including single-threaded
//! R) as trivially as a synchronous call.
//!
//! Order-layer quantities use [`rust_decimal::Decimal`], never `f64`: exchanges
//! reject mis-rounded prices and quantities, and float drift loses money.
//!
//! [`wickra-exchange`]: https://github.com/wickra-lib/wickra-exchange
//! [`Exchange`]: crate
//! [`MarketData`]: crate
//! [`Execution`]: crate

/// The crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
