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
mod coinbase;
mod gate;
mod htx;
mod kraken;
mod kucoin;
mod okx;
mod paper;
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
pub use upbit::Upbit;
