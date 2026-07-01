//! Construct a ready-to-use, real-socket exchange client by name.
//!
//! [`connect`] is the one-call entry point for applications: give it a venue
//! name, credentials and options, and it wires the real [`ReqwestHttpTransport`]
//! and [`TungsteniteWsTransport`] into the matching venue client and hands back a
//! boxed [`Exchange`] trait object. Everything below the trait — signing, symbol
//! mapping, the WebSocket state machine — is already covered by the core's
//! offline suite; this module only performs name dispatch and transport wiring,
//! so it lives in the facade and is excluded from coverage (name dispatch is the
//! one deterministic path a unit test can exercise without a network).

use crate::{ReqwestHttpTransport, TungsteniteWsTransport};
use wickra_exchange_core::{
    AdvancedOrders, Binance, Bitget, Bybit, Coinbase, Credentials, Derivatives, Error, Exchange,
    ExchangeOptions, Gate, HttpTransport, Htx, Kraken, KuCoin, Okx, Result, Upbit, WsExecution,
    WsTransport, WsUserData,
};

/// Build the real blocking HTTP + pull-based WebSocket transports for a client.
fn transports(options: &ExchangeOptions) -> Result<(Box<dyn HttpTransport>, Box<dyn WsTransport>)> {
    let http = Box::new(ReqwestHttpTransport::new(options)?) as Box<dyn HttpTransport>;
    let ws = Box::new(TungsteniteWsTransport::new()) as Box<dyn WsTransport>;
    Ok((http, ws))
}

/// Build a real-socket, authenticated client for `name`, ready to stream and
/// trade.
///
/// `name` is matched case-insensitively against the canonical venue keys:
/// `binance`, `bybit`, `okx`, `bitget`, `kucoin`, `gateio`, `htx`, `kraken`,
/// `coinbase` and `upbit`. The returned client carries a blocking HTTP transport
/// and a pull-based WebSocket transport, so it needs no async runtime on the
/// caller's side.
///
/// # Errors
///
/// Returns [`Error::UnsupportedExchange`] if `name` is not a known venue, or
/// [`Error::Network`] if the HTTP client cannot be constructed from `options`.
pub fn connect(
    name: &str,
    credentials: Credentials,
    options: &ExchangeOptions,
) -> Result<Box<dyn Exchange>> {
    let (http, ws) = transports(options)?;

    let exchange: Box<dyn Exchange> = match name.to_ascii_lowercase().as_str() {
        "binance" => Box::new(Binance::with_credentials(http, options, credentials).with_ws(ws)),
        "bybit" => Box::new(Bybit::with_credentials(http, options, credentials).with_ws(ws)),
        "okx" => Box::new(Okx::with_credentials(http, options, credentials).with_ws(ws)),
        "bitget" => Box::new(Bitget::with_credentials(http, options, credentials).with_ws(ws)),
        "kucoin" => Box::new(KuCoin::with_credentials(http, options, credentials).with_ws(ws)),
        "gateio" => Box::new(Gate::with_credentials(http, options, credentials).with_ws(ws)),
        "htx" => Box::new(Htx::with_credentials(http, options, credentials).with_ws(ws)),
        "kraken" => Box::new(Kraken::with_credentials(http, options, credentials).with_ws(ws)),
        "coinbase" => Box::new(Coinbase::with_credentials(http, options, credentials).with_ws(ws)),
        "upbit" => Box::new(Upbit::with_credentials(http, options, credentials).with_ws(ws)),
        other => return Err(Error::UnsupportedExchange(other.to_string())),
    };
    Ok(exchange)
}

