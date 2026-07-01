//! Gate.io (v4 API) — the sixth exchange.
//!
//! Gate signs with `SIGN = hex(HMAC-SHA512(secret, sig_string))`, where
//! `sig_string = METHOD\npath\nquery\nhex(SHA512(body))\ntimestamp` (unix
//! seconds), carried in `KEY`/`SIGN`/`Timestamp` headers. Symbols use an
//! underscore (`BTC_USDT`) and there is no response envelope — success is the raw
//! JSON, errors come back as an HTTP error status with `{label, message}`.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::ExchangeOptions;
use crate::signing::{hmac_sha512_hex, sha512_hex};
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use wickra_core::Candle;

fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// A Gate.io client over injected transports.
pub struct Gate {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
}

impl Gate {
    fn build(
        http: Box<dyn HttpTransport>,
        _options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://api.gateio.ws".to_string(),
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    /// Build a public Gate.io client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Gate.io client.
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

    /// The Gate wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTC_USDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}_{}", symbol.base(), symbol.quote())
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("currency_pair={}", Self::wire_symbol(symbol));
        let value = self.get("/api/v4/spot/tickers", &query)?;
        let list: Vec<RawTicker> = parse_json(value)?;
        let entry = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last)?,
            bid: parse_decimal(&entry.highest_bid)?,
            ask: parse_decimal(&entry.lowest_ask)?,
            volume: parse_decimal(&entry.base_volume)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Gate returns
    /// oldest-first, already chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "currency_pair={}&interval={}&limit={limit}",
            Self::wire_symbol(symbol),
            map_interval(interval),
        );
        let value = self.get("/api/v4/spot/candlesticks", &query)?;
        let rows: Vec<Vec<String>> = parse_json(value)?;
        rows.iter().map(|row| parse_kline_row(row)).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("currency_pair={}&limit={depth}", Self::wire_symbol(symbol));
        let value = self.get("/api/v4/spot/order_book", &query)?;
        let raw: RawDepth = parse_json(value)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.update,
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "spot.trades")
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "spot.order_book_update")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "spot.tickers")
    }

    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://api.gateio.ws/ws/v4/")?;
            self.connection = Some(connection);
        }
        let time = (self.now_ms)() / 1000;
        let message = format!(
            r#"{{"time":{time},"channel":"{channel}","event":"subscribe","payload":["{wire}"]}}"#
        );
        self.connection
            .as_mut()
            .expect("connection just ensured")
            .send(&message)?;
        if !self.sub_messages.contains(&message) {
            self.sub_messages.push(message.clone());
        }
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
        if let Some(connection) = self.connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(Some(event)) = parse_ws_message(&frame, &resolve) {
                    events.push(event);
                }
            }
        }
        let url = "wss://api.gateio.ws/ws/v4/";
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Place an order.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let mut body = serde_json::json!({
            "currency_pair": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "type": order_type_str(request.order_type),
            "amount": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if request.post_only {
            body["time_in_force"] = serde_json::json!("poc");
        }
        if let Some(id) = &request.client_order_id {
            let text = if id.starts_with("t-") {
                id.clone()
            } else {
                format!("t-{id}")
            };
            body["text"] = serde_json::json!(text);
        }
        let value = self.signed_request(
            HttpMethod::Post,
            "/api/v4/spot/orders",
            "",
            &body.to_string(),
        )?;
        let raw: RawOrder = parse_json(value)?;
        order_from_raw(request.symbol.clone(), &raw)
    }

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the venue rejects it.
    pub fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let path = format!("/api/v4/spot/orders/{order_id}");
        let query = format!("currency_pair={}", Self::wire_symbol(symbol));
        self.signed_request(HttpMethod::Delete, &path, &query, "")?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let path = format!("/api/v4/spot/orders/{order_id}");
        let query = format!("currency_pair={}", Self::wire_symbol(symbol));
        let value = self.signed_request(HttpMethod::Get, &path, &query, "")?;
        let raw: RawOrder = parse_json(value)?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// Open orders for one `symbol` (Gate requires a currency pair here).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing, no symbol is given, or the
    /// request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let sym = symbol.ok_or(Error::InvalidOrder("Gate open_orders requires a symbol"))?;
        let query = format!("currency_pair={}&status=open", Self::wire_symbol(sym));
        let value = self.signed_request(HttpMethod::Get, "/api/v4/spot/orders", &query, "")?;
        let list: Vec<RawOrder> = parse_json(value)?;
        list.iter()
            .map(|raw| order_from_raw(sym.clone(), raw))
            .collect()
    }

    /// Spot account balances.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let value = self.signed_request(HttpMethod::Get, "/api/v4/spot/accounts", "", "")?;
        let list: Vec<RawAccount> = parse_json(value)?;
        Ok(list
            .iter()
            .map(|a| Balance {
                asset: a.currency.clone(),
                free: dec_or_zero(&a.available),
                locked: dec_or_zero(&a.locked),
            })
            .collect())
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        parse_body(&response)
    }

    /// Sign with `KEY`/`SIGN`/`Timestamp`: `SIGN = hex(HMAC-SHA512(secret,
    /// METHOD\npath\nquery\nhex(SHA512(body))\ntimestamp))`.
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
        let timestamp = ((self.now_ms)() / 1000).to_string();
        let body_hash = sha512_hex(body.as_bytes());
        let sign_string = format!(
            "{}\n{path}\n{query}\n{body_hash}\n{timestamp}",
            method.as_str()
        );
        let signature = hmac_sha512_hex(creds.api_secret.as_bytes(), sign_string.as_bytes());
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let mut request = HttpRequest::new(method, url)
            .with_header("KEY", creds.api_key.clone())
            .with_header("SIGN", signature)
            .with_header("Timestamp", timestamp);
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        parse_body(&response)
    }
}

