//! Binance — the reference exchange implementation.
//!
//! This module is generic over the injected [`HttpTransport`], so the entire
//! request-build → parse → normalise path is exercised offline against
//! [`MockHttpTransport`] with recorded Binance responses. Only the production
//! wiring of a real socket lives elsewhere.
//!
//! Covered here: the public REST market data (ticker, klines, depth), the
//! URL/symbol mapping, the Binance error taxonomy, HMAC-SHA256 signed execution
//! (place/cancel/query/open orders, balances) with `exchangeInfo` filter
//! validation, and the pull-based WebSocket market streams (trade/depth/ticker
//! subscribe + `poll_events`). The user-data stream (listenKey) and the real
//! socket adapter land in a later slice.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::instruments::{Instrument, InstrumentCache, InstrumentFilters};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarketType};
use crate::signing::hmac_sha256_hex;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
use crate::types::{
    Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker, TimeInForce,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use wickra_core::Candle;

/// The current Unix time in milliseconds, from the system clock.
fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// A Binance client over an injected HTTP transport.
pub struct Binance {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    market_type: MarketType,
    testnet: bool,
    credentials: Option<Credentials>,
    recv_window_ms: u64,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    subscriptions: Vec<(String, Symbol)>,
    sub_id: u64,
    instruments: InstrumentCache,
}

impl Binance {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: rest_base_url(options.market_type, options.testnet).to_string(),
            market_type: options.market_type,
            testnet: options.testnet,
            credentials,
            recv_window_ms: options.recv_window_ms,
            now_ms: Box::new(system_now_ms),
            connection: None,
            subscriptions: Vec::new(),
            sub_id: 0,
            instruments: InstrumentCache::new(),
        }
    }

    /// Build a public (unauthenticated) Binance client over the given transport.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Binance client for signed endpoints.
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

    /// Attach a WebSocket transport, enabling the streaming subscriptions.
    #[must_use]
    pub fn with_ws(mut self, ws: Box<dyn WsTransport>) -> Self {
        self.ws = Some(ws);
        self
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

    /// Fetch `exchangeInfo` and populate the instrument/filter cache, so that
    /// [`place_order`](Self::place_order) validates against the venue's per-symbol
    /// filters (lot size, price tick, min-notional).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn load_instruments(&mut self) -> Result<()> {
        let body = self.get("/api/v3/exchangeInfo", "")?;
        let raw: RawExchangeInfo = deserialize(&body)?;
        let now = (self.now_ms)();
        let instruments: Vec<Instrument> = raw.symbols.iter().map(parse_instrument).collect();
        self.instruments.replace(instruments, now);
        Ok(())
    }

    /// The cached instrument metadata for `symbol`, if [`load_instruments`](Self::load_instruments)
    /// has been called.
    #[must_use]
    pub fn instrument(&self, symbol: &Symbol) -> Option<&Instrument> {
        self.instruments.get(symbol)
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured,
    /// or a transport error if the connection or subscription fails.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "trade")
    }

    /// Subscribe to the order-book (diff-depth) stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "depth")
    }

    /// Subscribe to the 24-hour ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    /// Open the connection if needed, send a SUBSCRIBE for `<symbol>@<channel>`,
    /// and register the symbol for wire-name resolution.
    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect(ws_base_url(self.market_type, self.testnet))?;
            self.connection = Some(connection);
        }
        self.sub_id += 1;
        let stream = format!("{}@{channel}", wire.to_lowercase());
        let message = format!(
            r#"{{"method":"SUBSCRIBE","params":["{stream}"],"id":{}}}"#,
            self.sub_id
        );
        self.connection
            .as_mut()
            .expect("connection just ensured")
            .send(&message)?;
        if !self.subscriptions.iter().any(|(w, _)| w == &wire) {
            self.subscriptions.push((wire, symbol.clone()));
        }
        Ok(())
    }

    /// Drain all stream events available since the last call. Non-blocking:
    /// returns an empty vector when nothing is pending or no stream is open.
    /// Frames that fail to parse are skipped.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let subscriptions: HashMap<String, Symbol> = self.subscriptions.iter().cloned().collect();
        let resolve = |wire: &str| {
            subscriptions
                .get(wire)
                .cloned()
                .unwrap_or_else(|| Symbol::new(wire, ""))
        };
        let mut events = Vec::new();
        let Some(connection) = self.connection.as_mut() else {
            return events;
        };
        while let Ok(Some(frame)) = connection.recv() {
            if let Ok(Some(event)) = parse_ws_message(&frame, &resolve) {
                events.push(event);
            }
        }
        events
    }

    /// Place an order. The order is validated locally first, then sent signed.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        // When exchangeInfo has been loaded, reject filter violations before the
        // round trip.
        if let Some(instrument) = self.instruments.get(&request.symbol) {
            instrument
                .filters
                .validate(request.quantity, request.price)?;
        }
        let type_str = if request.post_only && request.order_type == OrderType::Limit {
            "LIMIT_MAKER"
        } else {
            order_type_str(request.order_type)
        };
        let mut params = format!(
            "symbol={}&side={}&type={type_str}&quantity={}",
            Self::wire_symbol(&request.symbol),
            side_str(request.side),
            format_decimal(request.quantity),
        );
        if let Some(price) = request.price {
            params.push_str("&price=");
            params.push_str(&format_decimal(price));
        }
        if let Some(stop) = request.stop_price {
            params.push_str("&stopPrice=");
            params.push_str(&format_decimal(stop));
        }
        if matches!(type_str, "LIMIT" | "STOP_LOSS_LIMIT" | "TAKE_PROFIT_LIMIT") {
            params.push_str("&timeInForce=");
            params.push_str(tif_str(request.time_in_force));
        }
        if let Some(id) = &request.client_order_id {
            params.push_str("&newClientOrderId=");
            params.push_str(id);
        }
        if request.reduce_only {
            params.push_str("&reduceOnly=true");
        }
        let body = self.signed_request(HttpMethod::Post, "/api/v3/order", &params)?;
        parse_order(&request.symbol, &body)
    }

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the venue rejects it.
    pub fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let params = format!("symbol={}&orderId={order_id}", Self::wire_symbol(symbol));
        self.signed_request(HttpMethod::Delete, "/api/v3/order", &params)?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let params = format!("symbol={}&orderId={order_id}", Self::wire_symbol(symbol));
        let body = self.signed_request(HttpMethod::Get, "/api/v3/order", &params)?;
        parse_order(symbol, &body)
    }

    /// Account balances (assets with a non-zero free or locked amount are
    /// included as the venue reports them).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let body = self.signed_request(HttpMethod::Get, "/api/v3/account", "")?;
        let raw: RawAccount = deserialize(&body)?;
        raw.balances
            .iter()
            .map(|b| {
                Ok(Balance {
                    asset: b.asset.clone(),
                    free: parse_decimal(&b.free)?,
                    locked: parse_decimal(&b.locked)?,
                })
            })
            .collect()
    }

    /// All open orders, optionally filtered to one `symbol`. When unfiltered, the
    /// venue reports each order's wire symbol, which is mapped back to a canonical
    /// [`Symbol`] via the known quote-asset suffixes.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let params = match symbol {
            Some(s) => format!("symbol={}", Self::wire_symbol(s)),
            None => String::new(),
        };
        let body = self.signed_request(HttpMethod::Get, "/api/v3/openOrders", &params)?;
        let raws: Vec<RawOrder> = deserialize(&body)?;
        raws.iter()
            .map(|raw| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| split_wire_symbol(&raw.symbol));
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Issue a GET and return the body, mapping non-2xx responses onto the error
    /// taxonomy.
    fn get(&self, path: &str, query: &str) -> Result<String> {
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let response = self.http.execute(&HttpRequest::get(url))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(map_error(&response))
        }
    }

    /// Sign `params` (HMAC-SHA256 over the query with `recvWindow` + `timestamp`)
    /// and issue the request with the API-key header.
    fn signed_request(&self, method: HttpMethod, path: &str, params: &str) -> Result<String> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let timestamp = (self.now_ms)();
        let payload = if params.is_empty() {
            format!("recvWindow={}&timestamp={timestamp}", self.recv_window_ms)
        } else {
            format!(
                "{params}&recvWindow={}&timestamp={timestamp}",
                self.recv_window_ms
            )
        };
        let signature = hmac_sha256_hex(creds.api_secret.as_bytes(), payload.as_bytes());
        let url = format!("{}{path}?{payload}&signature={signature}", self.rest_base);
        let request =
            HttpRequest::new(method, url).with_header("X-MBX-APIKEY", creds.api_key.clone());
        let response = self.http.execute(&request)?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(map_error(&response))
        }
    }
}

