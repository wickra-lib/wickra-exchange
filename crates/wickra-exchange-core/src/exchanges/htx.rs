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
//!
//! When built with a futures [`MarketType`](crate::MarketType), the client
//! targets the USDT-margined swap host (`api.hbdm.com`): market data via
//! `/linear-swap-ex/market/*` (same tick/data shapes as spot, so the parsers are
//! reused), and the [`Derivatives`] trait plus `place_order`/`balances` via the
//! **cross-margin** `/linear-swap-api/v1/swap_cross_*` family, where orders carry
//! an integer contract `volume`, a `direction`, an `offset` (open/close) and a
//! `lever_rate`. `query_order`/`cancel_order`/`open_orders` route to the
//! cross-swap order endpoints (`swap_cross_order_info` / `swap_cross_cancel` /
//! `swap_cross_openorders`) with the swap order shape (`order_id_str`,
//! `direction`, numeric `status`). `set_margin_mode(Isolated)` is unsupported
//! within the cross family — a documented gap.
//!
//! [`AdvancedOrders`]: native spot batch place/cancel (`/v1/order/batch-orders`,
//! `/v1/order/orders/batchcancel`, per-order `err-code`). HTX has no STP field,
//! no in-place amend and no OCO order-list, so `amend_order`/`place_oco` and STP
//! are documented gaps.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType};
use crate::positions::{Position, PositionSide};
use crate::signing::hmac_sha256_base64;
use crate::symbol::Symbol;
use crate::traits::{
    AdvancedOrders, Derivatives, Exchange, Execution, MarketData, WsExecution, WsUserData,
};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{
    Balance, OcoRequest, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use wickra_core::Candle;

/// Spot API host.
const HOST: &str = "api.huobi.pro";
/// USDT-margined swap (futures) API host.
const FUTURES_HOST: &str = "api.hbdm.com";

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
    host: &'static str,
    market_type: MarketType,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    account_id: RefCell<Option<String>>,
    /// Leverage applied to futures orders (HTX sets `lever_rate` per order).
    leverage: Cell<u32>,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
    /// Set once the private stream is subscribed, so [`poll_events`](Self::poll_events)
    /// re-subscribes it after a drop.
    user_data_active: bool,
}

