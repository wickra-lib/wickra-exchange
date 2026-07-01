//! Upbit — the tenth exchange.
//!
//! Upbit authenticates with a **JWT HS512**: the payload carries `access_key`
//! and a `nonce`, plus — for parameterised requests — a `query_hash` (hex
//! SHA-512 of the form-encoded parameters) and `query_hash_alg="SHA512"`, signed
//! HMAC-SHA512 with the secret. Markets are **quote-first** (`USDT-BTC`), and
//! market-data JSON encodes numbers as JSON numbers. There is no envelope;
//! errors come back as an HTTP error status with `{error:{name, message}}`.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::ExchangeOptions;
use crate::signing::{hmac_sha512_bytes, sha512_hex};
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};
use base64::Engine;
use rust_decimal::Decimal;
use wickra_core::Candle;

fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// An Upbit client over injected transports.
pub struct Upbit {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
}

impl Upbit {
    fn build(
        http: Box<dyn HttpTransport>,
        _options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://api.upbit.com".to_string(),
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
        }
    }

    /// Build a public Upbit client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Upbit client.
    #[must_use]
    pub fn with_credentials(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Credentials,
    ) -> Self {
        Self::build(http, options, Some(credentials))
    }

    /// Override the timestamp/nonce source (deterministic signing in tests).
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

    /// The Upbit wire market for a canonical [`Symbol`] (`BTC/USDT` -> `USDT-BTC`,
    /// quote first).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}-{}", symbol.quote(), symbol.base())
    }

    /// A ticker for `symbol`. Upbit's ticker has no separate bid/ask, so both
    /// mirror the last trade price.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("markets={}", Self::wire_symbol(symbol));
        let value = self.get("/v1/ticker", &query)?;
        let entry = value
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        let last = decimal_field(entry, "trade_price")?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last,
            bid: last,
            ask: last,
            volume: decimal_field(entry, "acc_trade_volume_24h").unwrap_or(Decimal::ZERO),
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Upbit returns
    /// newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let path = candle_path(interval);
        let query = format!("market={}&count={limit}", Self::wire_symbol(symbol));
        let value = self.get(&path, &query)?;
        let rows = value
            .as_array()
            .ok_or_else(|| Error::Deserialization("missing candles".to_string()))?;
        let mut candles = rows.iter().map(parse_candle).collect::<Result<Vec<_>>>()?;
        candles.reverse();
        Ok(candles)
    }

    /// A depth snapshot of `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, _depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("markets={}", Self::wire_symbol(symbol));
        let value = self.get("/v1/orderbook", &query)?;
        let entry = value
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| Error::NotFound(format!("no book for {symbol}")))?;
        let units = entry
            .get("orderbook_units")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing orderbook_units".to_string()))?;
        let mut bids = Vec::with_capacity(units.len());
        let mut asks = Vec::with_capacity(units.len());
        for unit in units {
            bids.push(BookLevel {
                price: decimal_field(unit, "bid_price")?,
                quantity: decimal_field(unit, "bid_size")?,
            });
            asks.push(BookLevel {
                price: decimal_field(unit, "ask_price")?,
                quantity: decimal_field(unit, "ask_size")?,
            });
        }
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: 0,
            bids,
            asks,
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
        self.subscribe(symbol, "orderbook")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    fn subscribe(&mut self, symbol: &Symbol, kind: &str) -> Result<()> {
        let market = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://api.upbit.com/websocket/v1")?;
            self.connection = Some(connection);
        }
        let ticket = format!("wkex-{}", (self.now_ms)());
        let message = format!(
            r#"[{{"ticket":"{ticket}"}},{{"type":"{kind}","codes":["{market}"]}},{{"format":"DEFAULT"}}]"#
        );
        self.connection
            .as_mut()
            .expect("connection just ensured")
            .send(&message)?;
        Ok(())
    }

    /// Drain all stream events available since the last call. Non-blocking.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let Some(connection) = self.connection.as_mut() else {
            return events;
        };
        while let Ok(Some(frame)) = connection.recv() {
            if let Ok(Some(event)) = parse_ws_message(&frame) {
                events.push(event);
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
        let mut params: Vec<(&str, String)> = vec![
            ("market", Self::wire_symbol(&request.symbol)),
            ("side", side_str(request.side).to_string()),
            (
                "ord_type",
                ord_type_str(request.order_type, request.side).to_string(),
            ),
            ("volume", format_decimal(request.quantity)),
        ];
        if let Some(price) = request.price {
            params.push(("price", format_decimal(price)));
        }
        let value = self.signed_request(HttpMethod::Post, "/v1/orders", &params)?;
        order_from_value(request.symbol.clone(), &value)
    }

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the venue rejects it.
    pub fn cancel_order(&self, _symbol: &Symbol, order_id: &str) -> Result<()> {
        self.signed_request(
            HttpMethod::Delete,
            "/v1/order",
            &[("uuid", order_id.to_string())],
        )?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let value = self.signed_request(
            HttpMethod::Get,
            "/v1/order",
            &[("uuid", order_id.to_string())],
        )?;
        order_from_value(symbol.clone(), &value)
    }

    /// Open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let mut params: Vec<(&str, String)> = vec![("state", "wait".to_string())];
        let market = symbol.map(Self::wire_symbol);
        if let Some(m) = &market {
            params.push(("market", m.clone()));
        }
        let value = self.signed_request(HttpMethod::Get, "/v1/orders", &params)?;
        let orders = value
            .as_array()
            .ok_or_else(|| Error::Deserialization("missing orders".to_string()))?;
        orders
            .iter()
            .map(|order| {
                let sym = symbol.cloned().unwrap_or_else(|| {
                    let raw = order
                        .get("market")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    split_wire_symbol(raw)
                });
                order_from_value(sym, order)
            })
            .collect()
    }

    /// Account balances.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let value = self.signed_request(HttpMethod::Get, "/v1/accounts", &[])?;
        let accounts = value
            .as_array()
            .ok_or_else(|| Error::Deserialization("missing accounts".to_string()))?;
        accounts
            .iter()
            .map(|a| {
                Ok(Balance {
                    asset: a
                        .get("currency")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    free: decimal_field(a, "balance").unwrap_or(Decimal::ZERO),
                    locked: decimal_field(a, "locked").unwrap_or(Decimal::ZERO),
                })
            })
            .collect()
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        parse_body(&response)
    }

    /// Sign with a JWT HS512 (Authorization: Bearer). Parameterised requests add
    /// a hex-SHA512 `query_hash`; the parameters are sent form-encoded.
    fn signed_request(
        &self,
        method: HttpMethod,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        let query_string = params
            .iter()
            .map(|(key, val)| format!("{key}={val}"))
            .collect::<Vec<_>>()
            .join("&");
        let query_hash = (!params.is_empty()).then(|| sha512_hex(query_string.as_bytes()));
        let jwt = self.build_jwt(query_hash)?;

        let is_get = matches!(method, HttpMethod::Get);
        let url = if is_get && !query_string.is_empty() {
            format!("{}{path}?{query_string}", self.rest_base)
        } else {
            format!("{}{path}", self.rest_base)
        };
        let mut request =
            HttpRequest::new(method, url).with_header("Authorization", format!("Bearer {jwt}"));
        if !is_get && !query_string.is_empty() {
            request = request
                .with_header("Content-Type", "application/x-www-form-urlencoded")
                .with_body(query_string);
        }
        let response = self.http.execute(&request)?;
        parse_body(&response)
    }

    fn build_jwt(&self, query_hash: Option<String>) -> Result<String> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let header = br#"{"alg":"HS512","typ":"JWT"}"#;
        let nonce = format!("wkex-{:x}", (self.now_ms)());
        let mut payload = serde_json::json!({
            "access_key": creds.api_key,
            "nonce": nonce,
        });
        if let Some(hash) = query_hash {
            payload["query_hash"] = serde_json::json!(hash);
            payload["query_hash_alg"] = serde_json::json!("SHA512");
        }
        let signing_input = format!(
            "{}.{}",
            b64url(header),
            b64url(payload.to_string().as_bytes())
        );
        let signature = hmac_sha512_bytes(creds.api_secret.as_bytes(), signing_input.as_bytes());
        Ok(format!("{signing_input}.{}", b64url(&signature)))
    }
}

