//! Bybit (v5 unified API) — the second exchange, proving the pattern scales.
//!
//! Like Binance it is generic over the injected [`HttpTransport`] and tested
//! offline against recorded responses. The shape is the same; the internals are
//! bespoke: Bybit wraps every response in a `{retCode, retMsg, result}` envelope,
//! uses a `category` (spot/linear/inverse) query parameter, reports klines
//! newest-first, and (in a later slice) signs with `timestamp + apiKey +
//! recvWindow + payload` rather than a signed query string.
//!
//! Covered here: the public REST market data (ticker, klines, depth), the
//! `{retCode, retMsg, result}` envelope handling, the error taxonomy, and
//! `X-BAPI-*`-header signed execution (place/cancel/query order).

// Bybit `retCode`s are externally-defined numeric codes; grouping their digits
// with underscores would obscure them rather than aid reading.
#![allow(clippy::unreadable_literal)]

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookLevel, OrderBookSnapshot};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarketType};
use crate::signing::hmac_sha256_hex;
use crate::symbol::Symbol;
use crate::transport::{HttpMethod, HttpRequest, HttpTransport};
use crate::types::{Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker, TimeInForce};
use rust_decimal::Decimal;
use serde::Deserialize;
use wickra_core::Candle;

/// The current Unix time in milliseconds, from the system clock.
fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// A Bybit client over an injected HTTP transport.
pub struct Bybit {
    http: Box<dyn HttpTransport>,
    rest_base: String,
    category: &'static str,
    credentials: Option<Credentials>,
    recv_window_ms: u64,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl Bybit {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            rest_base: if options.testnet {
                "https://api-testnet.bybit.com".to_string()
            } else {
                "https://api.bybit.com".to_string()
            },
            category: category(options.market_type),
            credentials,
            recv_window_ms: options.recv_window_ms,
            now_ms: Box::new(system_now_ms),
        }
    }

    /// Build a public Bybit client over the given transport and options.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Bybit client for signed endpoints.
    #[must_use]
    pub fn with_credentials(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Credentials,
    ) -> Self {
        Self::build(http, options, Some(credentials))
    }

    /// Override the timestamp source (used for deterministic signing in tests).
    #[must_use]
    pub fn with_clock(mut self, now_ms: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now_ms = now_ms;
        self
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

    /// Place an order. Validated locally, then sent signed. Bybit's create
    /// endpoint returns only the ids, so the resulting [`Order`] carries the
    /// request's own fields with the venue order id and a `New` status.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let time_in_force = if request.post_only {
            "PostOnly"
        } else {
            tif_str(request.time_in_force)
        };
        let mut body = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "orderType": order_type_str(request.order_type),
            "qty": format_decimal(request.quantity),
            "timeInForce": time_in_force,
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            body["orderLinkId"] = serde_json::json!(id.clone());
        }
        if request.reduce_only {
            body["reduceOnly"] = serde_json::json!(true);
        }
        let result =
            self.signed_request(HttpMethod::Post, "/v5/order/create", "", &body.to_string())?;
        let created: CreateResult = parse_result(result)?;
        Ok(Order {
            id: created.order_id,
            client_order_id: (!created.order_link_id.is_empty()).then_some(created.order_link_id),
            symbol: request.symbol.clone(),
            side: request.side,
            order_type: request.order_type,
            status: OrderStatus::New,
            quantity: request.quantity,
            filled_quantity: Decimal::ZERO,
            price: request.price,
            average_price: None,
        })
    }

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the venue rejects it.
    pub fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let body = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "orderId": order_id,
        });
        self.signed_request(HttpMethod::Post, "/v5/order/cancel", "", &body.to_string())?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let query = format!(
            "category={}&symbol={}&orderId={order_id}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.signed_request(HttpMethod::Get, "/v5/order/realtime", &query, "")?;
        let list: OrderList = parse_result(result)?;
        let raw = list
            .list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// GET a public endpoint and unwrap the `{retCode, retMsg, result}` envelope.
    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_envelope(&response.body)
    }

    /// Sign a request with the Bybit `X-BAPI-*` header scheme: HMAC-SHA256 over
    /// `timestamp + apiKey + recvWindow + (query for GET, body for POST)`.
    fn signed_request(
        &self,
        method: HttpMethod,
        path: &str,
        query: &str,
        body: &str,
    ) -> Result<serde_json::Value> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let timestamp = (self.now_ms)().to_string();
        let recv_window = self.recv_window_ms.to_string();
        let payload = if body.is_empty() { query } else { body };
        let sign_input = format!("{timestamp}{}{recv_window}{payload}", creds.api_key);
        let signature = hmac_sha256_hex(creds.api_secret.as_bytes(), sign_input.as_bytes());
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let mut request = HttpRequest::new(method, url)
            .with_header("X-BAPI-API-KEY", creds.api_key.clone())
            .with_header("X-BAPI-TIMESTAMP", timestamp)
            .with_header("X-BAPI-RECV-WINDOW", recv_window)
            .with_header("X-BAPI-SIGN", signature);
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_envelope(&response.body)
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

