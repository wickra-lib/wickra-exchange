//! # wickra-exchange
//!
//! Streaming-native crypto-exchange connectivity built on the Wickra core: one
//! typed, unified API over the ten largest exchanges (market data + signed order
//! execution). This crate is a thin facade that re-exports
//! [`wickra_exchange_core`]; depend on it for the stable public surface.
//!
//! See the repository README for the supported exchanges, the language bindings
//! (native Python/Node + a C ABI hub for C/C++/C#/Go/Java/R), and the streaming
//! model.

pub use wickra_exchange_core::*;

mod factory;
mod net;

pub use factory::{connect, connect_advanced, connect_derivatives};
pub use net::{ReqwestHttpTransport, TungsteniteWsTransport};