// The trait surface delegates to the inherent methods (fully qualified to avoid
// resolving back to the trait method), giving a `Box<dyn Exchange>` for the factory.
impl MarketData for Binance {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Binance::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Binance::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Binance::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Binance::poll_events(self)
    }
}

impl Execution for Binance {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Binance::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Binance::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Binance::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Binance::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Binance::balances(self)
    }
}

impl Exchange for Binance {
    fn name(&self) -> &'static str {
        "binance"
    }
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    }
}

fn order_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market => "MARKET",
        OrderType::Limit => "LIMIT",
        OrderType::StopMarket => "STOP_LOSS",
        OrderType::StopLimit => "STOP_LOSS_LIMIT",
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
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    match raw {
        "MARKET" => Ok(OrderType::Market),
        "LIMIT" | "LIMIT_MAKER" => Ok(OrderType::Limit),
        "STOP_LOSS" | "TAKE_PROFIT" => Ok(OrderType::StopMarket),
        "STOP_LOSS_LIMIT" | "TAKE_PROFIT_LIMIT" => Ok(OrderType::StopLimit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "NEW" => Ok(OrderStatus::New),
        "PARTIALLY_FILLED" => Ok(OrderStatus::PartiallyFilled),
        "FILLED" => Ok(OrderStatus::Filled),
        "CANCELED" | "PENDING_CANCEL" => Ok(OrderStatus::Canceled),
        "REJECTED" => Ok(OrderStatus::Rejected),
        "EXPIRED" | "EXPIRED_IN_MATCH" => Ok(OrderStatus::Expired),
        other => Err(Error::Deserialization(format!("unknown status {other:?}"))),
    }
}