/// Build a derivatives client for `name` as a boxed [`Derivatives`] trait object
/// (positions / leverage / margin mode / reduce-only close).
///
/// Only the eight venues with futures/perpetual markets are dispatched; the
/// spot-only venues (`coinbase`, `upbit`) return [`Error::UnsupportedExchange`].
/// Pair with a derivatives [`MarketType`](wickra_exchange_core::MarketType) in
/// `options` so the client routes to the futures API.
///
/// # Errors
///
/// Returns [`Error::UnsupportedExchange`] if `name` is unknown or spot-only, or
/// [`Error::Network`] if the HTTP client cannot be constructed from `options`.
pub fn connect_derivatives(
    name: &str,
    credentials: Credentials,
    options: &ExchangeOptions,
) -> Result<Box<dyn Derivatives>> {
    let (http, ws) = transports(options)?;

    let client: Box<dyn Derivatives> = match name.to_ascii_lowercase().as_str() {
        "binance" => Box::new(Binance::with_credentials(http, options, credentials).with_ws(ws)),
        "bybit" => Box::new(Bybit::with_credentials(http, options, credentials).with_ws(ws)),
        "okx" => Box::new(Okx::with_credentials(http, options, credentials).with_ws(ws)),
        "bitget" => Box::new(Bitget::with_credentials(http, options, credentials).with_ws(ws)),
        "kucoin" => Box::new(KuCoin::with_credentials(http, options, credentials).with_ws(ws)),
        "gateio" => Box::new(Gate::with_credentials(http, options, credentials).with_ws(ws)),
        "htx" => Box::new(Htx::with_credentials(http, options, credentials).with_ws(ws)),
        "kraken" => Box::new(Kraken::with_credentials(http, options, credentials).with_ws(ws)),
        "coinbase" | "upbit" => {
            return Err(Error::UnsupportedExchange(format!(
                "{name} is spot-only (no derivatives market)"
            )))
        }
        other => return Err(Error::UnsupportedExchange(other.to_string())),
    };
    Ok(client)
}

/// Build a client for `name` as a boxed [`AdvancedOrders`] trait object (amend /
/// batch place-cancel / OCO). Available on the eight trading venues; the
/// spot-only venues (`coinbase`, `upbit`) return [`Error::UnsupportedExchange`].
///
/// Whether each operation is native or a documented gap is venue-specific — see
/// [docs/CAPABILITIES.md](https://github.com/wickra-lib/wickra-exchange/blob/main/docs/CAPABILITIES.md).
///
/// # Errors
///
/// Returns [`Error::UnsupportedExchange`] if `name` is unknown or unsupported, or
/// [`Error::Network`] if the HTTP client cannot be constructed from `options`.
pub fn connect_advanced(
    name: &str,
    credentials: Credentials,
    options: &ExchangeOptions,
) -> Result<Box<dyn AdvancedOrders>> {
    let (http, ws) = transports(options)?;

    let client: Box<dyn AdvancedOrders> = match name.to_ascii_lowercase().as_str() {
        "binance" => Box::new(Binance::with_credentials(http, options, credentials).with_ws(ws)),
        "bybit" => Box::new(Bybit::with_credentials(http, options, credentials).with_ws(ws)),
        "okx" => Box::new(Okx::with_credentials(http, options, credentials).with_ws(ws)),
        "bitget" => Box::new(Bitget::with_credentials(http, options, credentials).with_ws(ws)),
        "kucoin" => Box::new(KuCoin::with_credentials(http, options, credentials).with_ws(ws)),
        "gateio" => Box::new(Gate::with_credentials(http, options, credentials).with_ws(ws)),
        "htx" => Box::new(Htx::with_credentials(http, options, credentials).with_ws(ws)),
        "kraken" => Box::new(Kraken::with_credentials(http, options, credentials).with_ws(ws)),
        "coinbase" | "upbit" => {
            return Err(Error::UnsupportedExchange(format!(
                "{name} has no advanced-order surface"
            )))
        }
        other => return Err(Error::UnsupportedExchange(other.to_string())),
    };
    Ok(client)
}

