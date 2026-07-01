//! Bitget (v2 API) — the fourth exchange.
//!
//! Bitget's signing is close to OKX's — base64(HMAC-SHA256) with a passphrase —
//! but over a **millisecond** timestamp rather than ISO-8601, with `ACCESS-*`
//! headers, concatenated symbols (`BTCUSDT`) and a `{code, msg, data}` envelope
//! whose success code is the string `"00000"`.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::ExchangeOptions;
use crate::signing::hmac_sha256_base64;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
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

/// A Bitget client over injected transports.
pub struct Bitget {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    subscriptions: Vec<(String, Symbol)>,
}

impl Bitget {
    fn build(
        http: Box<dyn HttpTransport>,
        _options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://api.bitget.com".to_string(),
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            subscriptions: Vec::new(),
        }
    }

    /// Build a public Bitget client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Bitget client (credentials must carry a passphrase).
    #[must_use]
    pub fn with_credentials(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Credentials,
    ) -> Self {
        Self::build(http, options, Some(credentials))
    }

    /// Override the timestamp source (deterministic signing in tests).
    #[must_use]
    pub fn with_clock(mut self, now_ms: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now_ms = now_ms;
        self
    }

    /// Attach a WebSocket transport.
    #[must_use]
    pub fn with_ws(mut self, ws: Box<dyn WsTransport>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// The Bitget wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTCUSDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated()
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        let data = self.get("/api/v2/spot/market/tickers", &query)?;
        let list: Vec<RawTicker> = parse_json(data)?;
        let entry = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last_pr)?,
            bid: parse_decimal(&entry.bid_pr)?,
            ask: parse_decimal(&entry.ask_pr)?,
            volume: parse_decimal(&entry.base_volume)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Bitget returns
    /// oldest-first, which is already chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "symbol={}&granularity={}&limit={limit}",
            Self::wire_symbol(symbol),
            map_granularity(interval),
        );
        let data = self.get("/api/v2/spot/market/candles", &query)?;
        let rows: Vec<Vec<String>> = parse_json(data)?;
        rows.iter().map(|row| parse_kline_row(row)).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("symbol={}&limit={depth}", Self::wire_symbol(symbol));
        let data = self.get("/api/v2/spot/market/orderbook", &query)?;
        let raw: RawDepth = parse_json(data)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.ts.parse().unwrap_or(0),
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "trade")
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "books")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://ws.bitget.com/v2/ws/public")?;
            self.connection = Some(connection);
        }
        let message = format!(
            r#"{{"op":"subscribe","args":[{{"instType":"SPOT","channel":"{channel}","instId":"{wire}"}}]}}"#
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

    /// Drain all stream events available since the last call. Non-blocking.
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
            if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                events.append(&mut parsed);
            }
        }
        events
    }

    /// Place an order.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let force = if request.post_only {
            "post_only"
        } else {
            force_str(request.time_in_force)
        };
        let mut body = serde_json::json!({
            "symbol": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "orderType": order_type_str(request.order_type),
            "force": force,
            "size": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            body["clientOid"] = serde_json::json!(id.clone());
        }
        let data = self.signed_request(
            HttpMethod::Post,
            "/api/v2/spot/trade/place-order",
            "",
            &body.to_string(),
        )?;
        let placed: PlaceResult = parse_json(data)?;
        Ok(Order {
            id: placed.order_id,
            client_order_id: (!placed.client_oid.is_empty()).then_some(placed.client_oid),
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
            "symbol": Self::wire_symbol(symbol),
            "orderId": order_id,
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v2/spot/trade/cancel-order",
            "",
            &body.to_string(),
        )?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let query = format!("orderId={order_id}");
        let data =
            self.signed_request(HttpMethod::Get, "/api/v2/spot/trade/orderInfo", &query, "")?;
        let list: Vec<RawOrder> = parse_json(data)?;
        let raw = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// All open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let query = match symbol {
            Some(s) => format!("symbol={}", Self::wire_symbol(s)),
            None => String::new(),
        };
        let data = self.signed_request(
            HttpMethod::Get,
            "/api/v2/spot/trade/unfilled-orders",
            &query,
            "",
        )?;
        let list: Vec<RawOrder> = parse_json(data)?;
        list.iter()
            .map(|raw| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| split_wire_symbol(&raw.symbol));
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Spot account balances.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let data = self.signed_request(HttpMethod::Get, "/api/v2/spot/account/assets", "", "")?;
        let list: Vec<RawAsset> = parse_json(data)?;
        Ok(list
            .iter()
            .map(|a| Balance {
                asset: a.coin.clone(),
                free: dec_or_zero(&a.available),
                locked: dec_or_zero(&a.frozen),
            })
            .collect())
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_envelope(&response.body)
    }

    /// Sign with the `ACCESS-*` headers: base64(HMAC-SHA256) over
    /// `msTimestamp + METHOD + requestPath + body`.
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
        let passphrase = creds
            .passphrase
            .as_deref()
            .ok_or(Error::InvalidCredentials("Bitget requires a passphrase"))?;
        let timestamp = (self.now_ms)().to_string();
        let request_path = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let prehash = format!("{timestamp}{}{request_path}{body}", method.as_str());
        let signature = hmac_sha256_base64(creds.api_secret.as_bytes(), prehash.as_bytes());
        let url = format!("{}{request_path}", self.rest_base);
        let mut request = HttpRequest::new(method, url)
            .with_header("ACCESS-KEY", creds.api_key.clone())
            .with_header("ACCESS-SIGN", signature)
            .with_header("ACCESS-TIMESTAMP", timestamp)
            .with_header("ACCESS-PASSPHRASE", passphrase.to_string())
            .with_header("locale", "en-US");
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_envelope(&response.body)
    }
}