fn candle_path(interval: &str) -> String {
    match interval {
        "1m" => "/v1/candles/minutes/1".to_string(),
        "3m" => "/v1/candles/minutes/3".to_string(),
        "5m" => "/v1/candles/minutes/5".to_string(),
        "15m" => "/v1/candles/minutes/15".to_string(),
        "30m" => "/v1/candles/minutes/30".to_string(),
        "4h" => "/v1/candles/minutes/240".to_string(),
        "1d" => "/v1/candles/days".to_string(),
        "1w" => "/v1/candles/weeks".to_string(),
        // Default (and "1h") -> 60-minute candles.
        _ => "/v1/candles/minutes/60".to_string(),
    }
}

fn parse_body(response: &HttpResponse) -> Result<serde_json::Value> {
    if response.is_success() {
        serde_json::from_str(&response.body).map_err(|e| Error::Deserialization(e.to_string()))
    } else {
        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        let error = value.get("error");
        let name = error
            .and_then(|e| e.get("name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Err(map_error(name, message, response.status))
    }
}

fn map_error(name: &str, message: &str, status: u16) -> Error {
    if name.contains("insufficient_funds") || message.contains("주문가능") {
        Error::InsufficientBalance
    } else if status == 401 || name.contains("invalid_access_key") || name.contains("jwt") {
        Error::Auth(message.to_string())
    } else if status == 429 || name.contains("too_many") {
        Error::RateLimited { retry_after: None }
    } else if name.contains("order_not_found") {
        Error::NotFound(message.to_string())
    } else if name.contains("market") {
        Error::InvalidSymbol(message.to_string())
    } else {
        Error::Exchange {
            code: if name.is_empty() {
                status.to_string()
            } else {
                name.to_string()
            },
            message: message.to_string(),
        }
    }
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "bid",
        OrderSide::Sell => "ask",
    }
}

fn ord_type_str(order_type: OrderType, side: OrderSide) -> &'static str {
    match order_type {
        OrderType::Limit | OrderType::StopLimit => "limit",
        // Upbit market buys are priced by total ("price"), market sells by volume.
        OrderType::Market | OrderType::StopMarket => match side {
            OrderSide::Buy => "price",
            OrderSide::Sell => "market",
        },
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "bid" => Ok(OrderSide::Buy),
        "ask" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> OrderType {
    match raw {
        "price" | "market" => OrderType::Market,
        _ => OrderType::Limit,
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "wait" | "watch" => Ok(OrderStatus::New),
        "done" => Ok(OrderStatus::Filled),
        "cancel" => Ok(OrderStatus::Canceled),
        other => Err(Error::Deserialization(format!("unknown state {other:?}"))),
    }
}

fn nonzero(value: Decimal) -> Option<Decimal> {
    (value > Decimal::ZERO).then_some(value)
}

fn decimal_value(field: &serde_json::Value) -> Result<Decimal> {
    match field {
        serde_json::Value::String(s) => parse_decimal(s),
        serde_json::Value::Number(n) => parse_decimal(&n.to_string()),
        other => Err(Error::Deserialization(format!("not a number: {other}"))),
    }
}

fn decimal_field(value: &serde_json::Value, key: &str) -> Result<Decimal> {
    let field = value
        .get(key)
        .ok_or_else(|| Error::Deserialization(format!("missing field {key:?}")))?;
    decimal_value(field)
}

fn f64_field(value: &serde_json::Value, key: &str) -> Result<f64> {
    let field = value
        .get(key)
        .ok_or_else(|| Error::Deserialization(format!("missing field {key:?}")))?;
    field
        .as_f64()
        .or_else(|| field.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| Error::Deserialization(format!("field {key:?} not a number")))
}

fn split_wire_symbol(wire: &str) -> Symbol {
    match wire.split_once('-') {
        Some((quote, base)) if !quote.is_empty() && !base.is_empty() => Symbol::new(base, quote),
        _ => Symbol::new(wire, ""),
    }
}

fn parse_candle(candle: &serde_json::Value) -> Result<Candle> {
    let ts = candle
        .get("timestamp")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| Error::Deserialization("candle timestamp missing".to_string()))?;
    Candle::new(
        f64_field(candle, "opening_price")?,
        f64_field(candle, "high_price")?,
        f64_field(candle, "low_price")?,
        f64_field(candle, "trade_price")?,
        f64_field(candle, "candle_acc_trade_volume")?,
        ts,
    )
    .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_value(symbol: Symbol, order: &serde_json::Value) -> Result<Order> {
    let side = order
        .get("side")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let ord_type = order
        .get("ord_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("limit");
    let state = order
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("wait");
    Ok(Order {
        id: order
            .get("uuid")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        client_order_id: order
            .get("identifier")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        symbol,
        side: parse_side(side)?,
        order_type: parse_order_type(ord_type),
        status: parse_status(state)?,
        quantity: decimal_field(order, "volume").unwrap_or(Decimal::ZERO),
        filled_quantity: decimal_field(order, "executed_volume").unwrap_or(Decimal::ZERO),
        price: decimal_field(order, "price").ok().and_then(nonzero),
        average_price: None,
    })
}

fn parse_ws_message(text: &str) -> Result<Option<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(None); // status frame
    };
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let symbol = split_wire_symbol(code);

    match kind {
        "trade" => {
            let ask_bid = value
                .get("ask_bid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            Ok(Some(Event::Trade(TradePrint {
                symbol,
                price: decimal_field(&value, "trade_price")?,
                quantity: decimal_field(&value, "trade_volume")?,
                aggressor: if ask_bid == "ASK" {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                },
                timestamp: value
                    .get("trade_timestamp")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })))
        }
        "ticker" => Ok(Some(Event::Ticker(Ticker {
            symbol,
            last: decimal_field(&value, "trade_price")?,
            bid: decimal_field(&value, "trade_price").unwrap_or(Decimal::ZERO),
            ask: decimal_field(&value, "trade_price").unwrap_or(Decimal::ZERO),
            volume: decimal_field(&value, "acc_trade_volume_24h").unwrap_or(Decimal::ZERO),
        }))),
        "orderbook" => {
            let units = value
                .get("orderbook_units")
                .and_then(serde_json::Value::as_array);
            let mut bids = Vec::new();
            let mut asks = Vec::new();
            if let Some(units) = units {
                for unit in units {
                    bids.push(BookLevel {
                        price: decimal_field(unit, "bid_price")?,
                        quantity: decimal_field(unit, "bid_size")?,
                    });
                    asks.push(BookLevel {
                        price: decimal_field(unit, "ask_price")?,
                        quantity: decimal_field(unit, "ask_size")?,
                    });
                }
            }
            Ok(Some(Event::BookSnapshot(OrderBookSnapshot {
                symbol,
                last_update_id: 0,
                bids,
                asks,
            })))
        }
        _ => Ok(None),
    }
}