impl Htx {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        let host = if options.market_type.is_derivatives() {
            FUTURES_HOST
        } else {
            HOST
        };
        Self {
            http,
            ws: None,
            rest_base: format!("https://{host}"),
            host,
            market_type: options.market_type,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            account_id: RefCell::new(None),
            leverage: Cell::new(1),
            private_connection: None,
            user_data_active: false,
        }
    }

    /// Whether this client targets HTX USDT-margined swaps (`api.hbdm.com`,
    /// `/linear-swap-*`) rather than spot.
    fn is_futures(&self) -> bool {
        self.market_type.is_derivatives()
    }

    /// The HTX **swap** contract code for a canonical [`Symbol`]
    /// (`BTC/USDT` -> `BTC-USDT`, uppercase dash form).
    fn contract_code(symbol: &Symbol) -> String {
        format!(
            "{}-{}",
            symbol.base().to_uppercase(),
            symbol.quote().to_uppercase()
        )
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
        let (path, query) = if self.is_futures() {
            (
                "/linear-swap-ex/market/detail/merged",
                format!("contract_code={}", Self::contract_code(symbol)),
            )
        } else {
            (
                "/market/detail/merged",
                format!("symbol={}", Self::wire_symbol(symbol)),
            )
        };
        let value = self.get(path, &query)?;
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
        let (path, query) = if self.is_futures() {
            (
                "/linear-swap-ex/market/history/kline",
                format!(
                    "contract_code={}&period={}&size={limit}",
                    Self::contract_code(symbol),
                    map_period(interval),
                ),
            )
        } else {
            (
                "/market/history/kline",
                format!(
                    "symbol={}&period={}&size={limit}",
                    Self::wire_symbol(symbol),
                    map_period(interval),
                ),
            )
        };
        let value = self.get(path, &query)?;
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
        let (path, query) = if self.is_futures() {
            (
                "/linear-swap-ex/market/depth",
                format!("contract_code={}&type=step0", Self::contract_code(symbol)),
            )
        } else {
            (
                "/market/depth",
                format!("symbol={}&type=step0", Self::wire_symbol(symbol)),
            )
        };
        let value = self.get(path, &query)?;
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
        // Drain the private user-data (v2 `orders`/`accounts.update`) stream, if open.
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // A dropped private stream is re-subscribed with a fresh v2 auth handshake
        // (the signature is time-bound, so a stale replay would be rejected).
        if self.user_data_active
            && self
                .private_connection
                .as_ref()
                .is_some_and(|c| !c.is_connected())
        {
            events.push(Event::Disconnected);
            self.private_connection = None;
            if self.subscribe_user_data().is_ok() {
                events.push(Event::Reconnected);
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

    /// Open the private user-data stream (`wss://api.huobi.pro/ws/v2`).
    /// Authenticates with the v2 `req/auth` handshake (signature = base64(HMAC-SHA256)
    /// over `GET\napi.huobi.pro\n/ws/v2\n<sorted query>`), then subscribes to the
    /// `orders#*` and `accounts.update#2` channels. Afterwards
    /// [`poll_events`](Self::poll_events) also surfaces the account's own
    /// [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// A dropped private stream is re-subscribed automatically on the next
    /// [`poll_events`](Self::poll_events); call
    /// [`keepalive_user_data`](Self::keepalive_user_data) periodically to keep it
    /// from being dropped for inactivity.
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials,
    /// [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the request fails.
    pub fn subscribe_user_data(&mut self) -> Result<()> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "user-data stream requires credentials",
        ))?;
        let timestamp = iso8601_no_millis((self.now_ms)());
        // The v2 WS auth signs the same canonical form as REST, over `/ws/v2`.
        let mut params = [
            ("accessKey", creds.api_key.as_str()),
            ("signatureMethod", "HmacSHA256"),
            ("signatureVersion", "2.1"),
            ("timestamp", timestamp.as_str()),
        ];
        params.sort_by(|a, b| a.0.cmp(b.0));
        let encoded = params
            .iter()
            .map(|(key, val)| format!("{}={}", encode(key), encode(val)))
            .collect::<Vec<_>>()
            .join("&");
        let canonical = format!("GET\n{HOST}\n/ws/v2\n{encoded}");
        let signature = hmac_sha256_base64(creds.api_secret.as_bytes(), canonical.as_bytes());
        let auth = format!(
            r#"{{"action":"req","ch":"auth","params":{{"authType":"api","accessKey":"{}","signatureMethod":"HmacSHA256","signatureVersion":"2.1","timestamp":"{timestamp}","signature":"{signature}"}}}}"#,
            creds.api_key
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://api.huobi.pro/ws/v2")?;
        connection.send(&auth)?;
        connection.send(r#"{"action":"sub","ch":"orders#*"}"#)?;
        connection.send(r#"{"action":"sub","ch":"accounts.update#2"}"#)?;
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send the HTX v2 application-level heartbeat (`{"action":"ping"}`) on the
    /// private stream so it is not dropped for inactivity. A no-op before
    /// [`subscribe_user_data`](Self::subscribe_user_data).
    ///
    /// # Errors
    /// Returns an [`Error`] if the ping cannot be sent.
    pub fn keepalive_user_data(&mut self) -> Result<()> {
        if let Some(connection) = self.private_connection.as_mut() {
            connection.send(r#"{"action":"ping"}"#)?;
        }
        Ok(())
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
        if self.is_futures() {
            return self.place_futures_order(request);
        }
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
    pub fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<()> {
        if self.is_futures() {
            let body = serde_json::json!({
                "contract_code": Self::contract_code(symbol),
                "order_id": order_id,
            });
            self.signed_request(
                HttpMethod::Post,
                "/linear-swap-api/v1/swap_cross_cancel",
                &[],
                &body.to_string(),
            )?;
            return Ok(());
        }
        let path = format!("/v1/order/orders/{order_id}/submitcancel");
        self.signed_request(HttpMethod::Post, &path, &[], "{}")?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        if self.is_futures() {
            let body = serde_json::json!({
                "contract_code": Self::contract_code(symbol),
                "order_id": order_id,
            });
            let value = self.signed_request(
                HttpMethod::Post,
                "/linear-swap-api/v1/swap_cross_order_info",
                &[],
                &body.to_string(),
            )?;
            let order = value
                .get("data")
                .and_then(serde_json::Value::as_array)
                .and_then(|a| a.first())
                .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
            return swap_order_from_value(symbol.clone(), order);
        }
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
        if self.is_futures() {
            let sym = symbol.ok_or(Error::InvalidOrder(
                "HTX futures open_orders requires a symbol",
            ))?;
            let body = serde_json::json!({ "contract_code": Self::contract_code(sym) });
            let value = self.signed_request(
                HttpMethod::Post,
                "/linear-swap-api/v1/swap_cross_openorders",
                &[],
                &body.to_string(),
            )?;
            let orders = value
                .get("data")
                .and_then(|d| d.get("orders"))
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| Error::Deserialization("missing orders".to_string()))?;
            return orders
                .iter()
                .map(|order| swap_order_from_value(sym.clone(), order))
                .collect();
        }
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
        if self.is_futures() {
            return self.futures_balances();
        }
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

    /// Place a USDT-margined swap order via the cross-margin family
    /// (`/linear-swap-api/v1/swap_cross_order`). Orders carry an integer contract
    /// `volume`, a `direction` (buy/sell), an `offset` (open/close; reduce-only
    /// closes), the stored `lever_rate`, and an `order_price_type`
    /// (`optimal_5` for market, `limit` otherwise).
    fn place_futures_order(&self, request: &OrderRequest) -> Result<Order> {
        let volume = decimal_to_contracts(request.quantity)?;
        let mut body = serde_json::json!({
            "contract_code": Self::contract_code(&request.symbol),
            "direction": side_str(request.side),
            "offset": if request.reduce_only { "close" } else { "open" },
            "lever_rate": self.leverage.get(),
            "volume": volume,
        });
        match request.order_type {
            OrderType::Market | OrderType::StopMarket => {
                body["order_price_type"] = serde_json::json!("optimal_5");
            }
            OrderType::Limit | OrderType::StopLimit => {
                let price = request
                    .price
                    .ok_or(Error::InvalidOrder("limit order requires a price"))?;
                body["order_price_type"] = serde_json::json!("limit");
                body["price"] = serde_json::json!(format_decimal(price));
            }
        }
        if let Some(id) = &request.client_order_id {
            if let Ok(numeric) = id.parse::<u64>() {
                body["client_order_id"] = serde_json::json!(numeric);
            }
        }
        let value = self.signed_request(
            HttpMethod::Post,
            "/linear-swap-api/v1/swap_cross_order",
            &[],
            &body.to_string(),
        )?;
        let data = value
            .get("data")
            .ok_or_else(|| Error::Deserialization("missing order data".to_string()))?;
        let order_id = data
            .get("order_id_str")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                data.get("order_id")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n.to_string())
            })
            .ok_or_else(|| Error::Deserialization("missing order id".to_string()))?;
        Ok(Order {
            id: order_id,
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

    /// USDT-margined cross account balance
    /// (`/linear-swap-api/v1/swap_cross_account_info`).
    fn futures_balances(&self) -> Result<Vec<Balance>> {
        let value = self.signed_request(
            HttpMethod::Post,
            "/linear-swap-api/v1/swap_cross_account_info",
            &[],
            "{}",
        )?;
        let list = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing account data".to_string()))?;
        Ok(list
            .iter()
            .map(|acct| {
                let asset = acct
                    .get("margin_asset")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("USDT")
                    .to_string();
                Balance {
                    asset,
                    free: decimal_field(acct, "margin_available").unwrap_or(Decimal::ZERO),
                    locked: decimal_field(acct, "margin_frozen").unwrap_or(Decimal::ZERO),
                }
            })
            .collect())
    }

    /// Open positions on the USDT-margined cross account
    /// (`/linear-swap-api/v1/swap_cross_position_info`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let body = symbol.map_or_else(
            || "{}".to_string(),
            |s| format!(r#"{{"contract_code":"{}"}}"#, Self::contract_code(s)),
        );
        let value = self.signed_request(
            HttpMethod::Post,
            "/linear-swap-api/v1/swap_cross_position_info",
            &[],
            &body,
        )?;
        let list = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing position data".to_string()))?;
        list.iter().map(parse_futures_position).collect()
    }

    /// Set the leverage for `symbol`
    /// (`/linear-swap-api/v1/swap_cross_switch_lever_rate`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let lever = leverage.max(1);
        self.leverage.set(lever);
        let body = format!(
            r#"{{"contract_code":"{}","lever_rate":{lever}}}"#,
            Self::contract_code(symbol)
        );
        self.signed_request(
            HttpMethod::Post,
            "/linear-swap-api/v1/swap_cross_switch_lever_rate",
            &[],
            &body,
        )?;
        Ok(())
    }

    /// Set the margin mode for `symbol`.
    ///
    /// This binding drives HTX's **cross-margin** swap family, so `Cross` is a
    /// no-op success. HTX does not expose a per-symbol switch to isolated within
    /// this family (it is an account/order-family choice), so `Isolated` returns
    /// an [`Error::Exchange`] documenting the limitation.
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] when `Isolated` is requested.
    pub fn set_margin_mode(&self, _symbol: &Symbol, mode: MarginMode) -> Result<()> {
        match mode {
            MarginMode::Cross => Ok(()),
            MarginMode::Isolated => Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "HTX linear-swap binding uses the cross-margin family; \
                          isolated is not switchable per-symbol"
                    .to_string(),
            }),
        }
    }

    /// Flatten the open position in `symbol` with a reduce-only market order.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] if there is no open position, or another
    /// [`Error`] if the request fails.
    pub fn close_position(&self, symbol: &Symbol) -> Result<Order> {
        let position = self
            .positions(Some(symbol))?
            .into_iter()
            .find(|p| &p.symbol == symbol)
            .ok_or_else(|| Error::NotFound(format!("no open position for {symbol}")))?;
        let request = match position.side {
            PositionSide::Long => OrderRequest::market_sell(symbol.clone(), position.quantity),
            PositionSide::Short => OrderRequest::market_buy(symbol.clone(), position.quantity),
        }
        .reduce_only();
        self.place_order(&request)
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
        let canonical = format!("{}\n{}\n{path}\n{encoded}", method.as_str(), self.host);
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

/// Map an HTX linear-swap order `status` integer to the unified status.
fn swap_status(code: i64) -> OrderStatus {
    match code {
        3 => OrderStatus::PartiallyFilled,
        6 => OrderStatus::Filled,
        4 | 7 => OrderStatus::Canceled,
        _ => OrderStatus::New,
    }
}

/// Parse an HTX linear-swap order object (`swap_cross_order_info` / openorders).
fn swap_order_from_value(symbol: Symbol, data: &serde_json::Value) -> Result<Order> {
    let id = data
        .get("order_id_str")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            data.get("order_id")
                .and_then(serde_json::Value::as_i64)
                .map(|n| n.to_string())
        })
        .unwrap_or_default();
    let direction = data
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let price_type = data
        .get("order_price_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let order_type = if price_type.contains("limit") || price_type == "post_only" {
        OrderType::Limit
    } else {
        OrderType::Market
    };
    let status = swap_status(
        data.get("status")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    );
    let filled = decimal_field(data, "trade_volume").unwrap_or(Decimal::ZERO);
    let avg = decimal_field(data, "trade_avg_price")
        .ok()
        .filter(|d| *d > Decimal::ZERO);
    let price = decimal_field(data, "price")
        .ok()
        .filter(|d| *d > Decimal::ZERO);
    Ok(Order {
        id,
        client_order_id: None,
        symbol,
        side: parse_side(direction)?,
        order_type,
        status,
        quantity: decimal_field(data, "volume").unwrap_or(Decimal::ZERO),
        filled_quantity: filled,
        price,
        average_price: avg,
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

/// Build an [`Order`] from an HTX v2 `orders#*` push payload.
fn ws_order_from_data(data: &serde_json::Value) -> Result<Order> {
    let wire = data
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing order symbol".to_string()))?;
    let order_type = data
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing order type".to_string()))?;
    let state = data
        .get("orderStatus")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing orderStatus".to_string()))?;
    // HTX encodes the order id as a JSON number.
    let id = match data.get("orderId") {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => return Err(Error::Deserialization("missing orderId".to_string())),
    };
    let client_id = data
        .get("clientOrderId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let opt_dec = |key: &str| {
        data.get(key)
            .and_then(|f| decimal_value(f).ok())
            .filter(|d| *d > Decimal::ZERO)
    };
    Ok(Order {
        id,
        client_order_id: (!client_id.is_empty()).then(|| client_id.to_string()),
        symbol: split_wire_symbol(wire),
        side: parse_side(order_type)?,
        order_type: parse_order_type(order_type)?,
        status: parse_state(state)?,
        quantity: decimal_field(data, "orderSize").unwrap_or(Decimal::ZERO),
        filled_quantity: data
            .get("execAmt")
            .and_then(|f| decimal_value(f).ok())
            .unwrap_or(Decimal::ZERO),
        price: opt_dec("orderPrice"),
        average_price: opt_dec("tradePrice"),
    })
}

/// Build a [`Balance`] from an HTX v2 `accounts.update` push payload. `available`
/// is the free balance; `balance` (when present) is the total, so the locked
/// amount is `balance - available`.
fn ws_balance_from_data(data: &serde_json::Value) -> Result<Balance> {
    let currency = data
        .get("currency")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing balance currency".to_string()))?;
    let available = data.get("available").and_then(|f| decimal_value(f).ok());
    let total = data.get("balance").and_then(|f| decimal_value(f).ok());
    let (free, locked) = match (available, total) {
        (Some(free), Some(total)) => (free, (total - free).max(Decimal::ZERO)),
        (Some(free), None) => (free, Decimal::ZERO),
        (None, Some(total)) => (total, Decimal::ZERO),
        (None, None) => (Decimal::ZERO, Decimal::ZERO),
    };
    Ok(Balance {
        asset: currency.to_string(),
        free,
        locked,
    })
}

fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    // HTX v2 private stream frames are `{"action":"push","ch":"orders#..."|"accounts.update#...","data":{...}}`.
    if value.get("action").and_then(serde_json::Value::as_str) == Some("push") {
        let ch = value
            .get("ch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let null = serde_json::Value::Null;
        let data = value.get("data").unwrap_or(&null);
        if ch.starts_with("orders#") {
            return Ok(vec![Event::OrderUpdate(ws_order_from_data(data)?)]);
        } else if ch.starts_with("accounts.update") {
            return Ok(vec![Event::BalanceUpdate(vec![ws_balance_from_data(
                data,
            )?])]);
        }
        return Ok(Vec::new());
    }
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

impl WsUserData for Htx {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Htx::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Htx::keepalive_user_data(self)
    }
}

impl WsExecution for Htx {
    /// HTX exposes no WebSocket order-entry API for spot — its WebSocket surface
    /// is subscription-only (market data plus the private v2 order/account push
    /// channels). Orders are placed over REST, so this returns a documented
    /// [`Error::Exchange`].
    fn place_order_ws(&mut self, _request: &OrderRequest) -> Result<Order> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "HTX has no WebSocket order-entry API; place orders over REST \
                      (POST /v1/order/orders/place)"
                .to_string(),
        })
    }

    fn cancel_order_ws(&mut self, _symbol: &Symbol, _order_id: &str) -> Result<()> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "HTX has no WebSocket order-entry API; cancel orders over REST \
                      (POST /v1/order/orders/{id}/submitcancel)"
                .to_string(),
        })
    }
}

