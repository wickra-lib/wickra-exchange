//! Gate.io (v4 API) — the sixth exchange.
//!
//! Gate signs with `SIGN = hex(HMAC-SHA512(secret, sig_string))`, where
//! `sig_string = METHOD\npath\nquery\nhex(SHA512(body))\ntimestamp` (unix
//! seconds), carried in `KEY`/`SIGN`/`Timestamp` headers. Symbols use an
//! underscore (`BTC_USDT`) and there is no response envelope — success is the raw
//! JSON, errors come back as an HTTP error status with `{label, message}`.
//!
//! When built with a futures [`MarketType`](crate::MarketType), market data,
//! `place_order`, `balances` and the [`Derivatives`] trait route to the
//! USDT-margined perpetual endpoints (`/api/v4/futures/usdt/*`), where orders
//! carry a signed integer contract `size`. `query_order`/`cancel_order`/
//! `open_orders` route to the futures order endpoints too, parsing the futures
//! order object (numeric id, signed `size`, `finish_as`) via
//! `futures_order_from_raw`.
//!
//! [`AdvancedOrders`]: STP via `stp_act` on order create, native in-place amend
//! (`PATCH /api/v4/spot/orders/{id}`) and native spot batch place/cancel
//! (`/api/v4/spot/batch_orders`, `/cancel_batch_orders`, per-order `succeeded`
//! flag). Gate has no OCO order-list, so `place_oco` is a documented gap.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType, SelfTradePrevention};
use crate::positions::{Position, PositionSide};
use crate::signing::{hmac_sha512_hex, sha512_hex};
use crate::symbol::Symbol;
use crate::traits::{
    AdvancedOrders, Derivatives, Exchange, Execution, MarketData, WsExecution, WsUserData,
};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
use crate::types::{
    Balance, OcoRequest, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker,
};
use rust_decimal::prelude::ToPrimitive;
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
    market_type: MarketType,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    /// Leverage applied when switching to isolated margin. Gate couples the
    /// margin mode with its leverage endpoint (`leverage=0` = cross), so
    /// [`set_leverage`](Self::set_leverage) records the value here.
    leverage: u32,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
    /// Set once the private stream is subscribed, so [`poll_events`](Self::poll_events)
    /// re-subscribes it after a drop.
    user_data_active: bool,
    /// A dedicated logged-in connection to the WebSocket order API, opened lazily
    /// on the first [`place_order_ws`](Self::place_order_ws) /
    /// [`cancel_order_ws`](Self::cancel_order_ws) call.
    ws_api_connection: Option<Box<dyn WsConnection>>,
}

