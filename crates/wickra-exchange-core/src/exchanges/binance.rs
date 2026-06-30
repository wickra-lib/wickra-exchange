//! Binance — the reference exchange implementation.
//!
//! This module is generic over the injected [`HttpTransport`], so the entire
//! request-build → parse → normalise path is exercised offline against
//! [`MockHttpTransport`] with recorded Binance responses. Only the production
//! wiring of a real socket lives elsewhere.
//!
//! This first slice covers the public REST market-data surface (ticker, klines,
//! depth) plus the URL/symbol mapping and the Binance error taxonomy. Signed
//! execution and the WebSocket streams land in later slices.

use crate::error::{Error, Result};
use crate::events::{BookLevel, OrderBookSnapshot};
use crate::normalize::parse_decimal;
use crate::options::{ExchangeOptions, MarketType};
use crate::symbol::Symbol;
use crate::transport::{HttpRequest, HttpResponse, HttpTransport};
use crate::types::Ticker;
use serde::Deserialize;
use wickra_core::Candle;

/// A Binance client over an injected HTTP transport.
pub struct Binance {
    http: Box<dyn HttpTransport>,
    rest_base: String,
    market_type: MarketType,
}

impl Binance {
    /// Build a Binance client over the given transport and options.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self {
            http,
            rest_base: rest_base_url(options.market_type, options.testnet).to_string(),
            market_type: options.market_type,
        }
    }

    /// The market type this client is configured for.
    #[must_use]
    pub fn market_type(&self) -> MarketType {
        self.market_type
    }

    /// The Binance wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTCUSDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated()
    }

    /// A 24-hour ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        let body = self.get("/api/v3/ticker/24hr", &query)?;
        let raw: RawTicker = deserialize(&body)?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&raw.last_price)?,
            bid: parse_decimal(&raw.bid_price)?,
            ask: parse_decimal(&raw.ask_price)?,
            volume: parse_decimal(&raw.volume)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (e.g. `"1m"`, `"1h"`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "symbol={}&interval={interval}&limit={limit}",
            Self::wire_symbol(symbol)
        );
        let body = self.get("/api/v3/klines", &query)?;
        let rows: Vec<Vec<serde_json::Value>> = deserialize(&body)?;
        rows.iter().map(|row| parse_kline_row(row)).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("symbol={}&limit={depth}", Self::wire_symbol(symbol));
        let body = self.get("/api/v3/depth", &query)?;
        let raw: RawDepth = deserialize(&body)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.last_update_id,
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// Issue a GET and return the body, mapping non-2xx responses onto the error
    /// taxonomy.
    fn get(&self, path: &str, query: &str) -> Result<String> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(map_error(&response))
        }
    }
}

/// The REST base URL for a market type and network.
fn rest_base_url(market_type: MarketType, testnet: bool) -> &'static str {
    match (market_type, testnet) {
        (MarketType::UsdMFutures, false) => "https://fapi.binance.com",
        (MarketType::UsdMFutures, true) => "https://testnet.binancefuture.com",
        (_, true) => "https://testnet.binance.vision",
        (_, false) => "https://api.binance.com",
    }
}

#[derive(Deserialize)]
struct RawTicker {
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "bidPrice")]
    bid_price: String,
    #[serde(rename = "askPrice")]
    ask_price: String,
    volume: String,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
struct BinanceError {
    code: i64,
    msg: String,
}

fn deserialize<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))
}

fn parse_levels(levels: &[[String; 2]]) -> Result<Vec<BookLevel>> {
    levels
        .iter()
        .map(|[price, qty]| {
            Ok(BookLevel {
                price: parse_decimal(price)?,
                quantity: parse_decimal(qty)?,
            })
        })
        .collect()
}