impl Derivatives for Htx {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Htx::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Htx::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Htx::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Htx::close_position(self, symbol)
    }
}

impl Htx {
    /// Place several spot orders in one request (`/v1/order/batch-orders`). Each
    /// element carries an `err-code`, so each request's own [`Result`] is kept.
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails, or if called on a
    /// futures client (spot batch endpoint).
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "HTX batch-orders is a spot endpoint".to_string(),
            });
        }
        let account_id = self.account_id()?;
        let items: Vec<serde_json::Value> = requests
            .iter()
            .map(|r| {
                let order_kind = format!("{}-{}", side_str(r.side), order_type_str(r.order_type));
                let mut o = serde_json::json!({
                    "account-id": account_id,
                    "symbol": Self::wire_symbol(&r.symbol),
                    "type": order_kind,
                    "amount": format_decimal(r.quantity),
                });
                if let Some(price) = r.price {
                    o["price"] = serde_json::json!(format_decimal(price));
                }
                if let Some(id) = &r.client_order_id {
                    o["client-order-id"] = serde_json::json!(id.clone());
                }
                o
            })
            .collect();
        let body = serde_json::Value::Array(items).to_string();
        let value = self.signed_request(HttpMethod::Post, "/v1/order/batch-orders", &[], &body)?;
        let data = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing batch data".to_string()))?;
        Ok(requests
            .iter()
            .zip(data)
            .map(|(req, elem)| {
                let err_code = elem
                    .get("err-code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if !err_code.is_empty() {
                    return Err(Error::OrderRejected {
                        code: err_code.to_string(),
                        message: elem
                            .get("err-msg")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    });
                }
                let id = elem
                    .get("order-id")
                    .map(|v| {
                        v.as_u64()
                            .map_or_else(|| v.as_str().unwrap_or("").to_string(), |n| n.to_string())
                    })
                    .unwrap_or_default();
                Ok(Order {
                    id,
                    client_order_id: req.client_order_id.clone(),
                    symbol: req.symbol.clone(),
                    side: req.side,
                    order_type: req.order_type,
                    status: OrderStatus::New,
                    quantity: req.quantity,
                    filled_quantity: Decimal::ZERO,
                    price: req.price,
                    average_price: None,
                })
            })
            .collect())
    }

    /// Cancel several orders by id in one request (`/v1/order/orders/batchcancel`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, _symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        let ids: Vec<&str> = order_ids.iter().map(String::as_str).collect();
        let body = serde_json::json!({ "order-ids": ids }).to_string();
        self.signed_request(HttpMethod::Post, "/v1/order/orders/batchcancel", &[], &body)?;
        Ok(())
    }
}