/// Parse a Gate response: raw JSON on success, `{label, message}` on an HTTP error.
fn parse_body(response: &HttpResponse) -> Result<serde_json::Value> {
    if response.is_success() {
        serde_json::from_str(&response.body).map_err(|e| Error::Deserialization(e.to_string()))
    } else {
        let err: GateError = serde_json::from_str(&response.body).unwrap_or(GateError {
            label: response.status.to_string(),
            message: response.body.clone(),
        });
        Err(map_error(&err.label, &err.message))
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn map_interval(interval: &str) -> String {
    match interval {
        "1w" => "7d",
        other => other,
    }
    .to_string()
}

fn map_error(label: &str, message: &str) -> Error {
    match label {
        "TOO_MANY_REQUESTS" | "RATE_LIMIT" => Error::RateLimited { retry_after: None },
        "INVALID_KEY" | "INVALID_SIGNATURE" | "SIGN_MISMATCH" | "AUTHENTICATION_FAILED" => {
            Error::Auth(message.to_string())
        }
        "BALANCE_NOT_ENOUGH" | "INSUFFICIENT_AVAILABLE" => Error::InsufficientBalance,
        "ORDER_NOT_FOUND" => Error::NotFound(message.to_string()),
        "INVALID_CURRENCY_PAIR" => Error::InvalidSymbol(message.to_string()),
        other => Error::Exchange {
            code: other.to_string(),
            message: message.to_string(),
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
    // Gate candle: [ts, quoteVol, close, high, low, open, baseVol, closed].
    if row.len() < 7 {
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
    // open=row[5], high=row[3], low=row[4], close=row[2], volume=row[6].
    Candle::new(f(5)?, f(3)?, f(4)?, f(2)?, f(6)?, ts)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    let filled = dec_or_zero(&raw.filled_amount);
    let status = match raw.status.as_str() {
        "cancelled" => OrderStatus::Canceled,
        "closed" => OrderStatus::Filled,
        "open" => {
            if filled > Decimal::ZERO {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }
        }
        other => return Err(Error::Deserialization(format!("unknown status {other:?}"))),
    };
    Ok(Order {
        id: raw.id.clone(),
        client_order_id: (!raw.text.is_empty()).then(|| raw.text.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status,
        quantity: parse_decimal(&raw.amount)?,
        filled_quantity: filled,
        price: nonzero_decimal(&raw.price),
        average_price: nonzero_decimal(&raw.avg_deal_price),
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

fn opt_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn opt_i64(value: &serde_json::Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
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

fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Option<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("update") {
        return Ok(None); // subscribe ack / other events
    }
    let Some(channel) = value.get("channel").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let null = serde_json::Value::Null;
    let result = value.get("result").unwrap_or(&null);

    match channel {
        "spot.trades" => Ok(Some(Event::Trade(TradePrint {
            symbol: resolve(field_str(result, "currency_pair")?),
            price: parse_decimal(field_str(result, "price")?)?,
            quantity: parse_decimal(field_str(result, "amount")?)?,
            aggressor: parse_side(field_str(result, "side")?)?,
            timestamp: opt_i64(result, "create_time_ms"),
        }))),
        "spot.tickers" => Ok(Some(Event::Ticker(Ticker {
            symbol: resolve(field_str(result, "currency_pair")?),
            last: parse_decimal(field_str(result, "last")?)?,
            bid: dec_or_zero(opt_str(result, "highest_bid")),
            ask: dec_or_zero(opt_str(result, "lowest_ask")),
            volume: dec_or_zero(opt_str(result, "base_volume")),
        }))),
        "spot.order_book_update" => {
            let update_id = opt_u64(result, "u");
            Ok(Some(Event::BookDelta(BookDelta {
                symbol: resolve(opt_str(result, "s")),
                first_update_id: update_id,
                final_update_id: update_id,
                bids: parse_ws_levels(result.get("b"))?,
                asks: parse_ws_levels(result.get("a"))?,
            })))
        }
        _ => Ok(None),
    }
}

#[derive(Deserialize)]
struct GateError {
    #[serde(default)]
    label: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct RawTicker {
    last: String,
    highest_bid: String,
    lowest_ask: String,
    base_volume: String,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(default)]
    update: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
struct RawOrder {
    id: String,
    #[serde(default)]
    text: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    status: String,
    amount: String,
    #[serde(rename = "filled_amount", default)]
    filled_amount: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "avg_deal_price", default)]
    avg_deal_price: String,
}

#[derive(Deserialize)]
struct RawAccount {
    currency: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    locked: String,
}

impl MarketData for Gate {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Gate::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Gate::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Gate::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Gate::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Gate::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Gate::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Gate::poll_events(self)
    }
}

impl Execution for Gate {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Gate::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Gate::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Gate::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Gate::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Gate::balances(self)
    }
}

impl Exchange for Gate {
    fn name(&self) -> &'static str {
        "gate"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{MockHttpTransport, MockWsTransport};
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

    fn client() -> (Gate, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            Gate::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Gate, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let gate = Gate::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (gate, mock)
    }

    #[test]
    fn ticker_parses() {
        let (gate, mock) = client();
        mock.push_json(
            200,
            r#"[{"currency_pair":"BTC_USDT","last":"20000","highest_bid":"19999",
            "lowest_ask":"20001","base_volume":"1234"}]"#,
        );
        let ticker = gate.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, dec!(19999));
        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://api.gateio.ws/api/v4/spot/tickers?currency_pair=BTC_USDT"
        );
    }

    #[test]
    fn klines_field_order() {
        let (gate, mock) = client();
        // [ts, quoteVol, close, high, low, open, baseVol, closed].
        mock.push_json(
            200,
            r#"[["1700000000","0","105","110","95","100","12","true"]]"#,
        );
        let candles = gate.klines(&symbol(), "1h", 1).unwrap();
        assert!((candles[0].open - 100.0).abs() < 1e-9);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!((candles[0].close - 105.0).abs() < 1e-9);
        assert_eq!(candles[0].timestamp, 1_700_000_000);
    }

    #[test]
    fn order_book_parses() {
        let (gate, mock) = client();
        mock.push_json(
            200,
            r#"{"update":66,"bids":[["100","1"]],"asks":[["101","2"]]}"#,
        );
        let book = gate.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 66);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
    }

    #[test]
    fn http_error_maps_to_taxonomy() {
        let (gate, mock) = client();
        mock.push_json(400, r#"{"label":"BALANCE_NOT_ENOUGH","message":"nope"}"#);
        assert!(matches!(
            gate.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn place_order_signs_with_sha512_body_hash() {
        let (gate, mock) = signed_client(1_000_000); // ms -> 1000 s
        mock.push_json(
            200,
            r#"{"id":"77","text":"t-abc","side":"buy","type":"limit","status":"open",
            "amount":"1","filled_amount":"0","price":"100","avg_deal_price":"0"}"#,
        );
        let order = gate
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "77");
        assert_eq!(order.status, OrderStatus::New);

        let req = &mock.recorded_requests()[0];
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        assert_eq!(header("Timestamp"), "1000");
        assert_eq!(header("KEY"), "APIKEY");
        let body = req.body.as_ref().unwrap();
        let body_hash = sha512_hex(body.as_bytes());
        let sign_string = format!("POST\n/api/v4/spot/orders\n\n{body_hash}\n1000");
        assert_eq!(
            header("SIGN"),
            hmac_sha512_hex(b"SECRET", sign_string.as_bytes())
        );
        assert!(body.contains(r#""text":"t-abc""#));
    }

    #[test]
    fn query_and_balances() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":"77","text":"","side":"sell","type":"market","status":"closed",
            "amount":"2","filled_amount":"2","price":"0","avg_deal_price":"100"}"#,
        );
        let order = gate.query_order(&symbol(), "77").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.average_price, Some(dec!(100)));

        mock.push_json(
            200,
            r#"[{"currency":"USDT","available":"100.5","locked":"25.5"}]"#,
        );
        let bals = gate.balances().unwrap();
        assert_eq!(bals[0].total(), dec!(126));
    }

    #[test]
    fn open_orders_requires_symbol() {
        let (gate, _mock) = signed_client(1_000_000);
        assert!(matches!(
            gate.open_orders(None).unwrap_err(),
            Error::InvalidOrder(_)
        ));
    }

    #[test]
    fn signed_requires_credentials() {
        let (gate, _) = client();
        assert!(matches!(
            gate.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trade_and_book() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"time":1,"channel":"spot.trades","event":"update","result":
                {"currency_pair":"BTC_USDT","side":"buy","amount":"0.5","price":"100","create_time_ms":1700}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"time":2,"channel":"spot.order_book_update","event":"update","result":
                {"s":"BTC_USDT","u":9,"b":[["100","1"]],"a":[["101","2"]]}}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"channel":"spot.trades","event":"subscribe"}"#.to_string())),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut gate = Gate::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        gate.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""channel":"spot.trades""#));

        let events = gate.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert_eq!(t.timestamp, 1700);
        let Event::BookDelta(d) = &events[1] else {
            panic!("expected book delta")
        };
        assert_eq!(d.final_update_id, 9);
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":"1","text":"","side":"buy","type":"limit","status":"open",
            "amount":"1","filled_amount":"0","price":"100","avg_deal_price":"0"}"#,
        );
        let mut exchange: Box<dyn Exchange> = Box::new(gate);
        assert_eq!(exchange.name(), "gate");
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