/// Map a unified interval to Bitget's `granularity` (`1min`, `1h`, `1day`).
fn map_granularity(interval: &str) -> String {
    match interval {
        "1m" => "1min",
        "3m" => "3min",
        "5m" => "5min",
        "15m" => "15min",
        "30m" => "30min",
        "1d" => "1day",
        "1w" => "1week",
        other => other,
    }
    .to_string()
}

fn unwrap_envelope(body: &str) -> Result<serde_json::Value> {
    let envelope: Envelope =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if envelope.code != "00000" {
        return Err(map_error(&envelope.code, &envelope.msg));
    }
    Ok(envelope.data)
}

fn parse_json<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn map_error(code: &str, msg: &str) -> Error {
    match code {
        "429" | "30007" => Error::RateLimited { retry_after: None },
        "40009" | "40012" | "40037" => Error::Auth(msg.to_string()),
        "43012" | "43011" => Error::InsufficientBalance,
        "40034" | "43001" => Error::NotFound(msg.to_string()),
        other => Error::Exchange {
            code: other.to_string(),
            message: msg.to_string(),
        },
    }
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

fn order_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market | OrderType::StopMarket => "market",
        OrderType::Limit | OrderType::StopLimit => "limit",
    }
}

fn force_str(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "gtc",
        TimeInForce::Ioc => "ioc",
        TimeInForce::Fok => "fok",
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    match raw {
        "market" => Ok(OrderType::Market),
        "limit" => Ok(OrderType::Limit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "live" | "new" => Ok(OrderStatus::New),
        "partially_filled" => Ok(OrderStatus::PartiallyFilled),
        "filled" | "full_fill" => Ok(OrderStatus::Filled),
        "cancelled" | "canceled" => Ok(OrderStatus::Canceled),
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

const KNOWN_QUOTES: &[&str] = &["USDT", "USDC", "EUR", "BTC", "ETH", "USD"];

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
    // Bitget candle: [ts, open, high, low, close, baseVol, quoteVol].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let ts = row[0]
        .parse::<i64>()
        .map_err(|e| Error::Deserialization(format!("kline ts not an integer: {e}")))?;
    let f = |i: usize| -> Result<f64> {
        row[i]
            .parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
    };
    Candle::new(f(1)?, f(2)?, f(3)?, f(4)?, f(5)?, ts)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    Ok(Order {
        id: raw.order_id.clone(),
        client_order_id: (!raw.client_oid.is_empty()).then(|| raw.client_oid.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status: parse_status(&raw.status)?,
        quantity: parse_decimal(&raw.size)?,
        filled_quantity: dec_or_zero(&raw.base_volume),
        price: nonzero_decimal(&raw.price),
        average_price: nonzero_decimal(&raw.price_avg),
    })
}

fn field_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization(format!("missing string field {key:?}")))
}

fn opt_str<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
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

fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let arg = value.get("arg");
    let Some(channel) = arg
        .and_then(|a| a.get("channel"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let wire = arg
        .and_then(|a| a.get("instId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let symbol = resolve(wire);
    let empty = Vec::new();
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    if channel == "trade" {
        data.iter()
            .map(|t| {
                Ok(Event::Trade(TradePrint {
                    symbol: symbol.clone(),
                    price: parse_decimal(field_str(t, "price")?)?,
                    quantity: parse_decimal(field_str(t, "size")?)?,
                    aggressor: parse_side(field_str(t, "side")?)?,
                    timestamp: opt_str(t, "ts").parse().unwrap_or(0),
                }))
            })
            .collect()
    } else if channel == "ticker" {
        data.iter()
            .map(|t| {
                Ok(Event::Ticker(Ticker {
                    symbol: symbol.clone(),
                    last: parse_decimal(field_str(t, "lastPr")?)?,
                    bid: dec_or_zero(opt_str(t, "bidPr")),
                    ask: dec_or_zero(opt_str(t, "askPr")),
                    volume: dec_or_zero(opt_str(t, "baseVolume")),
                }))
            })
            .collect()
    } else if channel == "books" {
        let action = value.get("action").and_then(serde_json::Value::as_str);
        data.iter()
            .map(|b| {
                let update_id = opt_str(b, "ts").parse().unwrap_or(0);
                let bids = parse_ws_levels(b.get("bids"))?;
                let asks = parse_ws_levels(b.get("asks"))?;
                if action == Some("snapshot") {
                    Ok(Event::BookSnapshot(OrderBookSnapshot {
                        symbol: symbol.clone(),
                        last_update_id: update_id,
                        bids,
                        asks,
                    }))
                } else {
                    Ok(Event::BookDelta(BookDelta {
                        symbol: symbol.clone(),
                        first_update_id: update_id,
                        final_update_id: update_id,
                        bids,
                        asks,
                    }))
                }
            })
            .collect()
    } else {
        Ok(Vec::new())
    }
}

#[derive(Deserialize)]
struct Envelope {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct RawTicker {
    #[serde(rename = "lastPr")]
    last_pr: String,
    #[serde(rename = "bidPr")]
    bid_pr: String,
    #[serde(rename = "askPr")]
    ask_pr: String,
    #[serde(rename = "baseVolume")]
    base_volume: String,
}

#[derive(Deserialize)]
struct RawDepth {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
    #[serde(default)]
    ts: String,
}

#[derive(Deserialize)]
struct PlaceResult {
    #[serde(rename = "orderId", default)]
    order_id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: String,
}

#[derive(Deserialize)]
struct RawOrder {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: String,
    side: String,
    #[serde(rename = "orderType")]
    order_type: String,
    status: String,
    size: String,
    #[serde(rename = "baseVolume", default)]
    base_volume: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "priceAvg", default)]
    price_avg: String,
}

#[derive(Deserialize)]
struct RawAsset {
    coin: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    frozen: String,
}

impl MarketData for Bitget {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Bitget::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Bitget::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Bitget::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Bitget::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Bitget::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Bitget::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Bitget::poll_events(self)
    }
}

impl Execution for Bitget {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Bitget::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Bitget::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Bitget::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Bitget::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Bitget::balances(self)
    }
}

impl Exchange for Bitget {
    fn name(&self) -> &'static str {
        "bitget"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{HttpResponse, MockHttpTransport, MockWsTransport};
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    struct ArcTransport(Arc<MockHttpTransport>);
    impl HttpTransport for ArcTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.0.execute(request)
        }
    }
    struct ArcWs(Arc<MockWsTransport>);
    impl WsTransport for ArcWs {
        fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>> {
            self.0.connect(url)
        }
    }

    fn symbol() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    fn client() -> (Bitget, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            Bitget::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Bitget, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let bitget = Bitget::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (bitget, mock)
    }

    #[test]
    fn ticker_and_error_mapping() {
        let (bitget, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"00000","msg":"success","data":[{"symbol":"BTCUSDT","lastPr":"20000",
            "bidPr":"19999","askPr":"20001","baseVolume":"1234"}]}"#,
        );
        let ticker = bitget.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, dec!(19999));

        let (bitget, mock) = client();
        mock.push_json(200, r#"{"code":"43012","msg":"insufficient","data":null}"#);
        assert!(matches!(
            bitget.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn klines_chronological() {
        let (bitget, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"00000","data":[["1700000000000","100","110","95","105","1","0"],
            ["1700000060000","105","106","104","105.5","2","0"]]}"#,
        );
        let candles = bitget.klines(&symbol(), "1m", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert_eq!(candles[1].timestamp, 1_700_000_060_000);
    }

    #[test]
    fn order_book_parses() {
        let (bitget, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"ts":"88","bids":[["100","1"]],"asks":[["101","2"]]}}"#,
        );
        let book = bitget.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 88);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
    }

    #[test]
    fn place_order_signs_with_access_headers() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"orderId":"42","clientOid":"abc"}}"#,
        );
        let order = bitget
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "42");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        let ts = header("ACCESS-TIMESTAMP");
        assert_eq!(ts, "1000");
        let body = req.body.as_ref().unwrap();
        let prehash = format!("{ts}POST/api/v2/spot/trade/place-order{body}");
        let expected = hmac_sha256_base64(b"SECRET", prehash.as_bytes());
        assert_eq!(header("ACCESS-SIGN"), expected);
        assert_eq!(header("ACCESS-KEY"), "APIKEY");
        assert_eq!(header("ACCESS-PASSPHRASE"), "PASS");
    }

    #[test]
    fn query_and_balances_and_open_orders() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":[{"symbol":"BTCUSDT","orderId":"42","clientOid":"",
            "side":"sell","orderType":"market","status":"filled","size":"2","baseVolume":"2",
            "price":"0","priceAvg":"100"}]}"#,
        );
        let order = bitget.query_order(&symbol(), "42").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.average_price, Some(dec!(100)));
        assert_eq!(order.price, None);

        mock.push_json(
            200,
            r#"{"code":"00000","data":[{"coin":"USDT","available":"100.5","frozen":"25.5"}]}"#,
        );
        let bals = bitget.balances().unwrap();
        assert_eq!(bals[0].total(), dec!(126));

        mock.push_json(
            200,
            r#"{"code":"00000","data":[{"symbol":"ETHUSDT","orderId":"7","clientOid":"",
            "side":"buy","orderType":"limit","status":"live","size":"1","baseVolume":"0",
            "price":"50","priceAvg":"0"}]}"#,
        );
        let open = bitget.open_orders(None).unwrap();
        assert_eq!(open[0].symbol, Symbol::new("ETH", "USDT"));
    }

    #[test]
    fn signed_requires_passphrase() {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let bitget = Bitget::with_credentials(
            Box::new(ArcTransport(mock)),
            &opts,
            Credentials::new("k", "s"),
        );
        assert!(matches!(
            bitget.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trade_and_book() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"action":"snapshot","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"},
                "data":[{"ts":"1","price":"100","size":"0.5","side":"buy"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"action":"snapshot","arg":{"instType":"SPOT","channel":"books","instId":"BTCUSDT"},
                "data":[{"ts":"5","bids":[["100","1"]],"asks":[["101","2"]]}]}"#
                    .to_string(),
            )),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut bitget = Bitget::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        bitget.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""channel":"trade""#));

        let events = bitget.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert!(matches!(events[1], Event::BookSnapshot(_)));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"orderId":"1","clientOid":""}}"#,
        );
        let mut exchange: Box<dyn Exchange> = Box::new(bitget);
        assert_eq!(exchange.name(), "bitget");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "1");
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }
}
