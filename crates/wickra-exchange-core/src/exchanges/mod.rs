//! Per-exchange implementations.
//!
//! Each venue is a module here, implementing the same surface behind its own
//! authentication, WebSocket state machine and symbol/filter mapping. Every
//! client is generic over the injected [`HttpTransport`](crate::HttpTransport),
//! so its request-build → parse → normalise logic is tested offline against the
//! mock transport.

mod binance;
mod bitget;
mod bybit;
mod kucoin;
mod okx;

pub use binance::Binance;
pub use bitget::Bitget;
pub use bybit::Bybit;
pub use kucoin::KuCoin;
pub use okx::Okx;