impl Gate {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://api.gateio.ws".to_string(),
            market_type: options.market_type,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            leverage: 1,
            private_connection: None,
            user_data_active: false,
            ws_api_connection: None,
        }
    }

    /// Whether this client targets Gate USDT-margined perpetual futures
    /// (`/api/v4/futures/usdt/*`) rather than spot.
    fn is_futures(&self) -> bool {
        self.market_type.is_derivatives()
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
        if self.is_futures() {
            return self.futures_ticker(symbol);
        }
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

    /// A futures ticker. Gate's perpetual ticker carries `last`/`volume_24h_base`
    /// but no best bid/ask, so the top of the futures order book supplies those.
    fn futures_ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let contract = Self::wire_symbol(symbol);
        let value = self.get(
            "/api/v4/futures/usdt/tickers",
            &format!("contract={contract}"),
        )?;
        let list: Vec<RawFuturesTicker> = parse_json(value)?;
        let entry = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        let book = self.order_book(symbol, 1)?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last)?,
            bid: book.bids.first().map_or(Decimal::ZERO, |l| l.price),
            ask: book.asks.first().map_or(Decimal::ZERO, |l| l.price),
            volume: parse_decimal(&entry.volume_24h_base)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Gate returns
    /// oldest-first, already chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        if self.is_futures() {
            let query = format!(
                "contract={}&interval={}&limit={limit}",
                Self::wire_symbol(symbol),
                map_interval(interval),
            );
            let value = self.get("/api/v4/futures/usdt/candlesticks", &query)?;
            let rows: Vec<RawFuturesCandle> = parse_json(value)?;
            return rows.iter().map(parse_futures_candle).collect();
        }
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
        if self.is_futures() {
            let query = format!("contract={}&limit={depth}", Self::wire_symbol(symbol));
            let value = self.get("/api/v4/futures/usdt/order_book", &query)?;
            let raw: RawFuturesDepth = parse_json(value)?;
            return Ok(OrderBookSnapshot {
                symbol: symbol.clone(),
                last_update_id: raw.id,
                bids: parse_futures_levels(&raw.bids)?,
                asks: parse_futures_levels(&raw.asks)?,
            });
        }
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
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // Drain the private user-data stream (spot.orders/spot.balances), if open.
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // A dropped private stream is re-subscribed with fresh signed frames (the
        // per-channel signature is time-bound, so a stale replay would be rejected).
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

    /// Open the private user-data stream (`wss://api.gateio.ws/ws/v4/`). Sends
    /// signed `subscribe` frames for the `spot.orders` and `spot.balances`
    /// channels (each carries an `auth` object whose `SIGN` is HMAC-SHA512 over
    /// `channel=<channel>&event=subscribe&time=<time>`). Afterwards
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
        let time = (self.now_ms)() / 1000;
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://api.gateio.ws/ws/v4/")?;
        // `spot.orders` takes `!all` to cover every pair; `spot.balances` takes no payload.
        for (channel, payload) in [("spot.orders", r#"["!all"]"#), ("spot.balances", "[]")] {
            let sign_string = format!("channel={channel}&event=subscribe&time={time}");
            let sign = hmac_sha512_hex(creds.api_secret.as_bytes(), sign_string.as_bytes());
            let message = format!(
                r#"{{"time":{time},"channel":"{channel}","event":"subscribe","payload":{payload},"auth":{{"method":"api_key","KEY":"{}","SIGN":"{sign}"}}}}"#,
                creds.api_key
            );
            connection.send(&message)?;
        }
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send an application-level heartbeat (the `ping` text frame) on the private
    /// stream so it is not dropped for inactivity. A no-op before
    /// [`subscribe_user_data`](Self::subscribe_user_data).
    ///
    /// # Errors
    /// Returns an [`Error`] if the ping cannot be sent.
    pub fn keepalive_user_data(&mut self) -> Result<()> {
        if let Some(connection) = self.private_connection.as_mut() {
            connection.send("ping")?;
        }
        Ok(())
    }

    /// Place a spot order over the Gate WebSocket order API (`spot.order_place`).
    /// Logs into a dedicated connection on first use (`spot.login`, signature =
    /// HMAC-SHA512 over `api\nspot.login\n\n<timestamp>`), then exchanges the same
    /// order args as the REST path.
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] on the **futures** client (this binding's WS
    /// order API covers spot), [`Error::NotConnected`] without a WebSocket
    /// transport, or another [`Error`] if the order is invalid or rejected.
    pub fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Gate's WebSocket order API is wired for spot here; place \
                          futures orders over REST (/api/v4/futures/usdt/orders)"
                    .to_string(),
            });
        }
        let mut req_param = serde_json::json!({
            "currency_pair": Self::wire_symbol(&request.symbol),
            "type": order_type_str(request.order_type),
            "account": "spot",
            "side": side_str(request.side),
            "amount": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            req_param["price"] = serde_json::json!(format_decimal(price));
        }
        if request.post_only {
            req_param["time_in_force"] = serde_json::json!("poc");
        }
        if let Some(id) = &request.client_order_id {
            let text = if id.starts_with("t-") {
                id.clone()
            } else {
                format!("t-{id}")
            };
            req_param["text"] = serde_json::json!(text);
        }
        let result = self.ws_api_request("spot.order_place", &req_param)?;
        let raw: RawOrder =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        order_from_raw(request.symbol.clone(), &raw)
    }

    /// Cancel a spot order over the Gate WebSocket order API (`spot.order_cancel`).
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] on the futures client, [`Error::NotConnected`]
    /// without a WebSocket transport, or another [`Error`] if the request fails.
    pub fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Gate's WebSocket order API is wired for spot here; cancel \
                          futures orders over REST"
                    .to_string(),
            });
        }
        let req_param = serde_json::json!({
            "order_id": order_id,
            "currency_pair": Self::wire_symbol(symbol),
        });
        self.ws_api_request("spot.order_cancel", &req_param)?;
        Ok(())
    }

    /// Open and log into the WebSocket order connection if needed. The `spot.login`
    /// request signs `api\nspot.login\n\n<timestamp>`; later requests on the same
    /// connection inherit the session, so they are unsigned.
    fn ensure_ws_api(&mut self) -> Result<()> {
        if self.ws_api_connection.is_some() {
            return Ok(());
        }
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "WebSocket order API requires credentials",
        ))?;
        let timestamp = (self.now_ms)() / 1000;
        let signature = hmac_sha512_hex(
            creds.api_secret.as_bytes(),
            format!("api\nspot.login\n\n{timestamp}").as_bytes(),
        );
        let req_id = format!("wkex-login-{}", (self.now_ms)());
        let login = format!(
            r#"{{"time":{timestamp},"channel":"spot.login","event":"api","payload":{{"req_id":"{req_id}","api_key":"{}","signature":"{signature}","timestamp":"{timestamp}"}}}}"#,
            creds.api_key
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://api.gateio.ws/ws/v4/")?;
        connection.send(&login)?;
        connection.recv()?; // consume the login acknowledgement
        self.ws_api_connection = Some(connection);
        Ok(())
    }

    /// Send an order-API request frame and return its `data.result`, mapping a
    /// non-`"200"` header status onto the error taxonomy.
    fn ws_api_request(
        &mut self,
        channel: &str,
        req_param: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.ensure_ws_api()?;
        let time = (self.now_ms)() / 1000;
        let req_id = format!("wkex-{}", (self.now_ms)());
        let frame = serde_json::json!({
            "time": time,
            "channel": channel,
            "event": "api",
            "payload": { "req_id": req_id, "req_param": req_param },
        })
        .to_string();
        let connection = self
            .ws_api_connection
            .as_mut()
            .expect("ws order connection just ensured");
        connection.send(&frame)?;
        let Some(response) = connection.recv()? else {
            return Err(Error::NotConnected);
        };
        let value: serde_json::Value =
            serde_json::from_str(&response).map_err(|e| Error::Deserialization(e.to_string()))?;
        let status = value
            .get("header")
            .and_then(|header| header.get("status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if status == "200" {
            Ok(value
                .get("data")
                .and_then(|data| data.get("result"))
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        } else {
            let errs = value.get("data").and_then(|data| data.get("errs"));
            let label = errs
                .and_then(|e| e.get("label"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("ws");
            let message = errs
                .and_then(|e| e.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("order rejected")
                .to_string();
            Err(Error::OrderRejected {
                code: label.to_string(),
                message,
            })
        }
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
        if let Some(act) = stp_act_str(request.stp) {
            body["stp_act"] = serde_json::json!(act);
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
        if self.is_futures() {
            let path = format!("/api/v4/futures/usdt/orders/{order_id}");
            self.signed_request(HttpMethod::Delete, &path, "", "")?;
            return Ok(());
        }
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
        if self.is_futures() {
            let path = format!("/api/v4/futures/usdt/orders/{order_id}");
            let value = self.signed_request(HttpMethod::Get, &path, "", "")?;
            let raw: RawFuturesOrder = parse_json(value)?;
            return futures_order_from_raw(symbol.clone(), &raw);
        }
        let path = format!("/api/v4/spot/orders/{order_id}");
        let query = format!("currency_pair={}", Self::wire_symbol(symbol));
        let value = self.signed_request(HttpMethod::Get, &path, &query, "")?;
        let raw: RawOrder = parse_json(value)?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// Open orders for one `symbol` (Gate requires a currency pair / contract here).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing, no symbol is given, or the
    /// request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let sym = symbol.ok_or(Error::InvalidOrder("Gate open_orders requires a symbol"))?;
        if self.is_futures() {
            let query = format!("contract={}&status=open", Self::wire_symbol(sym));
            let value =
                self.signed_request(HttpMethod::Get, "/api/v4/futures/usdt/orders", &query, "")?;
            let list: Vec<RawFuturesOrder> = parse_json(value)?;
            return list
                .iter()
                .map(|raw| futures_order_from_raw(sym.clone(), raw))
                .collect();
        }
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
        if self.is_futures() {
            let value =
                self.signed_request(HttpMethod::Get, "/api/v4/futures/usdt/accounts", "", "")?;
            let acct: RawFuturesAccount = parse_json(value)?;
            let total = dec_or_zero(&acct.total);
            let available = dec_or_zero(&acct.available);
            return Ok(vec![Balance {
                asset: acct.currency,
                free: available,
                locked: (total - available).max(Decimal::ZERO),
            }]);
        }
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

    /// Place a Gate USDT-margined futures order. Futures orders carry a **signed
    /// integer `size`** (contracts; positive = long/buy, negative = short/sell)
    /// and use `price="0"` with `tif="ioc"` for market orders.
    fn place_futures_order(&self, request: &OrderRequest) -> Result<Order> {
        let contract = Self::wire_symbol(&request.symbol);
        let magnitude = decimal_to_contracts(request.quantity)?;
        let size = match request.side {
            OrderSide::Buy => magnitude,
            OrderSide::Sell => -magnitude,
        };
        let mut body = serde_json::json!({
            "contract": contract,
            "size": size,
            "reduce_only": request.reduce_only,
        });
        match request.order_type {
            OrderType::Market | OrderType::StopMarket => {
                body["price"] = serde_json::json!("0");
                body["tif"] = serde_json::json!("ioc");
            }
            OrderType::Limit | OrderType::StopLimit => {
                let price = request
                    .price
                    .ok_or(Error::InvalidOrder("limit order requires a price"))?;
                body["price"] = serde_json::json!(format_decimal(price));
                if request.post_only {
                    body["tif"] = serde_json::json!("poc");
                }
            }
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
            "/api/v4/futures/usdt/orders",
            "",
            &body.to_string(),
        )?;
        let raw: RawFuturesOrder = parse_json(value)?;
        futures_order_from_raw(request.symbol.clone(), &raw)
    }

    /// Open positions on the USDT-margined futures account
    /// (`/api/v4/futures/usdt/positions`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let value =
            self.signed_request(HttpMethod::Get, "/api/v4/futures/usdt/positions", "", "")?;
        let list: Vec<RawFuturesPosition> = parse_json(value)?;
        let wanted = symbol.map(Self::wire_symbol);
        Ok(list
            .iter()
            .filter(|p| p.size != 0)
            .filter(|p| wanted.as_ref().is_none_or(|w| &p.contract == w))
            .map(parse_futures_position)
            .collect())
    }

    /// Set the leverage for `symbol` (isolated margin; `leverage=0` = cross).
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        self.leverage = leverage.max(1);
        self.apply_leverage(symbol, leverage)
    }

    /// Switch the margin mode for `symbol`. Gate couples this with the leverage
    /// endpoint: cross is `leverage=0`, isolated re-applies the recorded leverage.
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let leverage = match mode {
            MarginMode::Cross => 0,
            MarginMode::Isolated => self.leverage.max(1),
        };
        self.apply_leverage(symbol, leverage)
    }

    fn apply_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let path = format!(
            "/api/v4/futures/usdt/positions/{}/leverage",
            Self::wire_symbol(symbol)
        );
        let query = format!("leverage={leverage}");
        self.signed_request(HttpMethod::Post, &path, &query, "")?;
        Ok(())
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

/// The Gate `stp_act` value for a self-trade-prevention policy, or `None` to omit.
/// Gate cancels the oldest (`co`), newest (`cn`) or both (`cb`) order.
fn stp_act_str(stp: SelfTradePrevention) -> Option<&'static str> {
    match stp {
        SelfTradePrevention::None => None,
        SelfTradePrevention::ExpireMaker => Some("co"),
        SelfTradePrevention::ExpireTaker => Some("cn"),
        SelfTradePrevention::ExpireBoth => Some("cb"),
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

/// Split a Gate wire pair (`BTC_USDT`) into a canonical [`Symbol`].
fn symbol_from_wire(wire: &str) -> Symbol {
    match wire.split_once('_') {
        Some((base, quote)) if !base.is_empty() && !quote.is_empty() => Symbol::new(base, quote),
        _ => Symbol::new(wire, ""),
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
    // REST reports `filled_amount` directly; the `spot.orders` stream reports the
    // remaining `left`, so the filled quantity is `amount - left`.
    let filled = if raw.filled_amount.is_empty() && !raw.left.is_empty() {
        (dec_or_zero(&raw.amount) - dec_or_zero(&raw.left)).max(Decimal::ZERO)
    } else {
        dec_or_zero(&raw.filled_amount)
    };
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

fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("update") {
        return Ok(Vec::new()); // subscribe ack / other events
    }
    let Some(channel) = value.get("channel").and_then(serde_json::Value::as_str) else {
        return Ok(Vec::new());
    };
    let null = serde_json::Value::Null;
    let result = value.get("result").unwrap_or(&null);

    match channel {
        "spot.trades" => Ok(vec![Event::Trade(TradePrint {
            symbol: resolve(field_str(result, "currency_pair")?),
            price: parse_decimal(field_str(result, "price")?)?,
            quantity: parse_decimal(field_str(result, "amount")?)?,
            aggressor: parse_side(field_str(result, "side")?)?,
            timestamp: opt_i64(result, "create_time_ms"),
        })]),
        "spot.tickers" => Ok(vec![Event::Ticker(Ticker {
            symbol: resolve(field_str(result, "currency_pair")?),
            last: parse_decimal(field_str(result, "last")?)?,
            bid: dec_or_zero(opt_str(result, "highest_bid")),
            ask: dec_or_zero(opt_str(result, "lowest_ask")),
            volume: dec_or_zero(opt_str(result, "base_volume")),
        })]),
        "spot.order_book_update" => {
            let update_id = opt_u64(result, "u");
            Ok(vec![Event::BookDelta(BookDelta {
                symbol: resolve(opt_str(result, "s")),
                first_update_id: update_id,
                final_update_id: update_id,
                bids: parse_ws_levels(result.get("b"))?,
                asks: parse_ws_levels(result.get("a"))?,
            })])
        }
        // Private order channel: `result` is an array of order objects, each
        // carrying its own `currency_pair`.
        "spot.orders" => result
            .as_array()
            .ok_or_else(|| Error::Deserialization("spot.orders result not an array".to_string()))?
            .iter()
            .map(|order| {
                // The private stream carries no public subscription, so the wire
                // `currency_pair` is split directly.
                let symbol = symbol_from_wire(field_str(order, "currency_pair")?);
                let raw: RawOrder = serde_json::from_value(order.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                Ok(Event::OrderUpdate(order_from_raw(symbol, &raw)?))
            })
            .collect(),
        // Private balance channel: `result` is an array of per-currency balances,
        // emitted together as one balance-update snapshot.
        "spot.balances" => {
            let balances = result
                .as_array()
                .ok_or_else(|| {
                    Error::Deserialization("spot.balances result not an array".to_string())
                })?
                .iter()
                .map(|entry| {
                    let total = dec_or_zero(opt_str(entry, "total"));
                    let available = dec_or_zero(opt_str(entry, "available"));
                    Ok(Balance {
                        asset: field_str(entry, "currency")?.to_string(),
                        free: available,
                        locked: (total - available).max(Decimal::ZERO),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(vec![Event::BalanceUpdate(balances)])
        }
        _ => Ok(Vec::new()),
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
    // The private `spot.orders` stream reports the remaining size as `left`
    // rather than the filled amount; the filled quantity is derived from it.
    #[serde(default)]
    left: String,
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

#[derive(Deserialize)]
struct RawFuturesTicker {
    last: String,
    #[serde(rename = "volume_24h_base", default)]
    volume_24h_base: String,
}

#[derive(Deserialize)]
struct RawFuturesCandle {
    t: i64,
    o: String,
    h: String,
    l: String,
    c: String,
    #[serde(default)]
    v: f64,
}

#[derive(Deserialize)]
struct RawFuturesLevel {
    p: String,
    s: i64,
}

#[derive(Deserialize)]
struct RawFuturesDepth {
    #[serde(default)]
    id: u64,
    bids: Vec<RawFuturesLevel>,
    asks: Vec<RawFuturesLevel>,
}

#[derive(Deserialize)]
struct RawFuturesOrder {
    id: i64,
    size: i64,
    #[serde(default)]
    left: i64,
    #[serde(default)]
    price: String,
    #[serde(rename = "fill_price", default)]
    fill_price: String,
    status: String,
    #[serde(rename = "finish_as", default)]
    finish_as: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct RawFuturesAccount {
    #[serde(default = "usdt_currency")]
    currency: String,
    #[serde(default)]
    total: String,
    #[serde(default)]
    available: String,
}

fn usdt_currency() -> String {
    "USDT".to_string()
}

#[derive(Deserialize)]
struct RawFuturesPosition {
    contract: String,
    size: i64,
    #[serde(default)]
    leverage: String,
    #[serde(rename = "cross_leverage_limit", default)]
    cross_leverage_limit: String,
    #[serde(rename = "entry_price", default)]
    entry_price: String,
    #[serde(rename = "mark_price", default)]
    mark_price: String,
    #[serde(rename = "unrealised_pnl", default)]
    unrealised_pnl: String,
}

/// Round a base quantity to a whole number of Gate futures contracts.
fn decimal_to_contracts(quantity: Decimal) -> Result<i64> {
    let contracts = quantity
        .round()
        .to_i64()
        .filter(|c| *c != 0)
        .ok_or(Error::InvalidOrder("futures size rounds to zero contracts"))?;
    Ok(contracts.abs())
}

fn parse_futures_candle(row: &RawFuturesCandle) -> Result<Candle> {
    let f = |s: &str| -> Result<f64> {
        s.parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("candle field not a number: {e}")))
    };
    Candle::new(f(&row.o)?, f(&row.h)?, f(&row.l)?, f(&row.c)?, row.v, row.t)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn parse_futures_levels(levels: &[RawFuturesLevel]) -> Result<Vec<BookLevel>> {
    levels
        .iter()
        .map(|level| {
            Ok(BookLevel {
                price: parse_decimal(&level.p)?,
                quantity: Decimal::from(level.s.abs()),
            })
        })
        .collect()
}

fn futures_order_from_raw(symbol: Symbol, raw: &RawFuturesOrder) -> Result<Order> {
    let side = if raw.size >= 0 {
        OrderSide::Buy
    } else {
        OrderSide::Sell
    };
    let quantity = Decimal::from(raw.size.abs());
    let filled = Decimal::from(raw.size.abs() - raw.left.abs());
    let order_type = if raw.price == "0" || raw.price.is_empty() {
        OrderType::Market
    } else {
        OrderType::Limit
    };
    let status = match raw.status.as_str() {
        "open" => {
            if filled > Decimal::ZERO {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }
        }
        "finished" => {
            if raw.finish_as == "filled" {
                OrderStatus::Filled
            } else {
                OrderStatus::Canceled
            }
        }
        other => return Err(Error::Deserialization(format!("unknown status {other:?}"))),
    };
    Ok(Order {
        id: raw.id.to_string(),
        client_order_id: (!raw.text.is_empty()).then(|| raw.text.clone()),
        symbol,
        side,
        order_type,
        status,
        quantity,
        filled_quantity: filled,
        price: nonzero_decimal(&raw.price),
        average_price: nonzero_decimal(&raw.fill_price),
    })
}

fn parse_futures_position(raw: &RawFuturesPosition) -> Position {
    let side = if raw.size > 0 {
        PositionSide::Long
    } else {
        PositionSide::Short
    };
    // Gate reports `leverage == "0"` for cross positions and carries the effective
    // cap in `cross_leverage_limit`.
    let is_cross = raw.leverage.is_empty() || raw.leverage == "0";
    let leverage = if is_cross {
        dec_or_zero(&raw.cross_leverage_limit)
    } else {
        dec_or_zero(&raw.leverage)
    };
    Position {
        symbol: symbol_from_contract(&raw.contract),
        side,
        quantity: Decimal::from(raw.size.abs()),
        entry_price: dec_or_zero(&raw.entry_price),
        mark_price: dec_or_zero(&raw.mark_price),
        leverage,
        unrealized_pnl: dec_or_zero(&raw.unrealised_pnl),
        margin_mode: if is_cross {
            MarginMode::Cross
        } else {
            MarginMode::Isolated
        },
    }
}

/// Reconstruct a canonical [`Symbol`] from a Gate contract (`BTC_USDT`).
fn symbol_from_contract(contract: &str) -> Symbol {
    match contract.split_once('_') {
        Some((base, quote)) => Symbol::new(base, quote),
        None => Symbol::new(contract, ""),
    }
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

impl WsUserData for Gate {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Gate::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Gate::keepalive_user_data(self)
    }
}

impl WsExecution for Gate {
    fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        Gate::place_order_ws(self, request)
    }
    fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Gate::cancel_order_ws(self, symbol, order_id)
    }
}

impl Derivatives for Gate {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Gate::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Gate::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Gate::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Gate::close_position(self, symbol)
    }
}

impl Gate {
    /// Amend a resting spot order's price and/or amount in place
    /// (`PATCH /api/v4/spot/orders/{id}`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is unknown or the amend is rejected.
    pub fn amend_order(
        &self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        let mut body = serde_json::json!({});
        if let Some(q) = new_quantity {
            body["amount"] = serde_json::json!(format_decimal(q));
        }
        if let Some(p) = new_price {
            body["price"] = serde_json::json!(format_decimal(p));
        }
        let path = format!("/api/v4/spot/orders/{order_id}");
        let query = format!("currency_pair={}", Self::wire_symbol(symbol));
        let value = self.signed_request(HttpMethod::Patch, &path, &query, &body.to_string())?;
        let raw: RawOrder = parse_json(value)?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// Place several spot orders in one request (`/api/v4/spot/batch_orders`).
    /// Each element carries a `succeeded` flag, so each request's own [`Result`]
    /// is preserved.
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails.
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        let items: Vec<serde_json::Value> = requests
            .iter()
            .map(|r| {
                let mut o = serde_json::json!({
                    "currency_pair": Self::wire_symbol(&r.symbol),
                    "side": side_str(r.side),
                    "type": order_type_str(r.order_type),
                    "amount": format_decimal(r.quantity),
                });
                if let Some(price) = r.price {
                    o["price"] = serde_json::json!(format_decimal(price));
                }
                if let Some(id) = &r.client_order_id {
                    let text = if id.starts_with("t-") {
                        id.clone()
                    } else {
                        format!("t-{id}")
                    };
                    o["text"] = serde_json::json!(text);
                }
                o
            })
            .collect();
        let body = serde_json::Value::Array(items).to_string();
        let value =
            self.signed_request(HttpMethod::Post, "/api/v4/spot/batch_orders", "", &body)?;
        let elements: Vec<serde_json::Value> = parse_json(value)?;
        Ok(requests
            .iter()
            .zip(elements)
            .map(|(req, elem)| {
                let succeeded = elem
                    .get("succeeded")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                if !succeeded {
                    let message = elem
                        .get("message")
                        .or_else(|| elem.get("label"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("order rejected in batch")
                        .to_string();
                    return Err(Error::OrderRejected {
                        code: "batch".to_string(),
                        message,
                    });
                }
                let raw: RawOrder = serde_json::from_value(elem)
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                order_from_raw(req.symbol.clone(), &raw)
            })
            .collect())
    }

    /// Cancel several orders on one `symbol` in one request
    /// (`/api/v4/spot/cancel_batch_orders`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        let pair = Self::wire_symbol(symbol);
        let items: Vec<serde_json::Value> = order_ids
            .iter()
            .map(|id| serde_json::json!({ "currency_pair": pair, "id": id }))
            .collect();
        let body = serde_json::Value::Array(items).to_string();
        self.signed_request(
            HttpMethod::Post,
            "/api/v4/spot/cancel_batch_orders",
            "",
            &body,
        )?;
        Ok(())
    }
}

impl AdvancedOrders for Gate {
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Gate::amend_order(self, symbol, order_id, new_price, new_quantity)
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Gate::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Gate::cancel_batch(self, symbol, order_ids)
    }
    /// Gate has no OCO order-list (it uses standalone price-triggered orders), so
    /// this returns an [`Error::Exchange`].
    fn place_oco(&mut self, _request: &OcoRequest) -> Result<Vec<Order>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Gate has no OCO order-list; use price-triggered orders".to_string(),
        })
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

    fn signed_ws_client(now_ms: i64) -> (Gate, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let gate = Gate::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (gate, ws)
    }

    #[test]
    fn subscribe_user_data_authenticates_and_streams_orders_and_balances() {
        let (mut gate, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(r#"{"channel":"spot.orders","event":"subscribe"}"#.to_string())),
            Ok(Some(r#"{"channel":"spot.balances","event":"subscribe"}"#.to_string())),
            Ok(Some(
                r#"{"time":1,"channel":"spot.orders","event":"update","result":[{"id":"55",
                "text":"my","currency_pair":"BTC_USDT","side":"buy","type":"limit","status":"closed",
                "amount":"1","left":"0","price":"100","avg_deal_price":"100"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"time":2,"channel":"spot.balances","event":"update","result":[{"currency":"USDT",
                "available":"900","total":"950"}]}"#
                    .to_string(),
            )),
        ]);
        gate.subscribe_user_data().unwrap();
        assert_eq!(ws.connected_urls()[0], "wss://api.gateio.ws/ws/v4/");
        assert!(ws.sent()[0].contains(r#""channel":"spot.orders""#));
        assert!(ws.sent()[0].contains(r#""KEY":"APIKEY""#));
        assert!(ws.sent()[0].contains(r#""SIGN""#));
        assert!(ws.sent()[1].contains(r#""channel":"spot.balances""#));

        let events = gate.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "55");
        assert_eq!(order.symbol, symbol());
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.filled_quantity, dec!(1));
        assert_eq!(order.average_price, Some(dec!(100)));
        let Event::BalanceUpdate(balances) = &events[1] else {
            panic!("second event must be a balance update");
        };
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].asset, "USDT");
        assert_eq!(balances[0].free, dec!(900));
        assert_eq!(balances[0].locked, dec!(50));
    }

    #[test]
    fn subscribe_user_data_requires_credentials() {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut gate = Gate::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            gate.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut gate, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![]);
        gate.subscribe_user_data().unwrap();
        gate.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == "ping"));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut gate, ws) = signed_ws_client(1_700_000_000_000);
        gate.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_fresh_signed_frames() {
        let (mut gate, ws) = signed_ws_client(1_700_000_000_000);
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        gate.subscribe_user_data().unwrap();

        let events = gate.poll_events();
        assert!(events.contains(&Event::Disconnected));
        assert!(events.contains(&Event::Reconnected));
        // Two private connections (initial + reconnect); each re-signs the
        // spot.orders subscribe, so four signed subscribe frames total.
        let signed_subs = ws
            .sent()
            .into_iter()
            .filter(|f| f.contains(r#""SIGN""#))
            .count();
        assert_eq!(signed_subs, 4);
        assert_eq!(ws.connected_urls().len(), 2);
        assert_eq!(ws.connected_urls()[1], "wss://api.gateio.ws/ws/v4/");
    }

    #[test]
    fn place_and_cancel_order_over_ws() {
        let (mut gate, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"request_id":"login","header":{"status":"200","channel":"spot.login",
                "event":"api"},"data":{"result":{"api_key":"APIKEY"}}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"request_id":"o","header":{"status":"200","channel":"spot.order_place",
                "event":"api"},"data":{"result":{"id":"55","text":"t-my","currency_pair":"BTC_USDT",
                "side":"buy","type":"limit","status":"closed","amount":"1","left":"0","price":"100",
                "avg_deal_price":"100"}}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"request_id":"c","header":{"status":"200","channel":"spot.order_cancel",
                "event":"api"},"data":{"result":{"id":"55"}}}"#
                    .to_string(),
            )),
        ]);
        let order = gate
            .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "55");
        assert_eq!(order.symbol, symbol());
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.filled_quantity, dec!(1));
        assert_eq!(ws.connected_urls()[0], "wss://api.gateio.ws/ws/v4/");
        // The login frame is sent first, then the order request.
        assert!(ws.sent()[0].contains(r#""channel":"spot.login""#));
        assert!(ws.sent()[0].contains(r#""signature""#));
        assert!(ws.sent()[1].contains(r#""channel":"spot.order_place""#));
        assert!(ws.sent()[1].contains(r#""currency_pair":"BTC_USDT""#));

        gate.cancel_order_ws(&symbol(), "55").unwrap();
        assert!(ws.sent()[2].contains(r#""channel":"spot.order_cancel""#));
        assert!(ws.sent()[2].contains(r#""order_id":"55""#));
    }

    #[test]
    fn ws_order_surfaces_rejection() {
        let (mut gate, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"header":{"status":"200","channel":"spot.login","event":"api"}}"#.to_string(),
            )),
            Ok(Some(
                r#"{"header":{"status":"400","channel":"spot.order_place","event":"api"},
                "data":{"errs":{"label":"BALANCE_NOT_ENOUGH","message":"not enough balance"}}}"#
                    .to_string(),
            )),
        ]);
        assert!(matches!(
            gate.place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn ws_order_requires_credentials() {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut gate = Gate::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            gate.place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    fn signed_futures_client(now_ms: i64) -> (Gate, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let gate = Gate::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (gate, mock)
    }

    const GATE_ORDER: &str = r#"{"id":"1","text":"","side":"buy","type":"limit","status":"open",
        "amount":"1","filled_amount":"0","price":"100","avg_deal_price":"0"}"#;

    #[test]
    fn stp_maps_to_stp_act() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(200, GATE_ORDER);
        gate.place_order(
            &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                .with_stp(SelfTradePrevention::ExpireBoth),
        )
        .unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[0].body.as_ref().unwrap().contains(r#""stp_act":"cb""#));
    }

    #[test]
    fn amend_order_patches_in_place() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":"1","text":"","side":"buy","type":"limit","status":"open",
            "amount":"2","filled_amount":"0","price":"101","avg_deal_price":"0"}"#,
        );
        let order = gate
            .amend_order(&symbol(), "1", Some(dec!(101)), Some(dec!(2)))
            .unwrap();
        assert_eq!(order.quantity, dec!(2));
        assert_eq!(order.price, Some(dec!(101)));
        let reqs = mock.recorded_requests();
        assert_eq!(reqs[0].method, HttpMethod::Patch);
        assert!(reqs[0]
            .url
            .contains("/api/v4/spot/orders/1?currency_pair=BTC_USDT"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""amount":"2""#));
        assert!(body.contains(r#""price":"101""#));
    }

    #[test]
    fn place_batch_reads_succeeded_flag() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(
            200,
            r#"[{"succeeded":true,"id":"o1","text":"","side":"buy","type":"limit","status":"open",
            "amount":"1","filled_amount":"0","price":"100","avg_deal_price":"0"},
            {"succeeded":false,"label":"BALANCE_NOT_ENOUGH","message":"no funds"}]"#,
        );
        let results = gate
            .place_batch(&[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(101)),
            ])
            .unwrap();
        assert_eq!(results[0].as_ref().unwrap().id, "o1");
        assert!(matches!(
            results[1].as_ref().unwrap_err(),
            Error::OrderRejected { .. }
        ));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/spot/batch_orders"));
    }

    #[test]
    fn cancel_batch_is_one_call() {
        let (gate, mock) = signed_client(1_000_000);
        mock.push_json(200, "[]");
        gate.cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/api/v4/spot/cancel_batch_orders"));
    }

    #[test]
    fn place_oco_is_unsupported() {
        let (mut gate, _mock) = signed_client(1_000_000);
        assert!(matches!(
            AdvancedOrders::place_oco(
                &mut gate,
                &OcoRequest::new(symbol(), OrderSide::Sell, dec!(1), dec!(110), dec!(95))
            )
            .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    fn futures_client() -> (Gate, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        (
            Gate::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    #[test]
    fn futures_ticker_takes_bid_ask_from_the_book() {
        let (gate, mock) = futures_client();
        mock.push_json(
            200,
            r#"[{"contract":"BTC_USDT","last":"20000","volume_24h_base":"1234"}]"#,
        );
        mock.push_json(
            200,
            r#"{"id":66,"bids":[{"p":"19999","s":10}],"asks":[{"p":"20001","s":8}]}"#,
        );
        let ticker = gate.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.ask, dec!(20001));
        assert_eq!(ticker.volume, dec!(1234));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/tickers?contract=BTC_USDT"));
    }

    #[test]
    fn futures_klines_parse_object_rows() {
        let (gate, mock) = futures_client();
        mock.push_json(
            200,
            r#"[{"t":1700000000,"v":12,"o":"100","h":"110","l":"95","c":"105"}]"#,
        );
        let candles = gate.klines(&symbol(), "1h", 1).unwrap();
        assert!((candles[0].open - 100.0).abs() < 1e-9);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!((candles[0].close - 105.0).abs() < 1e-9);
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/candlesticks"));
    }

    #[test]
    fn futures_order_book_parses_object_levels() {
        let (gate, mock) = futures_client();
        mock.push_json(
            200,
            r#"{"id":77,"bids":[{"p":"100","s":1}],"asks":[{"p":"101","s":2}]}"#,
        );
        let book = gate.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 77);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101), dec!(2)));
    }

    #[test]
    fn futures_market_order_uses_signed_size_and_ioc() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":88,"size":2,"left":0,"price":"0","fill_price":"20000",
            "status":"finished","finish_as":"filled","text":""}"#,
        );
        let order = gate
            .place_order(&OrderRequest::market_buy(symbol(), dec!(2)))
            .unwrap();
        assert_eq!(order.id, "88");
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.average_price, Some(dec!(20000)));
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v4/futures/usdt/orders"));
        let body = req.body.as_ref().unwrap();
        assert!(body.contains(r#""contract":"BTC_USDT""#));
        assert!(body.contains(r#""size":2"#));
        assert!(body.contains(r#""price":"0""#));
        assert!(body.contains(r#""tif":"ioc""#));
        assert!(body.contains(r#""reduce_only":false"#));
    }

    #[test]
    fn futures_limit_sell_signs_size_negative() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":90,"size":-3,"left":3,"price":"21000","fill_price":"0",
            "status":"open","finish_as":"","text":""}"#,
        );
        let order = gate
            .place_order(&OrderRequest::limit_sell(symbol(), dec!(3), dec!(21000)))
            .unwrap();
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.order_type, OrderType::Limit);
        let reqs = mock.recorded_requests();
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""size":-3"#));
        assert!(body.contains(r#""price":"21000""#));
    }

    #[test]
    fn derivatives_positions_parse_cross_and_isolated() {
        let (mut gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"[{"contract":"BTC_USDT","size":3,"leverage":"10","entry_price":"20000",
            "mark_price":"20100","unrealised_pnl":"30"},
            {"contract":"ETH_USDT","size":-2,"leverage":"0","cross_leverage_limit":"5",
            "entry_price":"3000","mark_price":"2900","unrealised_pnl":"200"}]"#,
        );
        let positions = Derivatives::positions(&mut gate, None).unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(3));
        assert_eq!(positions[0].leverage, dec!(10));
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert_eq!(positions[1].side, PositionSide::Short);
        assert_eq!(positions[1].leverage, dec!(5));
        assert_eq!(positions[1].margin_mode, MarginMode::Cross);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/positions"));
    }

    #[test]
    fn derivatives_set_leverage_and_cross_switch() {
        let (mut gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(200, "{}");
        Derivatives::set_leverage(&mut gate, &symbol(), 5).unwrap();
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/positions/BTC_USDT/leverage?leverage=5"));

        mock.push_json(200, "{}");
        Derivatives::set_margin_mode(&mut gate, &symbol(), MarginMode::Cross).unwrap();
        assert!(mock.recorded_requests()[1]
            .url
            .contains("leverage?leverage=0"));
    }

    #[test]
    fn derivatives_close_position_is_reduce_only_opposite() {
        let (mut gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"[{"contract":"BTC_USDT","size":3,"leverage":"10","entry_price":"20000",
            "mark_price":"20100","unrealised_pnl":"30"}]"#,
        );
        mock.push_json(
            200,
            r#"{"id":99,"size":-3,"left":0,"price":"0","fill_price":"20100",
            "status":"finished","finish_as":"filled","text":""}"#,
        );
        let order = Derivatives::close_position(&mut gate, &symbol()).unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        let reqs = mock.recorded_requests();
        let body = reqs[1].body.as_ref().unwrap();
        assert!(body.contains(r#""size":-3"#));
        assert!(body.contains(r#""reduce_only":true"#));
    }

    #[test]
    fn futures_balances_split_total_and_available() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"{"currency":"USDT","total":"1000","available":"800"}"#,
        );
        let bals = gate.balances().unwrap();
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].free, dec!(800));
        assert_eq!(bals[0].locked, dec!(200));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/accounts"));
    }

    #[test]
    fn futures_query_order_uses_futures_endpoint_and_shape() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":88,"size":2,"left":0,"price":"100","fill_price":"100",
            "status":"finished","finish_as":"filled","text":""}"#,
        );
        let order = gate.query_order(&symbol(), "88").unwrap();
        assert_eq!(order.id, "88");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.status, OrderStatus::Filled);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/orders/88"));
    }

    #[test]
    fn futures_cancel_order_targets_futures_endpoint() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"{"id":88,"size":0,"left":0,"status":"finished","finish_as":"cancelled"}"#,
        );
        gate.cancel_order(&symbol(), "88").unwrap();
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Delete);
        assert!(req.url.contains("/api/v4/futures/usdt/orders/88"));
    }

    #[test]
    fn futures_open_orders_parse_contract_shape() {
        let (gate, mock) = signed_futures_client(1_000_000);
        mock.push_json(
            200,
            r#"[{"id":90,"size":-3,"left":3,"price":"21000","fill_price":"0",
            "status":"open","finish_as":"","text":""}]"#,
        );
        let orders = gate.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);
        assert_eq!(orders[0].status, OrderStatus::New);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v4/futures/usdt/orders?contract=BTC_USDT&status=open"));
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
