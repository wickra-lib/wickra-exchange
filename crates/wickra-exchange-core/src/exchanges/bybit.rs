//! Bybit (v5 unified API) — the second exchange, proving the pattern scales.
//!
//! Like Binance it is generic over the injected [`HttpTransport`] and tested
//! offline against recorded responses. The shape is the same; the internals are
//! bespoke: Bybit wraps every response in a `{retCode, retMsg, result}` envelope,
//! uses a `category` (spot/linear/inverse) query parameter, reports klines
//! newest-first, and (in a later slice) signs with `timestamp + apiKey +
//! recvWindow + payload` rather than a signed query string.
//!
//! This first slice covers the public REST market data (ticker, klines, depth),
//! the envelope handling and the error taxonomy.

// Bybit `retCode`s are externally-defined numeric codes; grouping their digits
// with underscores would obscure them rather than aid reading.
#![allow(clippy::unreadable_literal)]

use crate::error::{Error, Result};
use crate::events::{BookLevel, OrderBookSnapshot};
use crate::normalize::parse_decimal;
use crate::options::{ExchangeOptions, MarketType};
use crate::symbol::Symbol;
use crate::transport::{HttpRequest, HttpTransport};
use crate::types::Ticker;
use serde::Deserialize;
use wickra_core::Candle;

/// A Bybit client over an injected HTTP transport.
pub struct Bybit {
    http: Box<dyn HttpTransport>,
    rest_base: String,
    category: &'static str,
}

impl Bybit {
    /// Build a public Bybit client over the given transport and options.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self {
            http,
            rest_base: if options.testnet {
                "https://api-testnet.bybit.com".to_string()
            } else {
                "https://api.bybit.com".to_string()
            },
            category: category(options.market_type),
        }
    }

    /// The Bybit product category this client targets (`spot`/`linear`/`inverse`).
    #[must_use]
    pub fn category(&self) -> &'static str {
        self.category
    }

    /// The Bybit wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTCUSDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated()
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!(
            "category={}&symbol={}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.get("/v5/market/tickers", &query)?;
        let raw: TickerList =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        let entry = raw
            .list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last_price)?,
            bid: parse_decimal(&entry.bid1_price)?,
            ask: parse_decimal(&entry.ask1_price)?,
            volume: parse_decimal(&entry.volume24h)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified, e.g. `"1m"`,
    /// `"1h"`, `"1d"`). Bybit returns newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "category={}&symbol={}&interval={}&limit={limit}",
            self.category,
            Self::wire_symbol(symbol),
            map_interval(interval),
        );
        let result = self.get("/v5/market/kline", &query)?;
        let raw: KlineList =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        let mut candles = raw
            .list
            .iter()
            .map(|row| parse_kline_row(row))
            .collect::<Result<Vec<_>>>()?;
        candles.reverse();
        Ok(candles)
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!(
            "category={}&symbol={}&limit={depth}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.get("/v5/market/orderbook", &query)?;
        let raw: RawDepth =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.update_id,
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// GET a public endpoint and unwrap the `{retCode, retMsg, result}` envelope.
    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        // Bybit returns HTTP 200 even for API errors; the retCode carries the
        // real status, so parse the envelope regardless of the HTTP code.
        let envelope: Envelope = serde_json::from_str(&response.body)
            .map_err(|e| Error::Deserialization(e.to_string()))?;
        if envelope.ret_code != 0 {
            return Err(map_error(envelope.ret_code, &envelope.ret_msg));
        }
        Ok(envelope.result)
    }
}

/// The Bybit product category for a market type.
fn category(market_type: MarketType) -> &'static str {
    match market_type {
        MarketType::Spot | MarketType::Margin => "spot",
        MarketType::UsdMFutures => "linear",
        MarketType::CoinMFutures => "inverse",
    }
}

/// Map a unified interval (`1m`/`1h`/`1d`) to Bybit's format (`1`/`60`/`D`).
fn map_interval(interval: &str) -> String {
    match interval {
        "1m" => "1",
        "3m" => "3",
        "5m" => "5",
        "15m" => "15",
        "30m" => "30",
        "1h" => "60",
        "2h" => "120",
        "4h" => "240",
        "6h" => "360",
        "12h" => "720",
        "1d" => "D",
        "1w" => "W",
        "1M" => "M",
        other => other,
    }
    .to_string()
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "retCode")]
    ret_code: i64,
    #[serde(rename = "retMsg", default)]
    ret_msg: String,
    #[serde(default)]
    result: serde_json::Value,
}

#[derive(Deserialize)]
struct TickerList {
    list: Vec<RawTicker>,
}

#[derive(Deserialize)]
struct RawTicker {
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "bid1Price")]
    bid1_price: String,
    #[serde(rename = "ask1Price")]
    ask1_price: String,
    #[serde(rename = "volume24h")]
    volume24h: String,
}

#[derive(Deserialize)]
struct KlineList {
    list: Vec<Vec<String>>,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(rename = "u")]
    update_id: u64,
    #[serde(rename = "b")]
    bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    asks: Vec<[String; 2]>,
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