/// Build a real-socket client that streams the account's own order and balance
/// updates over a private WebSocket. After
/// [`subscribe_user_data`](WsUserData::subscribe_user_data), the client's
/// `poll_events` surfaces the user's own `OrderUpdate` / `BalanceUpdate` events.
///
/// Available on the eight trading venues; the spot-only venues (`coinbase`,
/// `upbit`) return [`Error::UnsupportedExchange`].
///
/// # Errors
///
/// Returns [`Error::UnsupportedExchange`] if `name` is unknown or unsupported, or
/// [`Error::Network`] if the HTTP client cannot be constructed from `options`.
pub fn connect_user_data(
    name: &str,
    credentials: Credentials,
    options: &ExchangeOptions,
) -> Result<Box<dyn WsUserData>> {
    let (http, ws) = transports(options)?;

    let client: Box<dyn WsUserData> = match name.to_ascii_lowercase().as_str() {
        "binance" => Box::new(Binance::with_credentials(http, options, credentials).with_ws(ws)),
        "bybit" => Box::new(Bybit::with_credentials(http, options, credentials).with_ws(ws)),
        "okx" => Box::new(Okx::with_credentials(http, options, credentials).with_ws(ws)),
        "bitget" => Box::new(Bitget::with_credentials(http, options, credentials).with_ws(ws)),
        "kucoin" => Box::new(KuCoin::with_credentials(http, options, credentials).with_ws(ws)),
        "gateio" => Box::new(Gate::with_credentials(http, options, credentials).with_ws(ws)),
        "htx" => Box::new(Htx::with_credentials(http, options, credentials).with_ws(ws)),
        "kraken" => Box::new(Kraken::with_credentials(http, options, credentials).with_ws(ws)),
        "coinbase" | "upbit" => {
            return Err(Error::UnsupportedExchange(format!(
                "{name} has no private user-data stream"
            )))
        }
        other => return Err(Error::UnsupportedExchange(other.to_string())),
    };
    Ok(client)
}