fn parse_order(symbol: &Symbol, body: &str) -> Result<Order> {
    let raw: RawOrder = deserialize(body)?;
    order_from_raw(symbol.clone(), &raw)
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    let executed = parse_decimal(&raw.executed_qty)?;
    let average_price = if executed > Decimal::ZERO {
        Some(parse_decimal(&raw.cummulative_quote_qty)? / executed)
    } else {
        None
    };
    let parsed_price = parse_decimal(&raw.price)?;
    let price = (parsed_price > Decimal::ZERO).then_some(parsed_price);
    Ok(Order {
        id: raw.order_id.to_string(),
        client_order_id: (!raw.client_order_id.is_empty()).then(|| raw.client_order_id.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status: parse_status(&raw.status)?,
        quantity: parse_decimal(&raw.orig_qty)?,
        filled_quantity: executed,
        price,
        average_price,
    })
}

/// Quote assets used to split a concatenated wire symbol (`BTCUSDT` -> `BTC/USDT`)
/// when the venue reports only the wire form. Longer quotes are tried first.
const KNOWN_QUOTES: &[&str] = &[
    "FDUSD", "USDT", "USDC", "TUSD", "BUSD", "DAI", "EUR", "TRY", "BTC", "ETH", "BNB", "USD",
];

/// Map a concatenated Binance wire symbol back to a canonical [`Symbol`] using
/// the known quote-asset suffixes. Falls back to the whole string as the base.
fn split_wire_symbol(wire: &str) -> Symbol {
    for quote in KNOWN_QUOTES {
        if let Some(base) = wire.strip_suffix(quote) {
            if !base.is_empty() {
                return Symbol::new(base, *quote);
            }
        }
    }
    Symbol::new(wire, "")
}

fn field_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization(format!("missing string field {key:?}")))
}

fn parse_ws_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
    let array = value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Deserialization("missing depth levels".to_string()))?;
    array
        .iter()
        .map(|level| {
            let pair = level
                .as_array()
                .ok_or_else(|| Error::Deserialization("depth level not an array".to_string()))?;
            let price = parse_decimal(
                pair.first()
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::Deserialization("depth price missing".to_string()))?,
            )?;
            let quantity =
                parse_decimal(pair.get(1).and_then(serde_json::Value::as_str).ok_or_else(
                    || Error::Deserialization("depth quantity missing".to_string()),
                )?)?;
            Ok(BookLevel { price, quantity })
        })
        .collect()
}

