//! HTX (formerly Huobi) — the seventh exchange.
//!
//! HTX signs AWS-style: the signature is
//! `base64(HMAC-SHA256(secret, METHOD\nhost\npath\nsorted-urlencoded-params))`,
//! where the params include `AccessKeyId`, `SignatureMethod=HmacSHA256`,
//! `SignatureVersion=2` and an ISO-8601 (no-millis) `Timestamp`, and the
//! signature is appended to the query as `Signature=`. Symbols are lowercase and
//! concatenated (`btcusdt`); market-data JSON encodes numbers as JSON numbers
//! (not strings); orders require a spot `account-id` (fetched and cached). The
//! response envelope carries `status: "ok"` with the payload under `tick` or
//! `data`.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::ExchangeOptions;
use crate::signing::hmac_sha256_base64;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};
use rust_decimal::Decimal;
use std::cell::RefCell;
use std::collections::HashMap;
use wickra_core::Candle;

const HOST: &str = "api.huobi.pro";

fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// Unix milliseconds to an ISO-8601 UTC timestamp without milliseconds
/// (`YYYY-MM-DDTHH:MM:SS`), the form HTX signs.
fn iso8601_no_millis(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let mut rem = ms.rem_euclid(86_400_000) / 1000;
    let secs = rem % 60;
    rem /= 60;
    let mins = rem % 60;
    let hours = rem / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Percent-encode per RFC 3986 (unreserved characters pass through).
fn encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX[(b >> 4) as usize]));
                out.push(char::from(HEX[(b & 0x0f) as usize]));
            }
        }
    }
    out
}

/// An HTX client over injected transports.
pub struct Htx {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    account_id: RefCell<Option<String>>,
}