/// Build a real-socket client that places and cancels orders over a venue's
/// WebSocket order API.
///
/// Available on the eight trading venues. Order entry is native on `binance`,
/// `bybit`, `okx`, `gateio` and `kraken`; on `bitget`, `kucoin` and `htx` the
/// venue exposes no WebSocket order-entry API, so the methods return a documented
/// error pointing to REST — see
/// [docs/CAPABILITIES.md](https://github.com/wickra-lib/wickra-exchange/blob/main/docs/CAPABILITIES.md).
/// The spot-only venues (`coinbase`, `upbit`) return [`Error::UnsupportedExchange`].
///
/// # Errors
///
/// Returns [`Error::UnsupportedExchange`] if `name` is unknown or unsupported, or
/// [`Error::Network`] if the HTTP client cannot be constructed from `options`.
pub fn connect_ws_execution(
    name: &str,
    credentials: Credentials,
    options: &ExchangeOptions,
) -> Result<Box<dyn WsExecution>> {
    let (http, ws) = transports(options)?;

    let client: Box<dyn WsExecution> = match name.to_ascii_lowercase().as_str() {
        "binance" => Box::new(Binance::with_credentials(http, options, credentials).with_ws(ws)),
        "bybit" => Box::new(Bybit::with_credentials(http, options, credentials).with_ws(ws)),
        "okx" => Box::new(Okx::with_credentials(http, options, credentials).with_ws(ws)),
        "bitget" => Box::new(Bitget::with_credentials(http, options, credentials).with_ws(ws)),
        "kucoin" => Box::new(KuCoin::with_credentials(http, options, credentials).with_ws(ws)),
        "gateio" => Box::new(Gate::with_credentials(http, options, credentials).with_ws(ws)),
        "htx" => Box::new(Htx::with_credentials(http, options, credentials).with_ws(ws)),
        "kraken" => Box::new(Kraken::with_credentials(http, options, credentials).with_ws(ws)),
        "coinbase" | "upbit" => {
            return Err(Error::UnsupportedExchange(format!(
                "{name} has no WebSocket order API"
            )))
        }
        other => return Err(Error::UnsupportedExchange(other.to_string())),
    };
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wickra_exchange_core::MarketType;

    fn opts() -> ExchangeOptions {
        ExchangeOptions::mainnet(MarketType::Spot)
    }

    fn creds() -> Credentials {
        Credentials::new("key", "secret")
    }

    #[test]
    fn dispatches_every_known_venue() {
        // Construction is offline (no socket is opened until the client is used),
        // so the full name table is deterministic to exercise here.
        for name in [
            "binance", "bybit", "okx", "bitget", "kucoin", "gateio", "htx", "kraken", "coinbase",
            "upbit",
        ] {
            let client = connect(name, creds(), &opts()).unwrap();
            assert!(!client.name().is_empty());
        }
    }

    #[test]
    fn name_match_is_case_insensitive() {
        assert!(connect("BinAnce", creds(), &opts()).is_ok());
    }

    #[test]
    fn unknown_venue_is_rejected() {
        // `Box<dyn Exchange>` is not `Debug`, so match rather than `unwrap_err`.
        match connect("ftx", creds(), &opts()) {
            Err(Error::UnsupportedExchange(name)) => assert_eq!(name, "ftx"),
            _ => panic!("unknown venue must be rejected"),
        }
    }

    fn futures_opts() -> ExchangeOptions {
        ExchangeOptions::mainnet(MarketType::UsdMFutures)
    }

    #[test]
    fn derivatives_dispatch_covers_the_eight_futures_venues() {
        for name in [
            "binance", "bybit", "okx", "bitget", "kucoin", "gateio", "htx", "kraken",
        ] {
            assert!(
                connect_derivatives(name, creds(), &futures_opts()).is_ok(),
                "{name} should dispatch a derivatives client"
            );
        }
    }

    #[test]
    fn derivatives_rejects_spot_only_and_unknown() {
        for name in ["coinbase", "upbit", "ftx"] {
            match connect_derivatives(name, creds(), &opts()) {
                Err(Error::UnsupportedExchange(_)) => {}
                _ => panic!("{name} must be rejected for derivatives"),
            }
        }
    }

    #[test]
    fn advanced_dispatch_covers_the_eight_trading_venues() {
        for name in [
            "binance", "bybit", "okx", "bitget", "kucoin", "gateio", "htx", "kraken",
        ] {
            assert!(
                connect_advanced(name, creds(), &opts()).is_ok(),
                "{name} should dispatch an advanced-orders client"
            );
        }
    }

    #[test]
    fn advanced_rejects_spot_only_and_unknown() {
        for name in ["coinbase", "upbit", "ftx"] {
            match connect_advanced(name, creds(), &opts()) {
                Err(Error::UnsupportedExchange(_)) => {}
                _ => panic!("{name} must be rejected for advanced orders"),
            }
        }
    }

    #[test]
    fn user_data_dispatch_covers_the_eight_trading_venues() {
        for name in [
            "binance", "bybit", "okx", "bitget", "kucoin", "gateio", "htx", "kraken",
        ] {
            let mut client = connect_user_data(name, creds(), &opts())
                .unwrap_or_else(|_| panic!("{name} should dispatch a user-data client"));
            // `WsUserData: MarketData`, so the boxed facade handle can poll without
            // opening a socket (nothing is buffered yet).
            assert!(client.poll_events().is_empty());
        }
    }

    #[test]
    fn user_data_rejects_spot_only_and_unknown() {
        for name in ["coinbase", "upbit", "ftx"] {
            match connect_user_data(name, creds(), &opts()) {
                Err(Error::UnsupportedExchange(_)) => {}
                _ => panic!("{name} must be rejected for user-data streaming"),
            }
        }
    }

    #[test]
    fn ws_execution_dispatch_covers_the_eight_trading_venues() {
        for name in [
            "binance", "bybit", "okx", "bitget", "kucoin", "gateio", "htx", "kraken",
        ] {
            assert!(
                connect_ws_execution(name, creds(), &opts()).is_ok(),
                "{name} should dispatch a ws-execution client"
            );
        }
    }

    #[test]
    fn ws_execution_rejects_spot_only_and_unknown() {
        for name in ["coinbase", "upbit", "ftx"] {
            match connect_ws_execution(name, creds(), &opts()) {
                Err(Error::UnsupportedExchange(_)) => {}
                _ => panic!("{name} must be rejected for ws execution"),
            }
        }
    }
}