fn parse_kline_row(row: &[String]) -> Result<Candle> {
    // Bybit kline: [startTime, open, high, low, close, volume, turnover].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let start = row[0]
        .parse::<i64>()
        .map_err(|e| Error::Deserialization(format!("kline start not an integer: {e}")))?;
    let f = |i: usize| -> Result<f64> {
        row[i]
            .parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
    };
    Candle::new(f(1)?, f(2)?, f(3)?, f(4)?, f(5)?, start)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

/// Map a Bybit `retCode` onto the unified error taxonomy.
fn map_error(ret_code: i64, ret_msg: &str) -> Error {
    match ret_code {
        10001 | 10004 | 10005 | 33004 => Error::Auth(ret_msg.to_string()),
        10006 | 10018 => Error::RateLimited { retry_after: None },
        110004 | 110007 | 170131 => Error::InsufficientBalance,
        110001 | 170213 => Error::NotFound(ret_msg.to_string()),
        _ => Error::Exchange {
            code: ret_code.to_string(),
            message: ret_msg.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{HttpResponse, MockHttpTransport};
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    struct ArcTransport(Arc<MockHttpTransport>);
    impl HttpTransport for ArcTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.0.execute(request)
        }
    }

    fn symbol() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    fn client(market_type: MarketType, testnet: bool) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = if testnet {
            ExchangeOptions::testnet(market_type)
        } else {
            ExchangeOptions::mainnet(market_type)
        };
        let bybit = Bybit::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts);
        (bybit, mock)
    }

    #[test]
    fn category_by_market_type() {
        assert_eq!(category(MarketType::Spot), "spot");
        assert_eq!(category(MarketType::UsdMFutures), "linear");
        assert_eq!(category(MarketType::CoinMFutures), "inverse");
        assert_eq!(category(MarketType::Margin), "spot");
    }

    #[test]
    fn interval_mapping() {
        assert_eq!(map_interval("1m"), "1");
        assert_eq!(map_interval("1h"), "60");
        assert_eq!(map_interval("4h"), "240");
        assert_eq!(map_interval("1d"), "D");
        assert_eq!(map_interval("weird"), "weird");
    }

    #[test]
    fn ticker_unwraps_envelope_and_targets_url() {
        let (bybit, mock) = client(MarketType::Spot, false);
        assert_eq!(bybit.category(), "spot");
        mock.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"symbol":"BTCUSDT",
            "lastPrice":"20000.5","bid1Price":"20000.0","ask1Price":"20001.0","volume24h":"1234.5"}]}}"#,
        );
        let ticker = bybit.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(20000.0));
        assert_eq!(ticker.ask, dec!(20001.0));
        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://api.bybit.com/v5/market/tickers?category=spot&symbol=BTCUSDT"
        );
    }

    #[test]
    fn klines_are_reversed_to_chronological() {
        let (bybit, mock) = client(MarketType::Spot, false);
        // Bybit returns newest-first.
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[
            ["1700000060000","105","106","104","105.5","2","0"],
            ["1700000000000","100","110","95","105","12.5","0"]]}}"#,
        );
        let candles = bybit.klines(&symbol(), "1m", 2).unwrap();
        assert_eq!(candles.len(), 2);
        // Oldest first after reversing.
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert_eq!(candles[1].timestamp, 1_700_000_060_000);
    }

    #[test]
    fn order_book_parses_levels() {
        let (bybit, mock) = client(MarketType::UsdMFutures, true);
        assert_eq!(bybit.category(), "linear");
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"s":"BTCUSDT","u":77,
            "b":[["100.0","1.5"]],"a":[["101.0","2.0"]]}}"#,
        );
        let book = bybit.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 77);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100.0), dec!(1.5)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101.0), dec!(2.0)));
        assert!(mock.recorded_requests()[0]
            .url
            .starts_with("https://api-testnet.bybit.com/v5/market/orderbook"));
    }

    #[test]
    fn error_envelope_maps_to_taxonomy() {
        let cases = [
            (10004, "sign"),
            (10006, "rate"),
            (170131, "balance"),
            (110001, "notfound"),
            (99999, "exchange"),
        ];
        for (code, kind) in cases {
            let (bybit, mock) = client(MarketType::Spot, false);
            mock.push_json(
                200,
                format!(r#"{{"retCode":{code},"retMsg":"x","result":{{}}}}"#),
            );
            let err = bybit.ticker(&symbol()).unwrap_err();
            match kind {
                "sign" => assert!(matches!(err, Error::Auth(_))),
                "rate" => assert!(matches!(err, Error::RateLimited { .. })),
                "balance" => assert!(matches!(err, Error::InsufficientBalance)),
                "notfound" => assert!(matches!(err, Error::NotFound(_))),
                _ => assert!(matches!(err, Error::Exchange { .. })),
            }
        }
    }

    #[test]
    fn empty_ticker_list_is_not_found() {
        let (bybit, mock) = client(MarketType::Spot, false);
        mock.push_json(200, r#"{"retCode":0,"result":{"list":[]}}"#);
        assert!(matches!(
            bybit.ticker(&symbol()).unwrap_err(),
            Error::NotFound(_)
        ));
    }
}