impl Htx {
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
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            account_id: RefCell::new(None),
        }
    }

    /// Build a public HTX client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated HTX client.
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

    /// The HTX wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `btcusdt`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated().to_lowercase()
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        let value = self.get("/market/detail/merged", &query)?;
        let tick = value
            .get("tick")
            .ok_or_else(|| Error::Deserialization("missing tick".to_string()))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: decimal_field(tick, "close")?,
            bid: decimal_at(tick, "bid", 0)?,
            ask: decimal_at(tick, "ask", 0)?,
            volume: decimal_field(tick, "vol")?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). HTX returns
    /// newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "symbol={}&period={}&size={limit}",
            Self::wire_symbol(symbol),
            map_period(interval),
        );
        let value = self.get("/market/history/kline", &query)?;
        let data = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing kline data".to_string()))?;
        let mut candles = data
            .iter()
            .map(parse_kline_obj)
            .collect::<Result<Vec<_>>>()?;
        candles.reverse();
        Ok(candles)
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, _depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("symbol={}&type=step0", Self::wire_symbol(symbol));
        let value = self.get("/market/depth", &query)?;
        let tick = value
            .get("tick")
            .ok_or_else(|| Error::Deserialization("missing tick".to_string()))?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: tick
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            bids: num_levels(tick.get("bids"))?,
            asks: num_levels(tick.get("asks"))?,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("market.{}.trade.detail", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("market.{}.depth.step0", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("market.{}.ticker", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    fn subscribe(&mut self, symbol: &Symbol, topic: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://api.huobi.pro/ws")?;
            self.connection = Some(connection);
        }
        let message = format!(r#"{{"sub":"{topic}","id":"{wire}"}}"#);
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
    /// (The real adapter gunzips HTX frames before they reach the parser.)
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
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        let url = "wss://api.huobi.pro/ws";
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// The spot `account-id`, fetched once and cached.
    fn account_id(&self) -> Result<String> {
        if let Some(id) = self.account_id.borrow().clone() {
            return Ok(id);
        }
        let value = self.signed_request(HttpMethod::Get, "/v1/account/accounts", &[], "")?;
        let accounts = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing accounts".to_string()))?;
        let id = accounts
            .iter()
            .find(|a| a.get("type").and_then(serde_json::Value::as_str) == Some("spot"))
            .and_then(|a| a.get("id"))
            .map(|id| {
                id.as_u64()
                    .map_or_else(|| id.as_str().unwrap_or("").to_string(), |n| n.to_string())
            })
            .ok_or_else(|| Error::NotFound("no spot account".to_string()))?;
        *self.account_id.borrow_mut() = Some(id.clone());
        Ok(id)
    }

    /// Place an order.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let account_id = self.account_id()?;
        let order_kind = format!(
            "{}-{}",
            side_str(request.side),
            order_type_str(request.order_type)
        );
        let mut body = serde_json::json!({
            "account-id": account_id,
            "symbol": Self::wire_symbol(&request.symbol),
            "type": order_kind,
            "amount": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            body["client-order-id"] = serde_json::json!(id.clone());
        }
        let value = self.signed_request(
            HttpMethod::Post,
            "/v1/order/orders/place",
            &[],
            &body.to_string(),
        )?;
        let order_id = value
            .get("data")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing order id".to_string()))?;
        Ok(Order {
            id: order_id.to_string(),
            client_order_id: request.client_order_id.clone(),
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
        let path = format!("/v1/order/orders/{order_id}/submitcancel");
        self.signed_request(HttpMethod::Post, &path, &[], "{}")?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let path = format!("/v1/order/orders/{order_id}");
        let value = self.signed_request(HttpMethod::Get, &path, &[], "")?;
        let data = value
            .get("data")
            .ok_or_else(|| Error::Deserialization("missing order data".to_string()))?;
        order_from_value(symbol.clone(), data)
    }

    /// Open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let account_id = self.account_id()?;
        let wire = symbol.map(Self::wire_symbol);
        let mut params: Vec<(&str, &str)> = vec![("account-id", &account_id)];
        if let Some(w) = &wire {
            params.push(("symbol", w));
        }
        let value = self.signed_request(HttpMethod::Get, "/v1/order/openOrders", &params, "")?;
        let data = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing orders".to_string()))?;
        data.iter()
            .map(|order| {
                let sym = symbol.cloned().unwrap_or_else(|| {
                    let raw = order
                        .get("symbol")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    split_wire_symbol(raw)
                });
                order_from_value(sym, order)
            })
            .collect()
    }

    /// Spot account balances (HTX reports `trade` and `frozen` entries per asset).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let account_id = self.account_id()?;
        let path = format!("/v1/account/accounts/{account_id}/balance");
        let value = self.signed_request(HttpMethod::Get, &path, &[], "")?;
        let list = value
            .get("data")
            .and_then(|d| d.get("list"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing balance list".to_string()))?;
        let mut merged: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        for entry in list {
            let currency = entry
                .get("currency")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let kind = entry
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let amount = decimal_field(entry, "balance").unwrap_or(Decimal::ZERO);
            let slot = merged.entry(currency.to_string()).or_default();
            if kind == "frozen" {
                slot.1 += amount;
            } else {
                slot.0 += amount;
            }
        }
        let mut balances: Vec<Balance> = merged
            .into_iter()
            .map(|(asset, (free, locked))| Balance {
                asset,
                free,
                locked,
            })
            .collect();
        balances.sort_by(|a, b| a.asset.cmp(&b.asset));
        Ok(balances)
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_status(&response.body)
    }

    /// Sign AWS-style and issue the request. Auth params live in the query and are
    /// the only signed material (a POST body is sent unsigned as JSON).
    fn signed_request(
        &self,
        method: HttpMethod,
        path: &str,
        extra: &[(&str, &str)],
        body: &str,
    ) -> Result<serde_json::Value> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let mut params: Vec<(String, String)> = vec![
            ("AccessKeyId".to_string(), creds.api_key.clone()),
            ("SignatureMethod".to_string(), "HmacSHA256".to_string()),
            ("SignatureVersion".to_string(), "2".to_string()),
            ("Timestamp".to_string(), iso8601_no_millis((self.now_ms)())),
        ];
        for (key, val) in extra {
            params.push(((*key).to_string(), (*val).to_string()));
        }
        params.sort_by(|a, b| a.0.cmp(&b.0));
        let encoded = params
            .iter()
            .map(|(key, val)| format!("{}={}", encode(key), encode(val)))
            .collect::<Vec<_>>()
            .join("&");
        let canonical = format!("{}\n{HOST}\n{path}\n{encoded}", method.as_str());
        let signature = hmac_sha256_base64(creds.api_secret.as_bytes(), canonical.as_bytes());
        let url = format!(
            "{}{path}?{encoded}&Signature={}",
            self.rest_base,
            encode(&signature)
        );
        let mut request = HttpRequest::new(method, url);
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_status(&response.body)
    }
}

fn map_period(interval: &str) -> String {
    match interval {
        "1m" => "1min",
        "5m" => "5min",
        "15m" => "15min",
        "30m" => "30min",
        "1h" => "60min",
        "4h" => "4hour",
        "1d" => "1day",
        "1w" => "1week",
        other => other,
    }
    .to_string()
}

fn unwrap_status(body: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if value.get("status").and_then(serde_json::Value::as_str) == Some("ok") {
        Ok(value)
    } else {
        let code = value
            .get("err-code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let message = value
            .get("err-msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Err(map_error(code, message))
    }
}

fn map_error(code: &str, message: &str) -> Error {
    match code {
        "request-rate-limit" | "rate-limit" => Error::RateLimited { retry_after: None },
        "api-signature-not-valid" | "api-key-not-valid" | "invalid-signature" => {
            Error::Auth(message.to_string())
        }
        "account-frozen-balance-insufficient-error"
        | "order-value-min-error"
        | "insufficient-balance" => Error::InsufficientBalance,
        "base-symbol-error" | "invalid-symbol" => Error::InvalidSymbol(message.to_string()),
        "base-record-invalid" | "order-not-found" => Error::NotFound(message.to_string()),
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
    // HTX order `type` is `<side>-<kind>` (e.g. `buy-limit`).
    match raw.split('-').next() {
        Some("buy") => Ok(OrderSide::Buy),
        Some("sell") => Ok(OrderSide::Sell),
        _ => Err(Error::Deserialization(format!(
            "unknown order type {raw:?}"
        ))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    if raw.contains("market") {
        Ok(OrderType::Market)
    } else if raw.contains("limit") {
        Ok(OrderType::Limit)
    } else {
        Err(Error::Deserialization(format!(
            "unknown order type {raw:?}"
        )))
    }
}

fn parse_state(raw: &str) -> Result<OrderStatus> {
    match raw {
        "submitted" | "created" => Ok(OrderStatus::New),
        "partial-filled" => Ok(OrderStatus::PartiallyFilled),
        "filled" => Ok(OrderStatus::Filled),
        "canceled" | "partial-canceled" | "cancelling" => Ok(OrderStatus::Canceled),
        other => Err(Error::Deserialization(format!("unknown state {other:?}"))),
    }
}

fn nonzero_decimal(raw: &str) -> Option<Decimal> {
    crate::normalize::parse_opt_decimal(Some(raw))
        .ok()
        .flatten()
        .filter(|d| *d > Decimal::ZERO)
}

/// Read a decimal that HTX may encode as a JSON number or string.
fn decimal_field(value: &serde_json::Value, key: &str) -> Result<Decimal> {
    let field = value
        .get(key)
        .ok_or_else(|| Error::Deserialization(format!("missing field {key:?}")))?;
    decimal_value(field)
}

fn decimal_value(field: &serde_json::Value) -> Result<Decimal> {
    match field {
        serde_json::Value::String(s) => parse_decimal(s),
        serde_json::Value::Number(n) => parse_decimal(&n.to_string()),
        other => Err(Error::Deserialization(format!("not a number: {other}"))),
    }
}

/// Read `value[key][index]` as a decimal (HTX `bid`/`ask` are `[price, size]`).
fn decimal_at(value: &serde_json::Value, key: &str, index: usize) -> Result<Decimal> {
    let field = value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.get(index))
        .ok_or_else(|| Error::Deserialization(format!("missing {key}[{index}]")))?;
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

fn num_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
    let array = value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Deserialization("missing depth levels".to_string()))?;
    array
        .iter()
        .map(|level| {
            let pair = level
                .as_array()
                .ok_or_else(|| Error::Deserialization("depth level not an array".to_string()))?;
            let price = decimal_value(
                pair.first()
                    .ok_or_else(|| Error::Deserialization("depth price missing".to_string()))?,
            )?;
            let quantity =
                decimal_value(pair.get(1).ok_or_else(|| {
                    Error::Deserialization("depth quantity missing".to_string())
                })?)?;
            Ok(BookLevel { price, quantity })
        })
        .collect()
}

fn parse_kline_obj(obj: &serde_json::Value) -> Result<Candle> {
    let ts = obj
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| Error::Deserialization("kline id missing".to_string()))?;
    Candle::new(
        f64_field(obj, "open")?,
        f64_field(obj, "high")?,
        f64_field(obj, "low")?,
        f64_field(obj, "close")?,
        f64_field(obj, "amount")?,
        ts,
    )
    .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_value(symbol: Symbol, data: &serde_json::Value) -> Result<Order> {
    let kind = data
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let state = data
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let id = data
        .get("id")
        .map(|id| {
            id.as_u64()
                .map_or_else(|| id.as_str().unwrap_or("").to_string(), |n| n.to_string())
        })
        .unwrap_or_default();
    let client_order_id = data
        .get("client-order-id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let filled = decimal_field(data, "field-amount").unwrap_or(Decimal::ZERO);
    Ok(Order {
        id,
        client_order_id,
        symbol,
        side: parse_side(kind)?,
        order_type: parse_order_type(kind)?,
        status: parse_state(state)?,
        quantity: decimal_field(data, "amount")?,
        filled_quantity: filled,
        price: nonzero_decimal(
            &decimal_field(data, "price")
                .unwrap_or(Decimal::ZERO)
                .to_string(),
        ),
        average_price: None,
    })
}

const KNOWN_QUOTES: &[&str] = &["usdt", "usdc", "eur", "btc", "eth", "usd"];

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

fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let Some(channel) = value.get("ch").and_then(serde_json::Value::as_str) else {
        return Ok(Vec::new()); // ping / sub-ack
    };
    // channel = "market.<symbol>.<stream>...".
    let mut parts = channel.split('.');
    let _market = parts.next();
    let wire = parts.next().unwrap_or("");
    let stream = parts.next().unwrap_or("");
    let symbol = resolve(wire);
    let null = serde_json::Value::Null;
    let tick = value.get("tick").unwrap_or(&null);

    match stream {
        "trade" => {
            let data = tick.get("data").and_then(serde_json::Value::as_array);
            let empty = Vec::new();
            data.unwrap_or(&empty)
                .iter()
                .map(|t| {
                    let side = t
                        .get("direction")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    Ok(Event::Trade(TradePrint {
                        symbol: symbol.clone(),
                        price: decimal_field(t, "price")?,
                        quantity: decimal_field(t, "amount")?,
                        aggressor: parse_side(side)?,
                        timestamp: t.get("ts").and_then(serde_json::Value::as_i64).unwrap_or(0),
                    }))
                })
                .collect()
        }
        "ticker" => Ok(vec![Event::Ticker(Ticker {
            symbol,
            last: decimal_field(tick, "lastPrice").or_else(|_| decimal_field(tick, "close"))?,
            bid: decimal_field(tick, "bid").unwrap_or(Decimal::ZERO),
            ask: decimal_field(tick, "ask").unwrap_or(Decimal::ZERO),
            volume: decimal_field(tick, "vol").unwrap_or(Decimal::ZERO),
        })]),
        "depth" => {
            let update_id = tick
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            Ok(vec![Event::BookDelta(BookDelta {
                symbol,
                first_update_id: update_id,
                final_update_id: update_id,
                bids: num_levels(tick.get("bids"))?,
                asks: num_levels(tick.get("asks"))?,
            })])
        }
        _ => Ok(Vec::new()),
    }
}

impl MarketData for Htx {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Htx::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Htx::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Htx::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Htx::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Htx::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Htx::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Htx::poll_events(self)
    }
}

impl Execution for Htx {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Htx::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Htx::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Htx::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Htx::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Htx::balances(self)
    }
}

impl Exchange for Htx {
    fn name(&self) -> &'static str {
        "htx"
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

    fn client() -> (Htx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            Htx::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Htx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let htx = Htx::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (htx, mock)
    }

    #[test]
    fn iso_and_encode() {
        assert_eq!(iso8601_no_millis(0), "1970-01-01T00:00:00");
        assert_eq!(iso8601_no_millis(1_700_000_000_000), "2023-11-14T22:13:20");
        assert_eq!(encode("2023-11-14T22:13:20"), "2023-11-14T22%3A13%3A20");
        assert_eq!(encode("a+b/c=d"), "a%2Bb%2Fc%3Dd");
    }

    #[test]
    fn ticker_reads_numeric_json() {
        let (htx, mock) = client();
        mock.push_json(
            200,
            r#"{"status":"ok","tick":{"close":20000.5,"bid":[19999.0,1.0],"ask":[20001.0,2.0],"vol":1234.0}}"#,
        );
        let ticker = htx.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.ask, dec!(20001));
        assert_eq!(
            mock.recorded_requests()[0].url,
            "https://api.huobi.pro/market/detail/merged?symbol=btcusdt"
        );
    }

    #[test]
    fn klines_reversed_named_fields() {
        let (htx, mock) = client();
        mock.push_json(
            200,
            r#"{"status":"ok","data":[
            {"id":1700000060,"open":105,"close":105.5,"low":104,"high":106,"amount":2},
            {"id":1700000000,"open":100,"close":105,"low":95,"high":110,"amount":12}]}"#,
        );
        let candles = htx.klines(&symbol(), "1h", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!((candles[0].low - 95.0).abs() < 1e-9);
    }

    #[test]
    fn order_book_numeric_levels() {
        let (htx, mock) = client();
        mock.push_json(
            200,
            r#"{"status":"ok","tick":{"version":77,"bids":[[100.0,1.5]],"asks":[[101.0,2.0]]}}"#,
        );
        let book = htx.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 77);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1.5)));
    }

    #[test]
    fn error_status_maps() {
        let (htx, mock) = client();
        mock.push_json(
            200,
            r#"{"status":"error","err-code":"account-frozen-balance-insufficient-error","err-msg":"no"}"#,
        );
        assert!(matches!(
            htx.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn place_order_fetches_account_and_signs() {
        let (htx, mock) = signed_client(0);
        // First the account lookup, then the order.
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"id":42,"type":"spot","state":"working"}]}"#,
        );
        mock.push_json(200, r#"{"status":"ok","data":"777"}"#);
        let order = htx
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "777");
        assert_eq!(order.status, OrderStatus::New);

        let place = &mock.recorded_requests()[1];
        assert_eq!(place.method, HttpMethod::Post);
        // Reconstruct the AWS-style signature over the sorted auth params.
        let ts = "1970-01-01T00:00:00";
        let encoded = format!(
            "AccessKeyId=APIKEY&SignatureMethod=HmacSHA256&SignatureVersion=2&Timestamp={}",
            encode(ts)
        );
        let canonical = format!("POST\napi.huobi.pro\n/v1/order/orders/place\n{encoded}");
        let sign = hmac_sha256_base64(b"SECRET", canonical.as_bytes());
        assert!(place.url.contains(&format!("Signature={}", encode(&sign))));
        // The account-id was cached (order body carries it).
        assert!(place
            .body
            .as_ref()
            .unwrap()
            .contains(r#""account-id":"42""#));
    }

    #[test]
    fn query_order_parses_type_and_state() {
        let (htx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"id":777,"client-order-id":"","symbol":"btcusdt",
            "type":"sell-limit","state":"filled","amount":"2","field-amount":"2","price":"100"}}"#,
        );
        let order = htx.query_order(&symbol(), "777").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.filled_quantity, dec!(2));
    }

    #[test]
    fn balances_merge_trade_and_frozen() {
        let (htx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"id":42,"type":"spot","state":"working"}]}"#,
        );
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"list":[
            {"currency":"usdt","type":"trade","balance":"100.5"},
            {"currency":"usdt","type":"frozen","balance":"25.5"}]}}"#,
        );
        let bals = htx.balances().unwrap();
        assert_eq!(bals.len(), 1);
        assert_eq!(bals[0].asset, "usdt");
        assert_eq!(bals[0].free, dec!(100.5));
        assert_eq!(bals[0].locked, dec!(25.5));
    }

    #[test]
    fn signed_requires_credentials() {
        let (htx, _) = client();
        assert!(matches!(
            htx.query_order(&symbol(), "1").unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trade_and_depth() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"ch":"market.btcusdt.trade.detail","tick":{"data":[
                {"price":100.0,"amount":0.5,"direction":"buy","ts":1}]}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"ch":"market.btcusdt.depth.step0","tick":{"version":9,
                "bids":[[100.0,1.0]],"asks":[[101.0,2.0]]}}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"ping":123}"#.to_string())),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut htx = Htx::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        htx.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""sub":"market.btcusdt.trade.detail""#));

        let events = htx.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert!(matches!(events[1], Event::BookDelta(_)));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (htx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"id":42,"type":"spot","state":"working"}]}"#,
        );
        mock.push_json(200, r#"{"status":"ok","data":"1"}"#);
        let mut exchange: Box<dyn Exchange> = Box::new(htx);
        assert_eq!(exchange.name(), "htx");
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