/// Parse one Binance WebSocket frame into an [`Event`], resolving the wire
/// symbol with `resolve`. Non-data frames (subscription acks) and unhandled
/// event types return `Ok(None)`.
fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Option<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    // A combined stream wraps the payload as {"stream":..,"data":..}.
    let data = value.get("data").unwrap_or(&value);
    let Some(event_type) = data.get("e").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    match event_type {
        "trade" => {
            // `m` = "is the buyer the market maker?"; if so the taker (aggressor)
            // is the seller.
            let is_maker_buyer = data.get("m").and_then(serde_json::Value::as_bool) == Some(true);
            Ok(Some(Event::Trade(TradePrint {
                symbol: resolve(field_str(data, "s")?),
                price: parse_decimal(field_str(data, "p")?)?,
                quantity: parse_decimal(field_str(data, "q")?)?,
                aggressor: if is_maker_buyer {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                },
                timestamp: data
                    .get("T")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })))
        }
        "depthUpdate" => Ok(Some(Event::BookDelta(BookDelta {
            symbol: resolve(field_str(data, "s")?),
            first_update_id: data
                .get("U")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            final_update_id: data
                .get("u")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            bids: parse_ws_levels(data.get("b"))?,
            asks: parse_ws_levels(data.get("a"))?,
        }))),
        "24hrTicker" => Ok(Some(Event::Ticker(Ticker {
            symbol: resolve(field_str(data, "s")?),
            last: parse_decimal(field_str(data, "c")?)?,
            bid: parse_decimal(field_str(data, "b")?)?,
            ask: parse_decimal(field_str(data, "a")?)?,
            volume: parse_decimal(field_str(data, "v")?)?,
        }))),
        _ => Ok(None),
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

/// The WebSocket base URL for a market type and network.
fn ws_base_url(market_type: MarketType, testnet: bool) -> &'static str {
    match (market_type, testnet) {
        (MarketType::UsdMFutures, false) => "wss://fstream.binance.com/ws",
        (MarketType::UsdMFutures, true) => "wss://stream.binancefuture.com/ws",
        (_, true) => "wss://testnet.binance.vision/ws",
        (_, false) => "wss://stream.binance.com:9443/ws",
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

#[derive(Deserialize)]
struct RawOrder {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: u64,
    #[serde(rename = "clientOrderId", default)]
    client_order_id: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    status: String,
    #[serde(rename = "origQty")]
    orig_qty: String,
    #[serde(rename = "executedQty")]
    executed_qty: String,
    #[serde(rename = "cummulativeQuoteQty")]
    cummulative_quote_qty: String,
    price: String,
}

#[derive(Deserialize)]
struct RawAccount {
    balances: Vec<RawBalance>,
}

#[derive(Deserialize)]
struct RawBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Deserialize)]
struct RawExchangeInfo {
    symbols: Vec<RawSymbol>,
}

#[derive(Deserialize)]
struct RawSymbol {
    #[serde(rename = "baseAsset")]
    base_asset: String,
    #[serde(rename = "quoteAsset")]
    quote_asset: String,
    #[serde(rename = "baseAssetPrecision", default)]
    base_asset_precision: u32,
    #[serde(rename = "quoteAssetPrecision", default)]
    quote_asset_precision: u32,
    #[serde(default)]
    filters: Vec<serde_json::Value>,
}

fn find_filter<'a>(filters: &'a [serde_json::Value], kind: &str) -> Option<&'a serde_json::Value> {
    filters
        .iter()
        .find(|f| f.get("filterType").and_then(serde_json::Value::as_str) == Some(kind))
}

fn filter_decimal(filter: Option<&serde_json::Value>, key: &str) -> Decimal {
    filter
        .and_then(|f| f.get(key))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| parse_decimal(s).ok())
        .unwrap_or(Decimal::ZERO)
}