impl AdvancedOrders for Htx {
    /// HTX has no in-place amend (orders are cancelled and re-placed), so this
    /// returns an [`Error::Exchange`].
    fn amend_order(
        &mut self,
        _symbol: &Symbol,
        _order_id: &str,
        _new_price: Option<Decimal>,
        _new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "HTX has no in-place amend; cancel and re-place the order".to_string(),
        })
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Htx::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Htx::cancel_batch(self, symbol, order_ids)
    }
    /// HTX has no OCO order-list, so this returns an [`Error::Exchange`].
    fn place_oco(&mut self, _request: &OcoRequest) -> Result<Vec<Order>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "HTX has no OCO order-list".to_string(),
        })
    }
}

/// Round a base quantity to a whole number of HTX contracts.
fn decimal_to_contracts(quantity: Decimal) -> Result<i64> {
    quantity
        .round()
        .to_i64()
        .filter(|c| *c > 0)
        .ok_or(Error::InvalidOrder(
            "futures volume rounds to zero contracts",
        ))
}

fn parse_futures_position(data: &serde_json::Value) -> Result<Position> {
    let direction = data
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let side = match direction {
        "buy" => PositionSide::Long,
        "sell" => PositionSide::Short,
        other => {
            return Err(Error::Deserialization(format!(
                "unknown position direction {other:?}"
            )))
        }
    };
    let contract = data
        .get("contract_code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    Ok(Position {
        symbol: symbol_from_contract(contract),
        side,
        quantity: decimal_field(data, "volume").unwrap_or(Decimal::ZERO),
        entry_price: decimal_field(data, "cost_open").unwrap_or(Decimal::ZERO),
        mark_price: decimal_field(data, "last_price").unwrap_or(Decimal::ZERO),
        leverage: decimal_field(data, "lever_rate").unwrap_or(Decimal::ZERO),
        unrealized_pnl: decimal_field(data, "profit_unreal").unwrap_or(Decimal::ZERO),
        // The cross-margin family always reports cross positions.
        margin_mode: MarginMode::Cross,
    })
}

