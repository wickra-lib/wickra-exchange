//! Coinbase Advanced Trade — the ninth exchange.
//!
//! Coinbase authenticates every request with a short-lived **ES256 JWT**: the
//! header carries `alg=ES256`, the API key name as `kid`, and a nonce; the
//! payload carries `sub`, `iss=cdp`, `nbf`/`exp` and a `uri` of
//! `"METHOD host path"`. The JWT is signed with the account's EC private key
//! (`Credentials::with_private_key`, PKCS#8 PEM) via P-256 ECDSA — which uses
//! deterministic RFC-6979 nonces, so a test can verify the signature against the
//! derived public key. Symbols are dash-form (`BTC-USD`); errors come back as an
//! HTTP error status with an `{error, message}` body.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::ExchangeOptions;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};
use base64::Engine;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use rust_decimal::Decimal;
use wickra_core::Candle;

const HOST: &str = "api.coinbase.com";

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

/// A Coinbase Advanced Trade client over injected transports.
pub struct Coinbase {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
}

impl Coinbase {
    fn build(
        http: Box<dyn HttpTransport>,
        _options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: format!("https://{HOST}"),
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
        }
    }

    /// Build a Coinbase client. Signed calls require credentials whose key name
    /// is the API key id and whose private key (`with_private_key`) is the EC
    /// PKCS#8 PEM.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Coinbase client.
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

    /// The Coinbase wire symbol for a canonical [`Symbol`] (`BTC/USD` -> `BTC-USD`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}-{}", symbol.base(), symbol.quote())
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let path = format!(
            "/api/v3/brokerage/products/{}/ticker",
            Self::wire_symbol(symbol)
        );
        let value = self.signed_get(&path, "limit=1")?;
        let last = value
            .get("trades")
            .and_then(serde_json::Value::as_array)
            .and_then(|t| t.first())
            .and_then(|t| t.get("price"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing last price".to_string()))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(last)?,
            bid: parse_decimal(str_field(&value, "best_bid")?)?,
            ask: parse_decimal(str_field(&value, "best_ask")?)?,
            volume: Decimal::ZERO,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Coinbase
    /// returns newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let (granularity, step) = granularity(interval);
        let end = (self.now_ms)() / 1000;
        let start = end - i64::from(limit) * step;
        let path = format!(
            "/api/v3/brokerage/products/{}/candles",
            Self::wire_symbol(symbol)
        );
        let query = format!("granularity={granularity}&start={start}&end={end}");
        let value = self.signed_get(&path, &query)?;
        let candles = value
            .get("candles")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing candles".to_string()))?;
        let mut parsed = candles
            .iter()
            .map(parse_candle)
            .collect::<Result<Vec<_>>>()?;
        parsed.reverse();
        Ok(parsed)
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("product_id={}&limit={depth}", Self::wire_symbol(symbol));
        let value = self.signed_get("/api/v3/brokerage/product_book", &query)?;
        let book = value
            .get("pricebook")
            .ok_or_else(|| Error::Deserialization("missing pricebook".to_string()))?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: 0,
            bids: object_levels(book.get("bids"))?,
            asks: object_levels(book.get("asks"))?,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured,
    /// or [`Error::InvalidCredentials`] if credentials are missing.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "market_trades")
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "level2")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let jwt = self.build_jwt(None)?;
        let product = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://advanced-trade-ws.coinbase.com")?;
            self.connection = Some(connection);
        }
        let message = format!(
            r#"{{"type":"subscribe","product_ids":["{product}"],"channel":"{channel}","jwt":"{jwt}"}}"#
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
            if let Ok(mut parsed) = parse_ws_message(&frame) {
                events.append(&mut parsed);
            }
        }
        events
    }

    /// Place an order. Coinbase requires a client order id; one is generated from
    /// the clock when absent.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let client_order_id = request
            .client_order_id
            .clone()
            .unwrap_or_else(|| format!("wkex-{}", (self.now_ms)()));
        let base_size = format_decimal(request.quantity);
        let configuration = match request.order_type {
            OrderType::Market | OrderType::StopMarket => {
                serde_json::json!({ "market_market_ioc": { "base_size": base_size } })
            }
            OrderType::Limit | OrderType::StopLimit => {
                let price = request
                    .price
                    .ok_or(Error::InvalidOrder("limit order requires a price"))?;
                serde_json::json!({ "limit_limit_gtc": {
                    "base_size": base_size,
                    "limit_price": format_decimal(price),
                    "post_only": request.post_only,
                }})
            }
        };
        let body = serde_json::json!({
            "client_order_id": client_order_id,
            "product_id": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "order_configuration": configuration,
        });
        let value = self.signed_post("/api/v3/brokerage/orders", &body.to_string())?;
        if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
            let err = value.get("error_response");
            return Err(Error::OrderRejected {
                code: err
                    .and_then(|e| e.get("error"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("rejected")
                    .to_string(),
                message: err
                    .and_then(|e| e.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
        let order_id = value
            .get("success_response")
            .and_then(|r| r.get("order_id"))
            .or_else(|| value.get("order_id"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing order id".to_string()))?;
        Ok(Order {
            id: order_id.to_string(),
            client_order_id: Some(client_order_id),
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
    pub fn cancel_order(&self, _symbol: &Symbol, order_id: &str) -> Result<()> {
        let body = serde_json::json!({ "order_ids": [order_id] });
        self.signed_post("/api/v3/brokerage/orders/batch_cancel", &body.to_string())?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let path = format!("/api/v3/brokerage/orders/historical/{order_id}");
        let value = self.signed_get(&path, "")?;
        let order = value
            .get("order")
            .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
        order_from_value(symbol.clone(), order)
    }

    /// Open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let mut query = "order_status=OPEN".to_string();
        if let Some(s) = symbol {
            query.push_str("&product_id=");
            query.push_str(&Self::wire_symbol(s));
        }
        let value = self.signed_get("/api/v3/brokerage/orders/historical/batch", &query)?;
        let orders = value
            .get("orders")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing orders".to_string()))?;
        orders
            .iter()
            .map(|order| {
                let sym = symbol.cloned().unwrap_or_else(|| {
                    let raw = order
                        .get("product_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    raw.parse().unwrap_or_else(|_| Symbol::new(raw, ""))
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
        let value = self.signed_get("/api/v3/brokerage/accounts", "")?;
        let accounts = value
            .get("accounts")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing accounts".to_string()))?;
        accounts
            .iter()
            .map(|a| {
                let asset = a
                    .get("currency")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let free = nested_value(a, "available_balance");
                let hold = nested_value(a, "hold");
                Ok(Balance {
                    asset: asset.to_string(),
                    free: parse_decimal(&free)?,
                    locked: parse_decimal(&hold)?,
                })
            })
            .collect()
    }

    /// Build an ES256 JWT (optionally with a `uri` claim for REST requests).
    fn build_jwt(&self, uri: Option<String>) -> Result<String> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let private_key = creds
            .private_key
            .as_deref()
            .ok_or(Error::InvalidCredentials(
                "Coinbase requires an EC private key",
            ))?;
        let signing_key = SigningKey::from_pkcs8_pem(private_key)
            .map_err(|_| Error::InvalidCredentials("invalid EC private key"))?;
        let now = (self.now_ms)() / 1000;
        let nonce = format!("{:016x}", (self.now_ms)());
        let header = serde_json::json!({
            "alg": "ES256",
            "kid": creds.api_key,
            "typ": "JWT",
            "nonce": nonce,
        });
        let mut payload = serde_json::json!({
            "sub": creds.api_key,
            "iss": "cdp",
            "nbf": now,
            "exp": now + 120,
        });
        if let Some(uri) = uri {
            payload["uri"] = serde_json::json!(uri);
        }
        let signing_input = format!(
            "{}.{}",
            b64url(header.to_string().as_bytes()),
            b64url(payload.to_string().as_bytes())
        );
        let signature: Signature = signing_key.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            b64url(signature.to_bytes().as_slice())
        ))
    }

    fn signed_get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let jwt = self.build_jwt(Some(format!("GET {HOST}{path}")))?;
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let request = HttpRequest::get(url).with_header("Authorization", format!("Bearer {jwt}"));
        let response = self.http.execute(&request)?;
        parse_body(&response)
    }

    fn signed_post(&self, path: &str, body: &str) -> Result<serde_json::Value> {
        let jwt = self.build_jwt(Some(format!("POST {HOST}{path}")))?;
        let url = format!("{}{path}", self.rest_base);
        let request = HttpRequest::new(HttpMethod::Post, url)
            .with_header("Authorization", format!("Bearer {jwt}"))
            .with_header("Content-Type", "application/json")
            .with_body(body.to_string());
        let response = self.http.execute(&request)?;
        parse_body(&response)
    }
}

fn granularity(interval: &str) -> (&'static str, i64) {
    match interval {
        "1m" => ("ONE_MINUTE", 60),
        "5m" => ("FIVE_MINUTE", 300),
        "15m" => ("FIFTEEN_MINUTE", 900),
        "30m" => ("THIRTY_MINUTE", 1800),
        "2h" => ("TWO_HOUR", 7200),
        "6h" => ("SIX_HOUR", 21600),
        "1d" => ("ONE_DAY", 86400),
        _ => ("ONE_HOUR", 3600),
    }
}

fn parse_body(response: &HttpResponse) -> Result<serde_json::Value> {
    if response.is_success() {
        serde_json::from_str(&response.body).map_err(|e| Error::Deserialization(e.to_string()))
    } else {
        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let message = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Err(map_error(error, message, response.status))
    }
}

fn map_error(error: &str, message: &str, status: u16) -> Error {
    let text = format!("{error} {message}");
    if error.contains("INSUFFICIENT_FUND") || text.contains("Insufficient") {
        Error::InsufficientBalance
    } else if status == 401 || error.contains("UNAUTHORIZED") || text.contains("authentication") {
        Error::Auth(message.to_string())
    } else if status == 429 || error.contains("RATE") {
        Error::RateLimited { retry_after: None }
    } else if status == 404 || error.contains("NOT_FOUND") {
        Error::NotFound(message.to_string())
    } else if error.contains("INVALID") && text.contains("product") {
        Error::InvalidSymbol(message.to_string())
    } else {
        Error::Exchange {
            code: if error.is_empty() {
                status.to_string()
            } else {
                error.to_string()
            },
            message: message.to_string(),
        }
    }
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "OPEN" | "PENDING" | "QUEUED" => Ok(OrderStatus::New),
        "FILLED" => Ok(OrderStatus::Filled),
        "CANCELLED" | "CANCEL_QUEUED" => Ok(OrderStatus::Canceled),
        "EXPIRED" => Ok(OrderStatus::Expired),
        "FAILED" | "REJECTED" => Ok(OrderStatus::Rejected),
        other => Err(Error::Deserialization(format!("unknown status {other:?}"))),
    }
}

fn nonzero(value: Decimal) -> Option<Decimal> {
    (value > Decimal::ZERO).then_some(value)
}

fn str_field<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization(format!("missing string field {key:?}")))
}

fn opt_dec(value: &serde_json::Value, key: &str) -> Decimal {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(|s| parse_decimal(s).ok())
        .unwrap_or(Decimal::ZERO)
}

/// Read a nested `{value, currency}` amount (Coinbase balances) as a string.
fn nested_value(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0")
        .to_string()
}

/// Order-book / pricebook levels as `{price, size}` objects.
fn object_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
    let array = value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Deserialization("missing levels".to_string()))?;
    array
        .iter()
        .map(|level| {
            Ok(BookLevel {
                price: parse_decimal(str_field(level, "price")?)?,
                quantity: parse_decimal(str_field(level, "size")?)?,
            })
        })
        .collect()
}

fn parse_candle(candle: &serde_json::Value) -> Result<Candle> {
    let ts = str_field(candle, "start")?
        .parse::<i64>()
        .map_err(|e| Error::Deserialization(format!("candle start not an integer: {e}")))?;
    let f = |key: &str| -> Result<f64> {
        str_field(candle, key)?
            .parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("candle {key} not a number: {e}")))
    };
    Candle::new(
        f("open")?,
        f("high")?,
        f("low")?,
        f("close")?,
        f("volume")?,
        ts,
    )
    .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_value(symbol: Symbol, order: &serde_json::Value) -> Result<Order> {
    let side = str_field(order, "side")?;
    let status = str_field(order, "status")?;
    let config = order.get("order_configuration");
    let (order_type, limit_price) = if let Some(limit) = config.and_then(|c| {
        c.get("limit_limit_gtc")
            .or_else(|| c.get("limit_limit_gtd"))
    }) {
        (OrderType::Limit, opt_dec(limit, "limit_price"))
    } else {
        (OrderType::Market, Decimal::ZERO)
    };
    let quantity = config
        .and_then(|c| c.as_object())
        .and_then(|obj| obj.values().next())
        .map_or(Decimal::ZERO, |cfg| opt_dec(cfg, "base_size"));
    Ok(Order {
        id: str_field(order, "order_id")?.to_string(),
        client_order_id: order
            .get("client_order_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        symbol,
        side: parse_side(side)?,
        order_type,
        status: parse_status(status)?,
        quantity,
        filled_quantity: opt_dec(order, "filled_size"),
        price: nonzero(limit_price),
        average_price: nonzero(opt_dec(order, "average_filled_price")),
    })
}

fn parse_ws_message(text: &str) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let Some(channel) = value.get("channel").and_then(serde_json::Value::as_str) else {
        return Ok(Vec::new());
    };
    let empty = Vec::new();
    let events = value
        .get("events")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let mut out = Vec::new();

    match channel {
        "market_trades" => {
            for event in events {
                let trades = event.get("trades").and_then(serde_json::Value::as_array);
                for trade in trades.unwrap_or(&empty) {
                    let product = str_field(trade, "product_id")?;
                    out.push(Event::Trade(TradePrint {
                        symbol: product.parse().unwrap_or_else(|_| Symbol::new(product, "")),
                        price: parse_decimal(str_field(trade, "price")?)?,
                        quantity: parse_decimal(str_field(trade, "size")?)?,
                        aggressor: parse_side(str_field(trade, "side")?)?,
                        timestamp: 0,
                    }));
                }
            }
            Ok(out)
        }
        "ticker" => {
            for event in events {
                let tickers = event.get("tickers").and_then(serde_json::Value::as_array);
                for ticker in tickers.unwrap_or(&empty) {
                    let product = str_field(ticker, "product_id")?;
                    out.push(Event::Ticker(Ticker {
                        symbol: product.parse().unwrap_or_else(|_| Symbol::new(product, "")),
                        last: parse_decimal(str_field(ticker, "price")?)?,
                        bid: opt_dec(ticker, "best_bid"),
                        ask: opt_dec(ticker, "best_ask"),
                        volume: opt_dec(ticker, "volume_24_h"),
                    }));
                }
            }
            Ok(out)
        }
        "l2_data" => {
            for event in events {
                let product = event
                    .get("product_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let symbol = product.parse().unwrap_or_else(|_| Symbol::new(product, ""));
                let mut bids = Vec::new();
                let mut asks = Vec::new();
                if let Some(updates) = event.get("updates").and_then(serde_json::Value::as_array) {
                    for update in updates {
                        let level = BookLevel {
                            price: parse_decimal(str_field(update, "price_level")?)?,
                            quantity: parse_decimal(str_field(update, "new_quantity")?)?,
                        };
                        match update.get("side").and_then(serde_json::Value::as_str) {
                            Some("bid") => bids.push(level),
                            _ => asks.push(level),
                        }
                    }
                }
                out.push(Event::BookDelta(BookDelta {
                    symbol,
                    first_update_id: 0,
                    final_update_id: 0,
                    bids,
                    asks,
                }));
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

impl MarketData for Coinbase {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Coinbase::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Coinbase::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Coinbase::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Coinbase::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Coinbase::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Coinbase::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Coinbase::poll_events(self)
    }
}

impl Execution for Coinbase {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Coinbase::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Coinbase::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Coinbase::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Coinbase::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Coinbase::balances(self)
    }
}

impl Exchange for Coinbase {
    fn name(&self) -> &'static str {
        "coinbase"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{MockHttpTransport, MockWsTransport};
    use p256::ecdsa::{signature::Verifier, VerifyingKey};
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    // A fixed P-256 PKCS#8 test key (not a real credential).
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgZZ/YugITtxORUz74\n\
wHvqY4aizCFHQFTVNQCzDGy8/TOhRANCAAS69zNVQjOQ4RgxJVI8esP+jMfHLSTw\n\
2iVqo0qWlda/1D2jN4O3zcv4juQF5iE4pU5qPkeECTgsKSIYwZaMVMyO\n\
-----END PRIVATE KEY-----\n";

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
        Symbol::new("BTC", "USD")
    }

    fn signed_client(now_ms: i64) -> (Coinbase, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let coinbase = Coinbase::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("organizations/x/apiKeys/y", "unused").with_private_key(TEST_KEY),
        )
        .with_clock(Box::new(move || now_ms));
        (coinbase, mock)
    }

    #[test]
    fn jwt_is_valid_es256_and_carries_uri() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"trades":[{"price":"20000"}],"best_bid":"19999","best_ask":"20001"}"#,
        );
        coinbase.ticker(&symbol()).unwrap();

        let auth = mock.recorded_requests()[0]
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.clone())
            .unwrap();
        let jwt = auth.strip_prefix("Bearer ").unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Header + payload decode and carry the expected claims.
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: serde_json::Value =
            serde_json::from_slice(&engine.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        let payload: serde_json::Value =
            serde_json::from_slice(&engine.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(
            payload["uri"],
            "GET api.coinbase.com/api/v3/brokerage/products/BTC-USD/ticker"
        );
        assert_eq!(payload["iss"], "cdp");

        // The signature verifies against the public key derived from the test key.
        let signing_key = SigningKey::from_pkcs8_pem(TEST_KEY).unwrap();
        let verifying_key = VerifyingKey::from(&signing_key);
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = engine.decode(parts[2]).unwrap();
        let signature = Signature::from_slice(&sig_bytes).unwrap();
        assert!(verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .is_ok());
    }

    #[test]
    fn ticker_parses() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"trades":[{"price":"20000.5"}],"best_bid":"19999","best_ask":"20001"}"#,
        );
        let ticker = coinbase.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(19999));
    }

    #[test]
    fn klines_reversed() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"candles":[
            {"start":"1700003600","low":"104","high":"106","open":"105","close":"105.5","volume":"2"},
            {"start":"1700000000","low":"95","high":"110","open":"100","close":"105","volume":"12"}]}"#,
        );
        let candles = coinbase.klines(&symbol(), "1h", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
    }

    #[test]
    fn order_book_parses_object_levels() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"pricebook":{"bids":[{"price":"100","size":"1"}],"asks":[{"price":"101","size":"2"}]}}"#,
        );
        let book = coinbase.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
    }

    #[test]
    fn place_order_limit_configuration() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"success":true,"success_response":{"order_id":"OID-1"}}"#,
        );
        let order = coinbase
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "OID-1");
        let reqs = mock.recorded_requests();
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""limit_limit_gtc""#));
        assert!(body.contains(r#""side":"BUY""#));
    }

    #[test]
    fn place_order_rejection() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"success":false,"error_response":{"error":"INSUFFICIENT_FUND","message":"no"}}"#,
        );
        assert!(matches!(
            coinbase
                .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn query_order_and_balances() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"order":{"order_id":"OID-1","client_order_id":"","product_id":"BTC-USD","side":"SELL",
            "status":"FILLED","filled_size":"2","average_filled_price":"100",
            "order_configuration":{"market_market_ioc":{"base_size":"2"}}}}"#,
        );
        let order = coinbase.query_order(&symbol(), "OID-1").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.average_price, Some(dec!(100)));

        mock.push_json(
            200,
            r#"{"accounts":[{"currency":"USDC","available_balance":{"value":"100.5"},"hold":{"value":"25.5"}}]}"#,
        );
        let bals = coinbase.balances().unwrap();
        assert_eq!(bals[0].asset, "USDC");
        assert_eq!(bals[0].total(), dec!(126));
    }

    #[test]
    fn signed_requires_private_key() {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let coinbase = Coinbase::with_credentials(
            Box::new(ArcTransport(mock)),
            &opts,
            Credentials::new("k", "s"),
        );
        assert!(matches!(
            coinbase.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trades_and_l2() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"channel":"market_trades","events":[{"type":"update","trades":[
                {"product_id":"BTC-USD","price":"100","size":"0.5","side":"BUY"}]}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"channel":"l2_data","events":[{"type":"update","product_id":"BTC-USD","updates":[
                {"side":"bid","price_level":"100","new_quantity":"1"},
                {"side":"offer","price_level":"101","new_quantity":"2"}]}]}"#
                    .to_string(),
            )),
        ]);
        let (_c, _m) = signed_client(1_000_000);
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut coinbase = Coinbase::with_credentials(
            Box::new(ArcTransport(mock)),
            &opts,
            Credentials::new("kid", "s").with_private_key(TEST_KEY),
        )
        .with_clock(Box::new(|| 1_000_000))
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        coinbase.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""channel":"market_trades""#));
        assert!(ws.sent()[0].contains(r#""jwt":"#));

        let events = coinbase.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        let Event::BookDelta(d) = &events[1] else {
            panic!("expected book delta")
        };
        assert_eq!(d.bids.len(), 1);
        assert_eq!(d.asks.len(), 1);
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (coinbase, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"success":true,"success_response":{"order_id":"O1"}}"#,
        );
        let mut exchange: Box<dyn Exchange> = Box::new(coinbase);
        assert_eq!(exchange.name(), "coinbase");
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
