//! Per-exchange implementations.
//!
//! Each venue is a module here, implementing the same surface behind its own
//! authentication, WebSocket state machine and symbol/filter mapping. Every
//! client is generic over the injected [`HttpTransport`](crate::HttpTransport),
//! so its request-build → parse → normalise logic is tested offline against the
//! mock transport.

mod binance;
mod bybit;

pub use binance::Binance;
pub use bybit::Bybit;