fn parse_instrument(raw: &RawSymbol) -> Instrument {
    let lot = find_filter(&raw.filters, "LOT_SIZE");
    let price = find_filter(&raw.filters, "PRICE_FILTER");
    let notional =
        find_filter(&raw.filters, "NOTIONAL").or_else(|| find_filter(&raw.filters, "MIN_NOTIONAL"));
    Instrument {
        symbol: Symbol::new(&raw.base_asset, &raw.quote_asset),
        base_precision: raw.base_asset_precision,
        quote_precision: raw.quote_asset_precision,
        filters: InstrumentFilters {
            min_quantity: filter_decimal(lot, "minQty"),
            max_quantity: filter_decimal(lot, "maxQty"),
            step_size: filter_decimal(lot, "stepSize"),
            min_price: filter_decimal(price, "minPrice"),
            max_price: filter_decimal(price, "maxPrice"),
            tick_size: filter_decimal(price, "tickSize"),
            min_notional: filter_decimal(notional, "minNotional"),
        },
    }
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
    use crate::transport::{MockHttpTransport, MockWsTransport};
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    /// A `WsTransport` that forwards to a shared `MockWsTransport`, so a test
    /// keeps a handle after the client takes ownership.
    struct ArcWs(Arc<MockWsTransport>);
    impl WsTransport for ArcWs {
        fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>> {
            self.0.connect(url)
        }
    }

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

    const ORDER_JSON: &str = r#"{"symbol":"BTCUSDT","orderId":28,"clientOrderId":"abc",
        "price":"100.00000000","origQty":"1.00000000","executedQty":"0.00000000",
        "cummulativeQuoteQty":"0.00000000","status":"NEW","type":"LIMIT","side":"BUY"}"#;

    /// An authenticated client over a mock transport with a fixed clock.
    fn signed_client(now_ms: i64) -> (Binance, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let binance = Binance::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (binance, mock)
    }

    #[test]
    fn place_order_signs_request_and_parses_response() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        let order = binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "28");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.quantity, dec!(1));
        assert_eq!(order.price, Some(dec!(100)));
        assert_eq!(order.average_price, None);

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let payload = "symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=1&price=100\
                       &timeInForce=GTC&recvWindow=5000&timestamp=1000";
        let sig = crate::signing::hmac_sha256_hex(b"SECRET", payload.as_bytes());
        assert!(req.url.contains(payload), "payload mismatch: {}", req.url);
        assert!(req.url.ends_with(&format!("signature={sig}")));
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "X-MBX-APIKEY" && v == "APIKEY"));
    }

    #[test]
    fn post_only_limit_becomes_limit_maker_without_tif() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).post_only())
            .unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("type=LIMIT_MAKER"));
        assert!(!req.url.contains("timeInForce"));
    }

    #[test]
    fn signed_endpoint_without_credentials_errors() {
        let (binance, _) = client(MarketType::Spot, false);
        let err = binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap_err();
        assert!(matches!(err, Error::InvalidCredentials(_)));
    }

    #[test]
    fn cancel_order_is_a_signed_delete() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","orderId":28,"status":"CANCELED"}"#,
        );
        binance.cancel_order(&symbol(), "28").unwrap();
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Delete);
        assert!(req.url.contains("orderId=28"));
        assert!(req.url.contains("signature="));
    }

    #[test]
    fn query_order_computes_average_fill_price() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","orderId":28,"clientOrderId":"","price":"0.00000000",
            "origQty":"2.0","executedQty":"2.0","cummulativeQuoteQty":"200.0",
            "status":"FILLED","type":"MARKET","side":"SELL"}"#,
        );
        let order = binance.query_order(&symbol(), "28").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.average_price, Some(dec!(100))); // 200 / 2
        assert_eq!(order.price, None); // 0 -> None
        assert_eq!(order.client_order_id, None); // empty -> None
        assert_eq!(order.filled_quantity, dec!(2));
    }

    #[test]
    fn balances_parse() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"balances":[{"asset":"USDT","free":"100.5","locked":"25.5"},
            {"asset":"BTC","free":"0.1","locked":"0"}]}"#,
        );
        let bals = binance.balances().unwrap();
        assert_eq!(bals.len(), 2);
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].total(), dec!(126));
        assert_eq!(bals[1].asset, "BTC");
    }

    #[test]
    fn system_clock_is_sane() {
        // Covers the production timestamp source: a plausible 2023+ epoch ms.
        assert!(system_now_ms() > 1_600_000_000_000);
    }

    fn resolve(_wire: &str) -> Symbol {
        symbol()
    }

    #[test]
    fn ws_trade_frame_maps_aggressor() {
        // m=false -> buyer is taker -> Buy aggressor.
        let buy = parse_ws_message(
            r#"{"e":"trade","s":"BTCUSDT","p":"100.5","q":"0.25","m":false,"T":1700000000000}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            buy,
            Event::Trade(TradePrint {
                symbol: symbol(),
                price: dec!(100.5),
                quantity: dec!(0.25),
                aggressor: OrderSide::Buy,
                timestamp: 1_700_000_000_000,
            })
        );
        // m=true -> seller is taker -> Sell aggressor.
        let sell = parse_ws_message(
            r#"{"e":"trade","s":"BTCUSDT","p":"100","q":"1","m":true,"T":1}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        let Event::Trade(print) = sell else {
            panic!("expected trade")
        };
        assert_eq!(print.aggressor, OrderSide::Sell);
    }

    #[test]
    fn ws_combined_stream_wrapper_is_unwrapped() {
        let event = parse_ws_message(
            r#"{"stream":"btcusdt@trade","data":{"e":"trade","s":"BTCUSDT","p":"1","q":"1","m":false,"T":1}}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(event, Event::Trade(_)));
    }

    #[test]
    fn ws_depth_update_maps_to_book_delta() {
        let event = parse_ws_message(
            r#"{"e":"depthUpdate","s":"BTCUSDT","U":10,"u":12,"b":[["100","1"],["99","0"]],"a":[["101","2"]]}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        let Event::BookDelta(delta) = event else {
            panic!("expected book delta")
        };
        assert_eq!(delta.first_update_id, 10);
        assert_eq!(delta.final_update_id, 12);
        assert_eq!(
            delta.bids,
            vec![
                BookLevel::new(dec!(100), dec!(1)),
                BookLevel::new(dec!(99), dec!(0))
            ]
        );
        assert_eq!(delta.asks, vec![BookLevel::new(dec!(101), dec!(2))]);
    }

    #[test]
    fn ws_ticker_frame_maps_to_ticker() {
        let event = parse_ws_message(
            r#"{"e":"24hrTicker","s":"BTCUSDT","c":"100","b":"99","a":"101","v":"1234"}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            event,
            Event::Ticker(Ticker {
                symbol: symbol(),
                last: dec!(100),
                bid: dec!(99),
                ask: dec!(101),
                volume: dec!(1234),
            })
        );
    }

    #[test]
    fn ws_non_event_frames_are_ignored() {
        // Subscription ack.
        assert!(parse_ws_message(r#"{"result":null,"id":1}"#, &resolve)
            .unwrap()
            .is_none());
        // Unhandled event type.
        assert!(parse_ws_message(r#"{"e":"kline","s":"BTCUSDT"}"#, &resolve)
            .unwrap()
            .is_none());
    }

    #[test]
    fn ws_malformed_frame_errors() {
        assert!(matches!(
            parse_ws_message("not json", &resolve).unwrap_err(),
            Error::Deserialization(_)
        ));
        // A trade frame missing a required field.
        assert!(parse_ws_message(r#"{"e":"trade","s":"BTCUSDT"}"#, &resolve).is_err());
    }

    fn streaming_client(ws: &Arc<MockWsTransport>) -> Binance {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        Binance::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(ws))))
    }

    #[test]
    fn subscribe_sends_frame_and_poll_returns_events() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"e":"trade","s":"BTCUSDT","p":"100","q":"1","m":false,"T":1}"#.to_string(),
            )),
            Ok(Some(
                r#"{"e":"trade","s":"BTCUSDT","p":"101","q":"2","m":true,"T":2}"#.to_string(),
            )),
        ]);
        let mut binance = streaming_client(&ws);
        binance.subscribe_trades(&symbol()).unwrap();

        assert_eq!(
            ws.connected_urls(),
            vec!["wss://stream.binance.com:9443/ws".to_string()]
        );
        let sent = ws.sent();
        assert!(sent[0].contains(r#""method":"SUBSCRIBE""#));
        assert!(sent[0].contains("btcusdt@trade"));

        let events = binance.poll_events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::Trade(_)));
        // Draining again yields nothing.
        assert!(binance.poll_events().is_empty());
    }

    #[test]
    fn book_and_ticker_subscriptions_reuse_one_connection() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![]);
        let mut binance = streaming_client(&ws);
        binance.subscribe_book(&symbol()).unwrap();
        binance.subscribe_ticker(&symbol()).unwrap();

        // One connection, two SUBSCRIBE frames on the right channels.
        assert_eq!(ws.connected_urls().len(), 1);
        let sent = ws.sent();
        assert!(sent[0].contains("btcusdt@depth"));
        assert!(sent[1].contains("btcusdt@ticker"));
    }

    #[test]
    fn subscribe_without_ws_transport_errors() {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut binance = Binance::with_http(Box::new(ArcTransport(http)), &opts);
        assert!(matches!(
            binance.subscribe_trades(&symbol()).unwrap_err(),
            Error::NotConnected
        ));
    }

    #[test]
    fn poll_without_connection_is_empty() {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut binance = Binance::with_http(Box::new(ArcTransport(http)), &opts);
        assert!(binance.poll_events().is_empty());
    }

    #[test]
    fn split_wire_symbol_uses_known_quotes() {
        assert_eq!(split_wire_symbol("BTCUSDT"), Symbol::new("BTC", "USDT"));
        assert_eq!(split_wire_symbol("ETHFDUSD"), Symbol::new("ETH", "FDUSD"));
        assert_eq!(split_wire_symbol("ETHBTC"), Symbol::new("ETH", "BTC"));
        // Unknown quote -> whole string as the base.
        assert_eq!(split_wire_symbol("WEIRD"), Symbol::new("WEIRD", ""));
    }

    #[test]
    fn open_orders_filtered_and_unfiltered() {
        let (binance, mock) = signed_client(1000);
        // Filtered: the symbol is known from the caller.
        mock.push_json(
            200,
            r#"[{"symbol":"BTCUSDT","orderId":1,"clientOrderId":"a","price":"100.0",
            "origQty":"1.0","executedQty":"0.0","cummulativeQuoteQty":"0.0",
            "status":"NEW","type":"LIMIT","side":"BUY"}]"#,
        );
        let orders = binance.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].symbol, symbol());

        // Unfiltered: the symbol is resolved from the wire form.
        mock.push_json(
            200,
            r#"[{"symbol":"ETHUSDT","orderId":2,"clientOrderId":"","price":"0.0",
            "origQty":"2.0","executedQty":"0.0","cummulativeQuoteQty":"0.0",
            "status":"NEW","type":"MARKET","side":"SELL"}]"#,
        );
        let orders = binance.open_orders(None).unwrap();
        assert_eq!(orders[0].symbol, Symbol::new("ETH", "USDT"));
        let req = &mock.recorded_requests()[1];
        assert!(!req.url.contains("symbol="));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        let mut exchange: Box<dyn Exchange> = Box::new(binance);
        assert_eq!(exchange.name(), "binance");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "28");
    }

    const EXCHANGE_INFO: &str = r#"{"symbols":[{"symbol":"BTCUSDT","baseAsset":"BTC",
        "quoteAsset":"USDT","baseAssetPrecision":8,"quoteAssetPrecision":8,"filters":[
        {"filterType":"LOT_SIZE","minQty":"0.001","maxQty":"1000","stepSize":"0.001"},
        {"filterType":"PRICE_FILTER","minPrice":"0.01","maxPrice":"1000000","tickSize":"0.01"},
        {"filterType":"NOTIONAL","minNotional":"10"}]}]}"#;

    #[test]
    fn load_instruments_populates_filters() {
        let (mut binance, mock) = signed_client(1000);
        mock.push_json(200, EXCHANGE_INFO);
        binance.load_instruments().unwrap();
        let inst = binance.instrument(&symbol()).unwrap();
        assert_eq!(inst.filters.step_size, dec!(0.001));
        assert_eq!(inst.filters.min_notional, dec!(10));
        assert_eq!(inst.filters.tick_size, dec!(0.01));
        assert_eq!(inst.base_precision, 8);
        // The request hit exchangeInfo with no query string.
        assert!(mock.recorded_requests()[0]
            .url
            .ends_with("/api/v3/exchangeInfo"));
    }

    #[test]
    fn place_order_rejects_filter_violation_when_loaded() {
        let (mut binance, mock) = signed_client(1000);
        mock.push_json(200, EXCHANGE_INFO);
        binance.load_instruments().unwrap();
        // quantity 0.0005 < min 0.001 -> rejected locally, no order sent.
        let err = binance
            .place_order(&OrderRequest::limit_buy(
                symbol(),
                dec!(0.0005),
                dec!(20000),
            ))
            .unwrap_err();
        assert!(matches!(err, Error::Filter(_)));
        assert_eq!(mock.recorded_requests().len(), 1); // only exchangeInfo
    }

    #[test]
    fn place_order_skips_filter_check_without_instruments() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        // No load_instruments: the order is sent (best effort).
        binance
            .place_order(&OrderRequest::limit_buy(
                symbol(),
                dec!(0.0005),
                dec!(20000),
            ))
            .unwrap();
        assert!(mock.recorded_requests()[0].url.contains("/api/v3/order"));
    }
}