fn unwrap_envelope(body: &str) -> Result<serde_json::Value> {
    // Bybit returns HTTP 200 even for API errors; the retCode carries the real
    // status, so parse the envelope regardless of the HTTP code.
    let envelope: Envelope =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if envelope.ret_code != 0 {
        return Err(map_error(envelope.ret_code, &envelope.ret_msg));
    }
    Ok(envelope.result)
}

fn parse_result<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "Buy",
        OrderSide::Sell => "Sell",
    }
}

fn order_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market | OrderType::StopMarket => "Market",
        OrderType::Limit | OrderType::StopLimit => "Limit",
    }
}

fn tif_str(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "Buy" => Ok(OrderSide::Buy),
        "Sell" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    match raw {
        "Market" => Ok(OrderType::Market),
        "Limit" => Ok(OrderType::Limit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "New" | "Untriggered" | "Triggered" => Ok(OrderStatus::New),
        "PartiallyFilled" => Ok(OrderStatus::PartiallyFilled),
        "Filled" => Ok(OrderStatus::Filled),
        "Cancelled" | "PartiallyFilledCanceled" | "Deactivated" => Ok(OrderStatus::Canceled),
        "Rejected" => Ok(OrderStatus::Rejected),
        other => Err(Error::Deserialization(format!("unknown status {other:?}"))),
    }
}

fn dec_or_zero(raw: &str) -> Decimal {
    crate::normalize::parse_opt_decimal(Some(raw))
        .ok()
        .flatten()
        .unwrap_or(Decimal::ZERO)
}

fn nonzero_decimal(raw: &str) -> Option<Decimal> {
    crate::normalize::parse_opt_decimal(Some(raw))
        .ok()
        .flatten()
        .filter(|d| *d > Decimal::ZERO)
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    Ok(Order {
        id: raw.order_id.clone(),
        client_order_id: (!raw.order_link_id.is_empty()).then(|| raw.order_link_id.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status: parse_status(&raw.order_status)?,
        quantity: parse_decimal(&raw.qty)?,
        filled_quantity: dec_or_zero(&raw.cum_exec_qty),
        price: nonzero_decimal(&raw.price),
        average_price: nonzero_decimal(&raw.avg_price),
    })
}

#[derive(Deserialize)]
struct CreateResult {
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "orderLinkId", default)]
    order_link_id: String,
}

#[derive(Deserialize)]
struct OrderList {
    list: Vec<RawOrder>,
}

#[derive(Deserialize)]
struct RawOrder {
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "orderLinkId", default)]
    order_link_id: String,
    side: String,
    #[serde(rename = "orderType")]
    order_type: String,
    #[serde(rename = "orderStatus")]
    order_status: String,
    qty: String,
    #[serde(rename = "cumExecQty", default)]
    cum_exec_qty: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "avgPrice", default)]
    avg_price: String,
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

    fn signed_client(now_ms: i64) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let bybit = Bybit::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (bybit, mock)
    }

    fn header<'a>(req: &'a HttpRequest, name: &str) -> &'a str {
        req.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    #[test]
    fn place_order_signs_with_bapi_headers() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"1739","orderLinkId":"abc"}}"#,
        );
        let order = bybit
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "1739");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.symbol, symbol());

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let body = req.body.as_ref().unwrap();
        let ts = header(req, "X-BAPI-TIMESTAMP");
        let recv = header(req, "X-BAPI-RECV-WINDOW");
        assert_eq!(ts, "1000");
        let expected = hmac_sha256_hex(b"SECRET", format!("{ts}APIKEY{recv}{body}").as_bytes());
        assert_eq!(header(req, "X-BAPI-SIGN"), expected);
        assert_eq!(header(req, "X-BAPI-API-KEY"), "APIKEY");
        assert!(body.contains(r#""side":"Buy""#));
        assert!(body.contains(r#""orderLinkId":"abc""#));
    }

    #[test]
    fn cancel_order_posts_signed() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(200, r#"{"retCode":0,"result":{"orderId":"1739"}}"#);
        bybit.cancel_order(&symbol(), "1739").unwrap();
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        assert!(req.url.ends_with("/v5/order/cancel"));
        assert!(req.body.as_ref().unwrap().contains(r#""orderId":"1739""#));
    }

    #[test]
    fn query_order_parses_realtime_list() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"orderId":"1739","orderLinkId":"",
            "symbol":"BTCUSDT","side":"Sell","orderType":"Market","orderStatus":"Filled",
            "qty":"2","cumExecQty":"2","price":"0","avgPrice":"100"}]}}"#,
        );
        let order = bybit.query_order(&symbol(), "1739").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.filled_quantity, dec!(2));
        assert_eq!(order.average_price, Some(dec!(100)));
        assert_eq!(order.price, None);
        assert_eq!(order.client_order_id, None);
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Get);
        assert!(req.url.contains("orderId=1739"));
        assert!(req.headers.iter().any(|(k, _)| k == "X-BAPI-SIGN"));
    }

    #[test]
    fn query_missing_order_is_not_found() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(200, r#"{"retCode":0,"result":{"list":[]}}"#);
        assert!(matches!(
            bybit.query_order(&symbol(), "x").unwrap_err(),
            Error::NotFound(_)
        ));
    }

    #[test]
    fn signed_without_credentials_errors() {
        let (bybit, _) = client(MarketType::Spot, false);
        assert!(matches!(
            bybit
                .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }
}