/// Reconstruct a canonical [`Symbol`] from an HTX swap contract code
/// (`BTC-USDT` -> `BTC/USDT`).
fn symbol_from_contract(contract: &str) -> Symbol {
    match contract.split_once('-') {
        Some((base, quote)) => Symbol::new(base, quote),
        None => Symbol::new(contract, ""),
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

    fn futures_client() -> (Htx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        (
            Htx::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_futures_client(now_ms: i64) -> (Htx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let htx = Htx::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (htx, mock)
    }

    fn signed_ws_client(now_ms: i64) -> (Htx, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let htx = Htx::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (htx, ws)
    }

    #[test]
    fn subscribe_user_data_authenticates_and_streams_orders_and_accounts() {
        let (mut htx, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"action":"req","code":200,"ch":"auth","data":{}}"#.to_string(),
            )),
            Ok(Some(
                r#"{"action":"sub","code":200,"ch":"orders#*"}"#.to_string(),
            )),
            Ok(Some(
                r#"{"action":"push","ch":"orders#btcusdt","data":{"orderId":55,"symbol":"btcusdt",
                "clientOrderId":"my","type":"buy-limit","orderStatus":"filled","orderSize":"1",
                "execAmt":"1","orderPrice":"100","tradePrice":"100"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"action":"push","ch":"accounts.update#2","data":{"currency":"usdt",
                "balance":"950","available":"900"}}"#
                    .to_string(),
            )),
        ]);
        htx.subscribe_user_data().unwrap();
        assert_eq!(ws.connected_urls()[0], "wss://api.huobi.pro/ws/v2");
        assert!(ws.sent()[0].contains(r#""ch":"auth""#));
        assert!(ws.sent()[0].contains(r#""accessKey":"APIKEY""#));
        assert!(ws.sent()[0].contains(r#""signature""#));
        assert!(ws.sent()[1].contains(r#""ch":"orders#*""#));
        assert!(ws.sent()[2].contains(r#""ch":"accounts.update#2""#));

        let events = htx.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "55");
        assert_eq!(order.client_order_id.as_deref(), Some("my"));
        assert_eq!(order.symbol, symbol());
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.filled_quantity, dec!(1));
        assert_eq!(order.average_price, Some(dec!(100)));
        let Event::BalanceUpdate(balances) = &events[1] else {
            panic!("second event must be a balance update");
        };
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].asset, "usdt");
        assert_eq!(balances[0].free, dec!(900));
        assert_eq!(balances[0].locked, dec!(50));
    }

    #[test]
    fn subscribe_user_data_requires_credentials() {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut htx = Htx::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            htx.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut htx, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![]);
        htx.subscribe_user_data().unwrap();
        htx.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == r#"{"action":"ping"}"#));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut htx, ws) = signed_ws_client(1_700_000_000_000);
        htx.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_a_fresh_auth() {
        let (mut htx, ws) = signed_ws_client(1_700_000_000_000);
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        htx.subscribe_user_data().unwrap();

        let events = htx.poll_events();
        assert!(events.contains(&Event::Disconnected));
        assert!(events.contains(&Event::Reconnected));
        // Two private connections (initial + reconnect), each re-signing the v2 auth.
        let auth_frames = ws
            .sent()
            .into_iter()
            .filter(|f| f.contains(r#""ch":"auth""#))
            .count();
        assert_eq!(auth_frames, 2);
        assert_eq!(ws.connected_urls().len(), 2);
        assert_eq!(ws.connected_urls()[1], "wss://api.huobi.pro/ws/v2");
    }

    #[test]
    fn ws_execution_is_a_documented_gap() {
        // HTX has no WebSocket order-entry API; the trait methods return a
        // documented error rather than faking a round trip.
        let (mut htx, _mock) = client();
        assert!(matches!(
            htx.place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::Exchange { .. }
        ));
        assert!(matches!(
            htx.cancel_order_ws(&symbol(), "1").unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn place_batch_reads_per_order_err_code() {
        let (htx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"id":42,"type":"spot","state":"working"}]}"#,
        );
        mock.push_json(
            200,
            r#"{"status":"ok","data":[
            {"order-id":1,"client-order-id":"","err-code":"","err-msg":""},
            {"order-id":0,"client-order-id":"","err-code":"account-frozen-balance-insufficient-error","err-msg":"no"}]}"#,
        );
        let results = htx
            .place_batch(&[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(101)),
            ])
            .unwrap();
        assert_eq!(results[0].as_ref().unwrap().id, "1");
        assert!(matches!(
            results[1].as_ref().unwrap_err(),
            Error::OrderRejected { .. }
        ));
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/v1/order/batch-orders"));
    }

    #[test]
    fn cancel_batch_is_one_call() {
        let (htx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"success":["1"],"failed":[]}}"#,
        );
        htx.cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/v1/order/orders/batchcancel"));
        assert!(reqs[0]
            .body
            .as_ref()
            .unwrap()
            .contains(r#""order-ids":["1","2"]"#));
    }

    #[test]
    fn amend_and_oco_are_unsupported() {
        let (mut htx, _mock) = signed_client(0);
        assert!(matches!(
            AdvancedOrders::amend_order(&mut htx, &symbol(), "1", Some(dec!(1)), None).unwrap_err(),
            Error::Exchange { .. }
        ));
        assert!(matches!(
            AdvancedOrders::place_oco(
                &mut htx,
                &OcoRequest::new(symbol(), OrderSide::Sell, dec!(1), dec!(110), dec!(95))
            )
            .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn futures_ticker_targets_swap_host_and_contract_code() {
        let (htx, mock) = futures_client();
        mock.push_json(
            200,
            r#"{"status":"ok","tick":{"close":20000.5,"bid":[19999.0,1.0],"ask":[20001.0,2.0],"vol":1234.0}}"#,
        );
        let ticker = htx.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(
            mock.recorded_requests()[0].url,
            "https://api.hbdm.com/linear-swap-ex/market/detail/merged?contract_code=BTC-USDT"
        );
    }

    #[test]
    fn futures_market_order_uses_cross_swap_family() {
        let (htx, mock) = signed_futures_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"order_id":123,"order_id_str":"123"}}"#,
        );
        let order = htx
            .place_order(&OrderRequest::market_buy(symbol(), dec!(2)))
            .unwrap();
        assert_eq!(order.id, "123");
        assert_eq!(order.status, OrderStatus::New);
        let req = &mock.recorded_requests()[0];
        assert!(req
            .url
            .contains("api.hbdm.com/linear-swap-api/v1/swap_cross_order"));
        let body = req.body.as_ref().unwrap();
        assert!(body.contains(r#""contract_code":"BTC-USDT""#));
        assert!(body.contains(r#""direction":"buy""#));
        assert!(body.contains(r#""offset":"open""#));
        assert!(body.contains(r#""order_price_type":"optimal_5""#));
        assert!(body.contains(r#""volume":2"#));
        assert!(body.contains(r#""lever_rate":1"#));
    }

    #[test]
    fn futures_query_order_uses_swap_order_info() {
        let (htx, mock) = signed_futures_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"order_id":88,"order_id_str":"88","direction":"buy",
            "offset":"open","order_price_type":"limit","status":6,"volume":3,"trade_volume":3,
            "price":100,"trade_avg_price":100}]}"#,
        );
        let order = htx.query_order(&symbol(), "88").unwrap();
        assert_eq!(order.id, "88");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.quantity, dec!(3));
        let req = &mock.recorded_requests()[0];
        assert!(req
            .url
            .contains("/linear-swap-api/v1/swap_cross_order_info"));
        assert!(req
            .body
            .as_ref()
            .unwrap()
            .contains(r#""contract_code":"BTC-USDT""#));
    }

    #[test]
    fn futures_cancel_and_open_orders_use_swap_endpoints() {
        let (htx, mock) = signed_futures_client(0);
        mock.push_json(200, r#"{"status":"ok","data":{"successes":"88"}}"#);
        htx.cancel_order(&symbol(), "88").unwrap();
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"orders":[{"order_id":90,"order_id_str":"90",
            "direction":"sell","offset":"open","order_price_type":"limit","status":3,
            "volume":5,"trade_volume":2,"price":21000,"trade_avg_price":21000}]}}"#,
        );
        let orders = htx.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);
        assert_eq!(orders[0].status, OrderStatus::PartiallyFilled);
        let reqs = mock.recorded_requests();
        assert!(reqs[0]
            .url
            .contains("/linear-swap-api/v1/swap_cross_cancel"));
        assert!(reqs[1]
            .url
            .contains("/linear-swap-api/v1/swap_cross_openorders"));
    }

    #[test]
    fn derivatives_positions_parse_cross() {
        let (mut htx, mock) = signed_futures_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"contract_code":"BTC-USDT","direction":"buy","volume":3,
            "cost_open":20000,"last_price":20100,"lever_rate":10,"profit_unreal":30}]}"#,
        );
        let positions = Derivatives::positions(&mut htx, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(3));
        assert_eq!(positions[0].entry_price, dec!(20000));
        assert_eq!(positions[0].mark_price, dec!(20100));
        assert_eq!(positions[0].leverage, dec!(10));
        assert_eq!(positions[0].margin_mode, MarginMode::Cross);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/linear-swap-api/v1/swap_cross_position_info"));
    }

    #[test]
    fn set_leverage_persists_and_flows_into_orders() {
        let (htx, mock) = signed_futures_client(0);
        mock.push_json(200, r#"{"status":"ok"}"#);
        htx.set_leverage(&symbol(), 5).unwrap();
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/linear-swap-api/v1/swap_cross_switch_lever_rate"));

        mock.push_json(
            200,
            r#"{"status":"ok","data":{"order_id":1,"order_id_str":"1"}}"#,
        );
        htx.place_order(&OrderRequest::market_buy(symbol(), dec!(1)))
            .unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].body.as_ref().unwrap().contains(r#""lever_rate":5"#));
    }

    #[test]
    fn set_margin_mode_isolated_is_unsupported() {
        let (htx, _mock) = signed_futures_client(0);
        assert!(htx.set_margin_mode(&symbol(), MarginMode::Cross).is_ok());
        assert!(matches!(
            htx.set_margin_mode(&symbol(), MarginMode::Isolated)
                .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn close_position_is_reduce_only_opposite() {
        let (mut htx, mock) = signed_futures_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"contract_code":"BTC-USDT","direction":"buy","volume":3,
            "cost_open":20000,"last_price":20100,"lever_rate":10,"profit_unreal":30}]}"#,
        );
        mock.push_json(
            200,
            r#"{"status":"ok","data":{"order_id":9,"order_id_str":"9"}}"#,
        );
        Derivatives::close_position(&mut htx, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[1].body.as_ref().unwrap();
        assert!(body.contains(r#""direction":"sell""#));
        assert!(body.contains(r#""offset":"close""#));
    }

    #[test]
    fn futures_balances_split_available_and_frozen() {
        let (htx, mock) = signed_futures_client(0);
        mock.push_json(
            200,
            r#"{"status":"ok","data":[{"margin_asset":"USDT","margin_available":800,"margin_frozen":200}]}"#,
        );
        let bals = htx.balances().unwrap();
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].free, dec!(800));
        assert_eq!(bals[0].locked, dec!(200));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/linear-swap-api/v1/swap_cross_account_info"));
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