impl MarketData for Upbit {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Upbit::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Upbit::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Upbit::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Upbit::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Upbit::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Upbit::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Upbit::poll_events(self)
    }
}

impl Execution for Upbit {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Upbit::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Upbit::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Upbit::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Upbit::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Upbit::balances(self)
    }
}

impl Exchange for Upbit {
    fn name(&self) -> &'static str {
        "upbit"
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

    fn client() -> (Upbit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            Upbit::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Upbit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let upbit = Upbit::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (upbit, mock)
    }

    #[test]
    fn wire_symbol_is_quote_first() {
        assert_eq!(Upbit::wire_symbol(&symbol()), "USDT-BTC");
        assert_eq!(split_wire_symbol("USDT-BTC"), symbol());
    }

    #[test]
    fn ticker_uses_trade_price() {
        let (upbit, mock) = client();
        mock.push_json(
            200,
            r#"[{"market":"USDT-BTC","trade_price":20000.5,"acc_trade_volume_24h":1234.0}]"#,
        );
        let ticker = upbit.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(20000.5));
        assert_eq!(ticker.volume, dec!(1234));
        assert_eq!(
            mock.recorded_requests()[0].url,
            "https://api.upbit.com/v1/ticker?markets=USDT-BTC"
        );
    }

    #[test]
    fn klines_reversed() {
        let (upbit, mock) = client();
        mock.push_json(
            200,
            r#"[{"timestamp":1700003600000,"opening_price":105,"high_price":106,"low_price":104,
            "trade_price":105.5,"candle_acc_trade_volume":2},
            {"timestamp":1700000000000,"opening_price":100,"high_price":110,"low_price":95,
            "trade_price":105,"candle_acc_trade_volume":12}]"#,
        );
        let candles = upbit.klines(&symbol(), "1h", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/v1/candles/minutes/60"));
    }

    #[test]
    fn order_book_splits_units() {
        let (upbit, mock) = client();
        mock.push_json(
            200,
            r#"[{"market":"USDT-BTC","orderbook_units":[
            {"ask_price":101,"bid_price":100,"ask_size":2,"bid_size":1}]}]"#,
        );
        let book = upbit.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101), dec!(2)));
    }

    #[test]
    fn place_order_signs_with_hs512_jwt_and_query_hash() {
        let (upbit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"uuid":"U1","market":"USDT-BTC","side":"bid","ord_type":"limit","state":"wait",
            "volume":"1","executed_volume":"0","price":"100"}"#,
        );
        let order = upbit
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "U1");
        assert_eq!(order.side, OrderSide::Buy);

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let auth = req
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.strip_prefix("Bearer ").unwrap())
            .unwrap();
        // Reconstruct the HS512 JWT deterministically.
        let query = "market=USDT-BTC&side=bid&ord_type=limit&volume=1&price=100";
        let query_hash = sha512_hex(query.as_bytes());
        let header = br#"{"alg":"HS512","typ":"JWT"}"#;
        let payload = serde_json::json!({
            "access_key":"APIKEY","nonce":"wkex-3e8",
            "query_hash":query_hash,"query_hash_alg":"SHA512"
        });
        let signing_input = format!(
            "{}.{}",
            b64url(header),
            b64url(payload.to_string().as_bytes())
        );
        let sig = hmac_sha512_bytes(b"SECRET", signing_input.as_bytes());
        assert_eq!(auth, format!("{signing_input}.{}", b64url(&sig)));
        assert_eq!(req.body.as_deref(), Some(query));
    }

    #[test]
    fn query_and_balances() {
        let (upbit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"uuid":"U1","side":"ask","ord_type":"limit","state":"done","volume":"2",
            "executed_volume":"2","price":"100"}"#,
        );
        let order = upbit.query_order(&symbol(), "U1").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.filled_quantity, dec!(2));

        mock.push_json(
            200,
            r#"[{"currency":"USDT","balance":"100.5","locked":"25.5"}]"#,
        );
        let bals = upbit.balances().unwrap();
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].total(), dec!(126));
        // The accounts request has no query hash (no params) but still carries a JWT.
        let reqs = mock.recorded_requests();
        assert!(reqs[1].headers.iter().any(|(k, _)| k == "Authorization"));
    }

    #[test]
    fn signed_requires_credentials() {
        let (upbit, _) = client();
        assert!(matches!(
            upbit.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trade_and_orderbook() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"type":"trade","code":"USDT-BTC","trade_price":100,"trade_volume":0.5,
                "ask_bid":"BID","trade_timestamp":1700}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"type":"orderbook","code":"USDT-BTC","orderbook_units":[
                {"ask_price":101,"bid_price":100,"ask_size":2,"bid_size":1}]}"#
                    .to_string(),
            )),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut upbit = Upbit::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        upbit.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""type":"trade""#));
        assert!(ws.sent()[0].contains(r#""codes":["USDT-BTC"]"#));

        let events = upbit.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert_eq!(t.symbol, symbol());
        assert!(matches!(events[1], Event::BookSnapshot(_)));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (upbit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"uuid":"O1","side":"bid","ord_type":"limit","state":"wait","volume":"1","price":"100"}"#,
        );
        let mut exchange: Box<dyn Exchange> = Box::new(upbit);
        assert_eq!(exchange.name(), "upbit");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "O1");
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }
}