fn parse_kline_row(row: &[serde_json::Value]) -> Result<Candle> {
    // Binance kline: [openTime, open, high, low, close, volume, closeTime, ...].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let open_time = row[0]
        .as_i64()
        .ok_or_else(|| Error::Deserialization("kline open time not an integer".to_string()))?;
    let open = kline_f64(&row[1])?;
    let high = kline_f64(&row[2])?;
    let low = kline_f64(&row[3])?;
    let close = kline_f64(&row[4])?;
    let volume = kline_f64(&row[5])?;
    Candle::new(open, high, low, close, volume, open_time)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn kline_f64(value: &serde_json::Value) -> Result<f64> {
    value
        .as_str()
        .ok_or_else(|| Error::Deserialization("kline field not a string".to_string()))?
        .parse::<f64>()
        .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
}

/// Map a non-success Binance response onto the unified error taxonomy.
fn map_error(response: &HttpResponse) -> Error {
    let Ok(err) = serde_json::from_str::<BinanceError>(&response.body) else {
        return Error::Exchange {
            code: response.status.to_string(),
            message: response.body.clone(),
        };
    };
    match err.code {
        -1121 => Error::InvalidSymbol(err.msg),
        -2010 | -2018 | -2019 => Error::InsufficientBalance,
        -1003 => Error::RateLimited { retry_after: None },
        -1022 | -2014 | -2015 => Error::Auth(err.msg),
        _ => Error::Exchange {
            code: err.code.to_string(),
            message: err.msg,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockHttpTransport;
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    fn symbol() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    /// A Binance client over a mock transport, returning the mock so the test can
    /// queue responses and inspect requests.
    fn client(market_type: MarketType, testnet: bool) -> (Binance, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = if testnet {
            ExchangeOptions::testnet(market_type)
        } else {
            ExchangeOptions::mainnet(market_type)
        };
        let binance = Binance::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts);
        (binance, mock)
    }

    /// A transport that forwards to a shared `MockHttpTransport` so the test keeps
    /// a handle after the client takes ownership.
    struct ArcTransport(Arc<MockHttpTransport>);
    impl HttpTransport for ArcTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.0.execute(request)
        }
    }

    #[test]
    fn wire_symbol_concatenates() {
        assert_eq!(Binance::wire_symbol(&symbol()), "BTCUSDT");
    }

    #[test]
    fn rest_base_urls_by_market_and_network() {
        assert_eq!(
            rest_base_url(MarketType::Spot, false),
            "https://api.binance.com"
        );
        assert_eq!(
            rest_base_url(MarketType::Spot, true),
            "https://testnet.binance.vision"
        );
        assert_eq!(
            rest_base_url(MarketType::UsdMFutures, false),
            "https://fapi.binance.com"
        );
        assert_eq!(
            rest_base_url(MarketType::UsdMFutures, true),
            "https://testnet.binancefuture.com"
        );
    }

    #[test]
    fn ticker_parses_and_targets_the_right_url() {
        let (binance, mock) = client(MarketType::Spot, false);
        assert_eq!(binance.market_type(), MarketType::Spot);
        mock.push_json(
            200,
            r#"{"lastPrice":"20000.50","bidPrice":"20000.00","askPrice":"20001.00","volume":"1234.5"}"#,
        );
        let ticker = binance.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.50));
        assert_eq!(ticker.bid, dec!(20000.00));
        assert_eq!(ticker.ask, dec!(20001.00));
        assert_eq!(ticker.volume, dec!(1234.5));

        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://api.binance.com/api/v3/ticker/24hr?symbol=BTCUSDT"
        );
    }

    #[test]
    // The kline fields parse from exact decimal strings, so an exact f64 compare
    // is correct here.
    #[allow(clippy::float_cmp)]
    fn klines_parse_into_candles() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(
            200,
            r#"[[1499040000000,"100.0","110.0","95.0","105.0","12.5",1499040059999,"0",1,"0","0","0"]]"#,
        );
        let candles = binance.klines(&symbol(), "1h", 1).unwrap();
        assert_eq!(candles.len(), 1);
        let c = candles[0];
        assert_eq!(c.open, 100.0);
        assert_eq!(c.high, 110.0);
        assert_eq!(c.low, 95.0);
        assert_eq!(c.close, 105.0);
        assert_eq!(c.volume, 12.5);
        assert_eq!(c.timestamp, 1_499_040_000_000);
    }

    #[test]
    fn order_book_parses_levels() {
        let (binance, mock) = client(MarketType::Spot, true);
        mock.push_json(
            200,
            r#"{"lastUpdateId":42,"bids":[["100.0","1.5"],["99.0","2.0"]],"asks":[["101.0","1.0"]]}"#,
        );
        let book = binance.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 42);
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100.0), dec!(1.5)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101.0), dec!(1.0)));
        // Testnet base.
        let req = &mock.recorded_requests()[0];
        assert!(req
            .url
            .starts_with("https://testnet.binance.vision/api/v3/depth"));
    }

    #[test]
    fn invalid_symbol_error_is_mapped() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(400, r#"{"code":-1121,"msg":"Invalid symbol."}"#);
        assert!(matches!(
            binance.ticker(&symbol()).unwrap_err(),
            Error::InvalidSymbol(_)
        ));
    }

    #[test]
    fn error_taxonomy_mapping() {
        let cases = [
            (r#"{"code":-2010,"msg":"x"}"#, "balance"),
            (r#"{"code":-1003,"msg":"x"}"#, "rate"),
            (r#"{"code":-2015,"msg":"x"}"#, "auth"),
            (r#"{"code":-9999,"msg":"weird"}"#, "exchange"),
        ];
        for (body, kind) in cases {
            let (binance, mock) = client(MarketType::Spot, false);
            mock.push_json(400, body);
            let err = binance.ticker(&symbol()).unwrap_err();
            match kind {
                "balance" => assert!(matches!(err, Error::InsufficientBalance)),
                "rate" => assert!(matches!(err, Error::RateLimited { .. })),
                "auth" => assert!(matches!(err, Error::Auth(_))),
                _ => assert!(matches!(err, Error::Exchange { .. })),
            }
        }
    }

    #[test]
    fn non_json_error_body_falls_back_to_exchange() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(502, "<html>bad gateway</html>");
        assert!(matches!(
            binance.ticker(&symbol()).unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn short_kline_row_is_rejected() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(200, r#"[[1499040000000,"100.0"]]"#);
        assert!(matches!(
            binance.klines(&symbol(), "1h", 1).unwrap_err(),
            Error::Deserialization(_)
        ));
    }
}
