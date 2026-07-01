//! OKX (v5 API) — the third exchange.
//!
//! Generic over the injected transports and tested offline, like the others.
//! Its bespoke parts: dash-form symbols (`BTC-USDT`), a `{code, msg, data}`
//! envelope, and the `OK-ACCESS-*` signing scheme — base64(HMAC-SHA256) over an
//! **ISO-8601** timestamp plus method, request path and body, with a passphrase
//! header. The ISO-8601 timestamp is derived from the injectable clock with a
//! dependency-free civil-date conversion, so signing stays deterministic.
//!
//! [`AdvancedOrders`] is native: STP via `stpMode` on order create, amend via
//! `/api/v5/trade/amend-order`, batch place/cancel via
//! `/api/v5/trade/batch-orders` and `.../cancel-batch-orders`, and OCO via
//! `/api/v5/trade/order-algo` (`ordType=oco`) — OKX models an OCO as one algo
//! order, so `place_oco` returns a single order carrying the `algoId`.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType, SelfTradePrevention};
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

/// Convert Unix milliseconds to an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
fn iso8601_from_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let mut rem = ms.rem_euclid(86_400_000);
    let millis = rem % 1000;
    rem /= 1000;
    let secs = rem % 60;
    rem /= 60;
    let mins = rem % 60;
    let hours = rem / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}.{millis:03}Z")
}

/// Days since the Unix epoch to a `(year, month, day)` civil date (Howard
/// Hinnant's algorithm), avoiding a date-library dependency.
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

/// An OKX client over injected transports.
pub struct Okx {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    inst_type: &'static str,
    td_mode: &'static str,
    testnet: bool,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
    /// Set once the private stream is subscribed, so [`poll_events`](Self::poll_events)
    /// re-subscribes it after a drop.
    user_data_active: bool,
    /// A dedicated logged-in connection to the private WebSocket order API, opened
    /// lazily on the first [`place_order_ws`](Self::place_order_ws) /
    /// [`cancel_order_ws`](Self::cancel_order_ws) call.
    ws_api_connection: Option<Box<dyn WsConnection>>,
}

impl Okx {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://www.okx.com".to_string(),
            inst_type: inst_type(options.market_type),
            td_mode: td_mode(options.market_type),
            testnet: options.testnet,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            private_connection: None,
            user_data_active: false,
            ws_api_connection: None,
        }
    }

    /// Build a public OKX client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated OKX client (credentials must carry a passphrase).
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

    /// The OKX wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTC-USDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}-{}", symbol.base(), symbol.quote())
    }

    /// Whether this client targets a perpetual-swap market (instId gets a
    /// `-SWAP` suffix and requests carry `instType=SWAP`).
    fn is_swap(&self) -> bool {
        self.inst_type == "SWAP"
    }

    /// The OKX instrument id for `symbol` in this client's market: `BTC-USDT` for
    /// spot, `BTC-USDT-SWAP` for the USDⓈ-M perpetual.
    fn inst_id(&self, symbol: &Symbol) -> String {
        if self.is_swap() {
            format!("{}-{}-SWAP", symbol.base(), symbol.quote())
        } else {
            format!("{}-{}", symbol.base(), symbol.quote())
        }
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("instId={}", self.inst_id(symbol));
        let data = self.get("/api/v5/market/ticker", &query)?;
        let list: Vec<RawTicker> = parse_json(data)?;
        let entry = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last)?,
            bid: parse_decimal(&entry.bid_px)?,
            ask: parse_decimal(&entry.ask_px)?,
            volume: parse_decimal(&entry.vol24h)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). OKX returns
    /// newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "instId={}&bar={}&limit={limit}",
            self.inst_id(symbol),
            map_bar(interval),
        );
        let data = self.get("/api/v5/market/candles", &query)?;
        let rows: Vec<Vec<String>> = parse_json(data)?;
        let mut candles = rows
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
        let query = format!("instId={}&sz={depth}", self.inst_id(symbol));
        let data = self.get("/api/v5/market/books", &query)?;
        let list: Vec<RawDepth> = parse_json(data)?;
        let raw = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no book for {symbol}")))?;
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
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured,
    /// or a transport error on failure.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "trades")
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
        self.subscribe(symbol, "tickers")
    }

    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let wire = self.inst_id(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect(ws_url(self.testnet))?;
            self.connection = Some(connection);
        }
        let message =
            format!(r#"{{"op":"subscribe","args":[{{"channel":"{channel}","instId":"{wire}"}}]}}"#);
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
                .unwrap_or_else(|| symbol_from_inst_id(wire))
        };
        let mut events = Vec::new();
        if let Some(connection) = self.connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // Drain the private user-data stream (orders/account channels), if open.
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // A dropped private stream is re-subscribed with a fresh op:login (the
        // signature is time-bound, so a stale replay would be rejected).
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
        let url = ws_url(self.testnet);
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Open the private user-data stream (`wss://.../ws/v5/private`). Logs in with
    /// an `op:login` frame (sign = base64(HMAC-SHA256) over
    /// `<epochSeconds>GET/users/self/verify`), then subscribes to the `orders`
    /// and `account` channels. Afterwards [`poll_events`](Self::poll_events) also
    /// surfaces the account's own [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// A dropped private stream is re-subscribed automatically on the next
    /// [`poll_events`](Self::poll_events); call
    /// [`keepalive_user_data`](Self::keepalive_user_data) periodically to keep it
    /// from being dropped for inactivity.
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials or a passphrase,
    /// [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the request fails.
    /// Build the OKX WS `op:login` frame. The login signs
    /// `<timestamp>GET/users/self/verify`, where the timestamp is Unix epoch
    /// seconds.
    fn ws_login_frame(&self) -> Result<String> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed WebSocket requires credentials",
        ))?;
        let passphrase = creds
            .passphrase
            .as_deref()
            .ok_or(Error::InvalidCredentials("OKX requires a passphrase"))?;
        let timestamp = (self.now_ms)() / 1000;
        let sign = hmac_sha256_base64(
            creds.api_secret.as_bytes(),
            format!("{timestamp}GET/users/self/verify").as_bytes(),
        );
        Ok(format!(
            r#"{{"op":"login","args":[{{"apiKey":"{}","passphrase":"{passphrase}","timestamp":"{timestamp}","sign":"{sign}"}}]}}"#,
            creds.api_key
        ))
    }

    pub fn subscribe_user_data(&mut self) -> Result<()> {
        let login = self.ws_login_frame()?;
        let subscribe = format!(
            r#"{{"op":"subscribe","args":[{{"channel":"orders","instType":"{}"}},{{"channel":"account"}}]}}"#,
            self.inst_type
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect(ws_private_url(self.testnet))?;
        connection.send(&login)?;
        connection.send(&subscribe)?;
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send an application-level heartbeat (the `ping` text frame OKX expects) on
    /// the private stream so it is not dropped for inactivity. A no-op before
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

    /// Place an order over the OKX private WebSocket order API (`op:order`). Builds
    /// the same args as the REST path and exchanges them on the lazily-opened,
    /// logged-in connection.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the order is invalid or the venue rejects it.
    pub fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let ord_type = if request.post_only && request.order_type == OrderType::Limit {
            "post_only"
        } else {
            ord_type_str(request.order_type)
        };
        let mut arg = serde_json::json!({
            "instId": self.inst_id(&request.symbol),
            "tdMode": self.td_mode,
            "side": side_str(request.side),
            "ordType": ord_type,
            "sz": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            arg["px"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            arg["clOrdId"] = serde_json::json!(id.clone());
        }
        let data = self.ws_order_request("order", &arg)?;
        let list: Vec<PlaceResult> = parse_json(data)?;
        let placed = list.into_iter().next().ok_or_else(|| Error::Exchange {
            code: "empty".to_string(),
            message: "empty order response".to_string(),
        })?;
        if placed.s_code != "0" {
            return Err(Error::OrderRejected {
                code: placed.s_code,
                message: placed.s_msg,
            });
        }
        Ok(Order {
            id: placed.ord_id,
            client_order_id: (!placed.cl_ord_id.is_empty()).then_some(placed.cl_ord_id),
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

    /// Cancel an order over the OKX private WebSocket order API (`op:cancel-order`).
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the order is unknown or the request fails.
    pub fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let arg = serde_json::json!({
            "instId": self.inst_id(symbol),
            "ordId": order_id,
        });
        self.ws_order_request("cancel-order", &arg)?;
        Ok(())
    }

    /// Open and log into the private WebSocket order connection if needed,
    /// consuming the login acknowledgement so later requests read their own
    /// responses.
    fn ensure_ws_api(&mut self) -> Result<()> {
        if self.ws_api_connection.is_some() {
            return Ok(());
        }
        let login = self.ws_login_frame()?;
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect(ws_private_url(self.testnet))?;
        connection.send(&login)?;
        connection.recv()?; // consume the login acknowledgement
        self.ws_api_connection = Some(connection);
        Ok(())
    }

    /// Send an order-API request frame and return its `data`, mapping a non-`"0"`
    /// top-level `code` onto the error taxonomy.
    fn ws_order_request(&mut self, op: &str, arg: &serde_json::Value) -> Result<serde_json::Value> {
        self.ensure_ws_api()?;
        let id = (self.now_ms)().to_string();
        let frame = serde_json::json!({ "id": id, "op": op, "args": [arg] }).to_string();
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
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("1");
        if code != "0" {
            let message = value
                .get("msg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            return Err(Error::OrderRejected {
                code: code.to_string(),
                message,
            });
        }
        Ok(value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Place an order.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let ord_type = if request.post_only && request.order_type == OrderType::Limit {
            "post_only"
        } else {
            ord_type_str(request.order_type)
        };
        let mut body = serde_json::json!({
            "instId": self.inst_id(&request.symbol),
            "tdMode": self.td_mode,
            "side": side_str(request.side),
            "ordType": ord_type,
            "sz": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            body["px"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            body["clOrdId"] = serde_json::json!(id.clone());
        }
        if request.reduce_only {
            body["reduceOnly"] = serde_json::json!(true);
        }
        if let Some(mode) = stp_mode_str(request.stp) {
            body["stpMode"] = serde_json::json!(mode);
        }
        let data = self.signed_request(
            HttpMethod::Post,
            "/api/v5/trade/order",
            "",
            &body.to_string(),
        )?;
        let list: Vec<PlaceResult> = parse_json(data)?;
        let placed = list.into_iter().next().ok_or_else(|| Error::Exchange {
            code: "empty".to_string(),
            message: "empty order response".to_string(),
        })?;
        if placed.s_code != "0" {
            return Err(Error::OrderRejected {
                code: placed.s_code,
                message: placed.s_msg,
            });
        }
        Ok(Order {
            id: placed.ord_id,
            client_order_id: (!placed.cl_ord_id.is_empty()).then_some(placed.cl_ord_id),
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
            "instId": self.inst_id(symbol),
            "ordId": order_id,
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v5/trade/cancel-order",
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
        let query = format!("instId={}&ordId={order_id}", self.inst_id(symbol));
        let data = self.signed_request(HttpMethod::Get, "/api/v5/trade/order", &query, "")?;
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
        let mut query = format!("instType={}", self.inst_type);
        if let Some(s) = symbol {
            query.push_str("&instId=");
            query.push_str(&self.inst_id(s));
        }
        let data =
            self.signed_request(HttpMethod::Get, "/api/v5/trade/orders-pending", &query, "")?;
        let list: Vec<RawOrder> = parse_json(data)?;
        list.iter()
            .map(|raw| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| symbol_from_inst_id(&raw.inst_id));
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Account balances.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let data = self.signed_request(HttpMethod::Get, "/api/v5/account/balance", "", "")?;
        let list: Vec<RawBalance> = parse_json(data)?;
        let account = list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound("no balance account".to_string()))?;
        Ok(account
            .details
            .iter()
            .map(|d| Balance {
                asset: d.ccy.clone(),
                free: dec_or_zero(&d.avail_bal),
                locked: dec_or_zero(&d.frozen_bal),
            })
            .collect())
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_envelope(&response.body)
    }

    /// Sign with the `OK-ACCESS-*` headers: base64(HMAC-SHA256) over
    /// `isoTimestamp + METHOD + requestPath + body`.
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
            .ok_or(Error::InvalidCredentials("OKX requires a passphrase"))?;
        let timestamp = iso8601_from_ms((self.now_ms)());
        let request_path = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let prehash = format!("{timestamp}{}{request_path}{body}", method.as_str());
        let signature = hmac_sha256_base64(creds.api_secret.as_bytes(), prehash.as_bytes());
        let url = format!("{}{request_path}", self.rest_base);
        let mut request = HttpRequest::new(method, url)
            .with_header("OK-ACCESS-KEY", creds.api_key.clone())
            .with_header("OK-ACCESS-SIGN", signature)
            .with_header("OK-ACCESS-TIMESTAMP", timestamp)
            .with_header("OK-ACCESS-PASSPHRASE", passphrase.to_string());
        if self.testnet {
            request = request.with_header("x-simulated-trading", "1");
        }
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_envelope(&response.body)
    }
}

fn inst_type(market_type: MarketType) -> &'static str {
    match market_type {
        MarketType::Spot | MarketType::Margin => "SPOT",
        MarketType::UsdMFutures | MarketType::CoinMFutures => "SWAP",
    }
}

fn td_mode(market_type: MarketType) -> &'static str {
    match market_type {
        MarketType::Spot => "cash",
        _ => "cross",
    }
}

/// Map a unified interval to OKX's `bar` (`1m`, `1H`, `1D`).
fn map_bar(interval: &str) -> String {
    match interval {
        "1h" => "1H",
        "2h" => "2H",
        "4h" => "4H",
        "6h" => "6H",
        "12h" => "12H",
        "1d" => "1D",
        "1w" => "1W",
        other => other,
    }
    .to_string()
}

fn ws_url(testnet: bool) -> &'static str {
    if testnet {
        "wss://wspap.okx.com:8443/ws/v5/public"
    } else {
        "wss://ws.okx.com:8443/ws/v5/public"
    }
}

/// The private (user-data) WebSocket URL for a network.
fn ws_private_url(testnet: bool) -> &'static str {
    if testnet {
        "wss://wspap.okx.com:8443/ws/v5/private"
    } else {
        "wss://ws.okx.com:8443/ws/v5/private"
    }
}

fn unwrap_envelope(body: &str) -> Result<serde_json::Value> {
    let envelope: Envelope =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if envelope.code != "0" {
        return Err(map_error(&envelope.code, &envelope.msg));
    }
    Ok(envelope.data)
}

fn parse_json<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn map_error(code: &str, msg: &str) -> Error {
    match code {
        "50011" | "50061" => Error::RateLimited { retry_after: None },
        "50102" | "50103" | "50104" | "50113" => Error::Auth(msg.to_string()),
        "51008" | "51127" | "51131" => Error::InsufficientBalance,
        "51000" | "51001" => Error::InvalidSymbol(msg.to_string()),
        "51603" => Error::NotFound(msg.to_string()),
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

fn ord_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market | OrderType::StopMarket => "market",
        OrderType::Limit | OrderType::StopLimit => "limit",
    }
}

/// The OKX `stpMode` value for a self-trade-prevention policy, or `None` to omit.
fn stp_mode_str(stp: SelfTradePrevention) -> Option<&'static str> {
    match stp {
        SelfTradePrevention::None => None,
        SelfTradePrevention::ExpireMaker => Some("cancel_maker"),
        SelfTradePrevention::ExpireTaker => Some("cancel_taker"),
        SelfTradePrevention::ExpireBoth => Some("cancel_both"),
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_ord_type(raw: &str) -> Result<OrderType> {
    match raw {
        "market" => Ok(OrderType::Market),
        "limit" | "post_only" | "fok" | "ioc" => Ok(OrderType::Limit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_state(raw: &str) -> Result<OrderStatus> {
    match raw {
        "live" => Ok(OrderStatus::New),
        "partially_filled" => Ok(OrderStatus::PartiallyFilled),
        "filled" => Ok(OrderStatus::Filled),
        "canceled" | "mmp_canceled" => Ok(OrderStatus::Canceled),
        other => Err(Error::Deserialization(format!("unknown state {other:?}"))),
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

fn parse_levels(levels: &[Vec<String>]) -> Result<Vec<BookLevel>> {
    levels
        .iter()
        .map(|level| {
            let price = parse_decimal(
                level
                    .first()
                    .ok_or_else(|| Error::Deserialization("level price missing".to_string()))?,
            )?;
            let quantity =
                parse_decimal(level.get(1).ok_or_else(|| {
                    Error::Deserialization("level quantity missing".to_string())
                })?)?;
            Ok(BookLevel { price, quantity })
        })
        .collect()
}

fn parse_kline_row(row: &[String]) -> Result<Candle> {
    // OKX candle: [ts, o, h, l, c, vol, volCcy, volCcyQuote, confirm].
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
        id: raw.ord_id.clone(),
        client_order_id: (!raw.cl_ord_id.is_empty()).then(|| raw.cl_ord_id.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_ord_type(&raw.ord_type)?,
        status: parse_state(&raw.state)?,
        quantity: parse_decimal(&raw.sz)?,
        filled_quantity: dec_or_zero(&raw.acc_fill_sz),
        price: nonzero_decimal(&raw.px),
        average_price: nonzero_decimal(&raw.avg_px),
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
    let Some(channel) = value
        .get("arg")
        .and_then(|a| a.get("channel"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new()); // event/ack frame
    };
    let empty = Vec::new();
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    if channel == "trades" {
        data.iter()
            .map(|t| {
                Ok(Event::Trade(TradePrint {
                    symbol: resolve(field_str(t, "instId")?),
                    price: parse_decimal(field_str(t, "px")?)?,
                    quantity: parse_decimal(field_str(t, "sz")?)?,
                    aggressor: parse_side(field_str(t, "side")?)?,
                    timestamp: opt_str(t, "ts").parse().unwrap_or(0),
                }))
            })
            .collect()
    } else if channel == "tickers" {
        data.iter()
            .map(|t| {
                Ok(Event::Ticker(Ticker {
                    symbol: resolve(field_str(t, "instId")?),
                    last: parse_decimal(field_str(t, "last")?)?,
                    bid: dec_or_zero(opt_str(t, "bidPx")),
                    ask: dec_or_zero(opt_str(t, "askPx")),
                    volume: dec_or_zero(opt_str(t, "vol24h")),
                }))
            })
            .collect()
    } else if channel == "books" {
        let action = value.get("action").and_then(serde_json::Value::as_str);
        data.iter()
            .map(|b| {
                let symbol = resolve(&value_inst_id(&value));
                let update_id = opt_str(b, "ts").parse().unwrap_or(0);
                let bids = parse_ws_levels(b.get("bids"))?;
                let asks = parse_ws_levels(b.get("asks"))?;
                if action == Some("snapshot") {
                    Ok(Event::BookSnapshot(OrderBookSnapshot {
                        symbol,
                        last_update_id: update_id,
                        bids,
                        asks,
                    }))
                } else {
                    Ok(Event::BookDelta(BookDelta {
                        symbol,
                        first_update_id: update_id,
                        final_update_id: update_id,
                        bids,
                        asks,
                    }))
                }
            })
            .collect()
    } else if channel == "orders" {
        // Private order channel: each element shares the REST order shape.
        data.iter()
            .map(|raw| {
                let order: RawOrder = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                let symbol = symbol_from_inst_id(&order.inst_id);
                Ok(Event::OrderUpdate(order_from_raw(symbol, &order)?))
            })
            .collect()
    } else if channel == "account" {
        // Private account channel: each element carries a `details` array of
        // per-currency balances.
        data.iter()
            .map(|raw| {
                let balance: RawBalance = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                Ok(Event::BalanceUpdate(
                    balance
                        .details
                        .iter()
                        .map(|c| Balance {
                            asset: c.ccy.clone(),
                            free: dec_or_zero(&c.avail_bal),
                            locked: dec_or_zero(&c.frozen_bal),
                        })
                        .collect(),
                ))
            })
            .collect()
    } else {
        Ok(Vec::new())
    }
}

fn value_inst_id(value: &serde_json::Value) -> String {
    value
        .get("arg")
        .and_then(|a| a.get("instId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
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
    last: String,
    #[serde(rename = "bidPx")]
    bid_px: String,
    #[serde(rename = "askPx")]
    ask_px: String,
    vol24h: String,
}

#[derive(Deserialize)]
struct RawDepth {
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    #[serde(default)]
    ts: String,
}

#[derive(Deserialize)]
struct PlaceResult {
    #[serde(rename = "ordId", default)]
    ord_id: String,
    #[serde(rename = "clOrdId", default)]
    cl_ord_id: String,
    #[serde(rename = "sCode", default)]
    s_code: String,
    #[serde(rename = "sMsg", default)]
    s_msg: String,
}

#[derive(Deserialize)]
struct RawOrder {
    #[serde(rename = "instId", default)]
    inst_id: String,
    #[serde(rename = "ordId")]
    ord_id: String,
    #[serde(rename = "clOrdId", default)]
    cl_ord_id: String,
    side: String,
    #[serde(rename = "ordType")]
    ord_type: String,
    state: String,
    sz: String,
    #[serde(rename = "accFillSz", default)]
    acc_fill_sz: String,
    #[serde(default)]
    px: String,
    #[serde(rename = "avgPx", default)]
    avg_px: String,
}

#[derive(Deserialize)]
struct RawBalance {
    details: Vec<CoinDetail>,
}

#[derive(Deserialize)]
struct CoinDetail {
    ccy: String,
    #[serde(rename = "availBal", default)]
    avail_bal: String,
    #[serde(rename = "frozenBal", default)]
    frozen_bal: String,
}

impl MarketData for Okx {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Okx::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Okx::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Okx::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Okx::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Okx::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Okx::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Okx::poll_events(self)
    }
}

impl Execution for Okx {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Okx::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Okx::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Okx::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Okx::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Okx::balances(self)
    }
}

impl Exchange for Okx {
    fn name(&self) -> &'static str {
        "okx"
    }
}

impl WsUserData for Okx {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Okx::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Okx::keepalive_user_data(self)
    }
}

impl WsExecution for Okx {
    fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        Okx::place_order_ws(self, request)
    }
    fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Okx::cancel_order_ws(self, symbol, order_id)
    }
}

/// Map an OKX instrument id back to a canonical [`Symbol`], dropping any market
/// suffix (`BTC-USDT-SWAP` / `BTC-USDT-240329` -> `BTC/USDT`).
fn symbol_from_inst_id(inst_id: &str) -> Symbol {
    let mut parts = inst_id.split('-');
    match (parts.next(), parts.next()) {
        (Some(base), Some(quote)) if !base.is_empty() && !quote.is_empty() => {
            Symbol::new(base, quote)
        }
        _ => Symbol::new(inst_id, ""),
    }
}

impl Okx {
    /// Open positions (`/api/v5/account/positions`); flat positions are omitted.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let query = match symbol {
            Some(s) => format!("instType={}&instId={}", self.inst_type, self.inst_id(s)),
            None => format!("instType={}", self.inst_type),
        };
        let data = self.signed_request(HttpMethod::Get, "/api/v5/account/positions", &query, "")?;
        let raw: Vec<RawOkxPosition> = parse_json(data)?;
        raw.iter().filter_map(parse_okx_position).collect()
    }

    /// Set the leverage for `symbol` (`/api/v5/account/set-leverage`), preserving
    /// the current margin mode.
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let mgn_mode = self.current_margin_mode(symbol)?;
        self.apply_leverage(symbol, &leverage.to_string(), mgn_mode)
    }

    /// Set the margin mode for `symbol`. OKX couples the mode with the leverage,
    /// so the current leverage is preserved.
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let leverage = self
            .positions(Some(symbol))?
            .first()
            .map_or_else(|| "3".to_string(), |p| p.leverage.normalize().to_string());
        self.apply_leverage(symbol, &leverage, mode)
    }

    fn current_margin_mode(&self, symbol: &Symbol) -> Result<MarginMode> {
        Ok(self
            .positions(Some(symbol))?
            .first()
            .map_or(MarginMode::Cross, |p| p.margin_mode))
    }

    fn apply_leverage(&self, symbol: &Symbol, leverage: &str, mode: MarginMode) -> Result<()> {
        let mgn_mode = match mode {
            MarginMode::Cross => "cross",
            MarginMode::Isolated => "isolated",
        };
        let body = serde_json::json!({
            "instId": self.inst_id(symbol),
            "lever": leverage,
            "mgnMode": mgn_mode,
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v5/account/set-leverage",
            "",
            &body.to_string(),
        )?;
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

    /// Amend a resting order's price and/or quantity in place
    /// (`/api/v5/trade/amend-order`), then return the refreshed order.
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
        let mut body = serde_json::json!({
            "instId": self.inst_id(symbol),
            "ordId": order_id,
        });
        if let Some(q) = new_quantity {
            body["newSz"] = serde_json::json!(format_decimal(q));
        }
        if let Some(p) = new_price {
            body["newPx"] = serde_json::json!(format_decimal(p));
        }
        let data = self.signed_request(
            HttpMethod::Post,
            "/api/v5/trade/amend-order",
            "",
            &body.to_string(),
        )?;
        let list: Vec<PlaceResult> = parse_json(data)?;
        let amended = list.into_iter().next().ok_or_else(|| Error::Exchange {
            code: "empty".to_string(),
            message: "empty amend response".to_string(),
        })?;
        if amended.s_code != "0" {
            return Err(Error::OrderRejected {
                code: amended.s_code,
                message: amended.s_msg,
            });
        }
        self.query_order(symbol, order_id)
    }

    /// The JSON for one order in a batch (`/api/v5/trade/batch-orders`).
    fn batch_order_json(&self, request: &OrderRequest) -> serde_json::Value {
        let mut o = serde_json::json!({
            "instId": self.inst_id(&request.symbol),
            "tdMode": self.td_mode,
            "side": side_str(request.side),
            "ordType": ord_type_str(request.order_type),
            "sz": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            o["px"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            o["clOrdId"] = serde_json::json!(id.clone());
        }
        if request.reduce_only {
            o["reduceOnly"] = serde_json::json!(true);
        }
        o
    }

    /// Place several orders in one request (`/api/v5/trade/batch-orders`). Each
    /// element's `sCode` drives that leg's own [`Result`].
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails.
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        let items: Vec<serde_json::Value> =
            requests.iter().map(|r| self.batch_order_json(r)).collect();
        let body = serde_json::Value::Array(items).to_string();
        let data =
            self.signed_request(HttpMethod::Post, "/api/v5/trade/batch-orders", "", &body)?;
        let list: Vec<PlaceResult> = parse_json(data)?;
        Ok(requests
            .iter()
            .zip(list)
            .map(|(req, res)| {
                if res.s_code != "0" {
                    return Err(Error::OrderRejected {
                        code: res.s_code,
                        message: res.s_msg,
                    });
                }
                Ok(Order {
                    id: res.ord_id,
                    client_order_id: (!res.cl_ord_id.is_empty()).then_some(res.cl_ord_id),
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

    /// Cancel several orders on one `symbol` in one request
    /// (`/api/v5/trade/cancel-batch-orders`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        let inst = self.inst_id(symbol);
        let items: Vec<serde_json::Value> = order_ids
            .iter()
            .map(|id| serde_json::json!({ "instId": inst, "ordId": id }))
            .collect();
        let body = serde_json::Value::Array(items).to_string();
        self.signed_request(
            HttpMethod::Post,
            "/api/v5/trade/cancel-batch-orders",
            "",
            &body,
        )?;
        Ok(())
    }

    /// Place a one-cancels-other bracket. OKX models OCO as a single **algo**
    /// order (`/api/v5/trade/order-algo`, `ordType=oco`) with take-profit and
    /// stop-loss legs, so the returned vector holds one order carrying the
    /// `algoId`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the OCO is invalid or rejected.
    pub fn place_oco(&self, request: &OcoRequest) -> Result<Vec<Order>> {
        request.validate()?;
        let sl_ord_px = request
            .stop_limit_price
            .map_or_else(|| "-1".to_string(), format_decimal); // -1 = stop-market leg
        let mut body = serde_json::json!({
            "instId": self.inst_id(&request.symbol),
            "tdMode": self.td_mode,
            "side": side_str(request.side),
            "ordType": "oco",
            "sz": format_decimal(request.quantity),
            "tpTriggerPx": format_decimal(request.price),
            "tpOrdPx": format_decimal(request.price),
            "slTriggerPx": format_decimal(request.stop_price),
            "slOrdPx": sl_ord_px,
        });
        if let Some(id) = &request.client_order_id {
            body["algoClOrdId"] = serde_json::json!(id.clone());
        }
        let data = self.signed_request(
            HttpMethod::Post,
            "/api/v5/trade/order-algo",
            "",
            &body.to_string(),
        )?;
        let list: Vec<AlgoResult> = parse_json(data)?;
        let algo = list.into_iter().next().ok_or_else(|| Error::Exchange {
            code: "empty".to_string(),
            message: "empty algo response".to_string(),
        })?;
        if algo.s_code != "0" {
            return Err(Error::OrderRejected {
                code: algo.s_code,
                message: algo.s_msg,
            });
        }
        Ok(vec![Order {
            id: algo.algo_id,
            client_order_id: request.client_order_id.clone(),
            symbol: request.symbol.clone(),
            side: request.side,
            order_type: OrderType::StopLimit,
            status: OrderStatus::New,
            quantity: request.quantity,
            filled_quantity: Decimal::ZERO,
            price: Some(request.price),
            average_price: None,
        }])
    }
}

impl AdvancedOrders for Okx {
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Okx::amend_order(self, symbol, order_id, new_price, new_quantity)
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Okx::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Okx::cancel_batch(self, symbol, order_ids)
    }
    fn place_oco(&mut self, request: &OcoRequest) -> Result<Vec<Order>> {
        Okx::place_oco(self, request)
    }
}

#[derive(Deserialize)]
struct AlgoResult {
    #[serde(rename = "algoId", default)]
    algo_id: String,
    #[serde(rename = "sCode", default)]
    s_code: String,
    #[serde(rename = "sMsg", default)]
    s_msg: String,
}

impl Derivatives for Okx {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Okx::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Okx::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Okx::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Okx::close_position(self, symbol)
    }
}

#[derive(Deserialize)]
struct RawOkxPosition {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(rename = "posSide", default)]
    pos_side: String,
    pos: String,
    #[serde(rename = "avgPx", default)]
    avg_px: String,
    #[serde(rename = "markPx", default)]
    mark_px: String,
    lever: String,
    upl: String,
    #[serde(rename = "mgnMode")]
    mgn_mode: String,
}

fn parse_okx_position(raw: &RawOkxPosition) -> Option<Result<Position>> {
    let pos = match parse_decimal(&raw.pos) {
        Ok(pos) if !pos.is_zero() => pos,
        Ok(_) => return None, // flat position
        Err(e) => return Some(Err(e)),
    };
    let side = if raw.pos_side == "short" || pos.is_sign_negative() {
        PositionSide::Short
    } else {
        PositionSide::Long
    };
    let build = || -> Result<Position> {
        Ok(Position {
            symbol: symbol_from_inst_id(&raw.inst_id),
            side,
            quantity: pos.abs(),
            entry_price: parse_decimal(&raw.avg_px)?,
            mark_price: parse_decimal(&raw.mark_px)?,
            leverage: parse_decimal(&raw.lever)?,
            unrealized_pnl: parse_decimal(&raw.upl)?,
            margin_mode: if raw.mgn_mode.eq_ignore_ascii_case("isolated") {
                MarginMode::Isolated
            } else {
                MarginMode::Cross
            },
        })
    };
    Some(build())
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

    fn client() -> (Okx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        (
            Okx::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Okx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let okx = Okx::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (okx, mock)
    }

    fn signed_futures_client(now_ms: i64) -> (Okx, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
        let okx = Okx::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (okx, mock)
    }

    const OKX_POSITIONS: &str = r#"{"code":"0","msg":"","data":[
        {"instId":"BTC-USDT-SWAP","posSide":"net","pos":"0.5","avgPx":"20000","markPx":"20100","lever":"10","upl":"50","mgnMode":"isolated"}
    ]}"#;

    #[test]
    fn stp_maps_to_stp_mode() {
        let (okx, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"0","data":[{"ordId":"1","sCode":"0"}]}"#);
        okx.place_order(
            &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                .with_stp(SelfTradePrevention::ExpireTaker),
        )
        .unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""stpMode":"cancel_taker""#));
    }

    #[test]
    fn amend_order_amends_then_reads_back() {
        let (okx, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"0","data":[{"ordId":"1","sCode":"0"}]}"#);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"instId":"BTC-USDT","ordId":"1","side":"buy",
            "ordType":"limit","state":"live","sz":"2","px":"101"}]}"#,
        );
        let order = okx
            .amend_order(&symbol(), "1", Some(dec!(101)), Some(dec!(2)))
            .unwrap();
        assert_eq!(order.quantity, dec!(2));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/api/v5/trade/amend-order"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""newSz":"2""#));
        assert!(body.contains(r#""newPx":"101""#));
    }

    #[test]
    fn place_batch_per_order_results() {
        let (okx, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"0","data":[
            {"ordId":"o1","sCode":"0"},
            {"ordId":"","sCode":"51000","sMsg":"bad symbol"}]}"#,
        );
        let results = okx
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
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/api/v5/trade/batch-orders"));
        // The body is a bare JSON array of order objects.
        assert!(reqs[0].body.as_ref().unwrap().starts_with('['));
    }

    #[test]
    fn cancel_batch_is_one_call() {
        let (okx, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"0","data":[{}]}"#);
        okx.cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/api/v5/trade/cancel-batch-orders"));
    }

    #[test]
    fn place_oco_is_a_single_algo_order() {
        let (okx, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"algoId":"algo1","sCode":"0","sMsg":""}]}"#,
        );
        let legs = okx
            .place_oco(&OcoRequest::new(
                symbol(),
                OrderSide::Sell,
                dec!(1),
                dec!(110),
                dec!(95),
            ))
            .unwrap();
        assert_eq!(legs.len(), 1);
        assert_eq!(legs[0].id, "algo1");
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/api/v5/trade/order-algo"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""ordType":"oco""#));
        assert!(body.contains(r#""tpTriggerPx":"110""#));
        assert!(body.contains(r#""slTriggerPx":"95""#));
    }

    #[test]
    fn swap_client_appends_swap_to_inst_id() {
        let (okx, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"code":"0","msg":"","data":[{"instId":"BTC-USDT-SWAP","last":"20000","bidPx":"19999","askPx":"20001","vol24h":"1000"}]}"#,
        );
        okx.ticker(&symbol()).unwrap();
        assert!(mock.recorded_requests()[0]
            .url
            .contains("instId=BTC-USDT-SWAP"));
    }

    #[test]
    fn derivatives_positions_parse() {
        let (mut okx, mock) = signed_futures_client(1000);
        mock.push_json(200, OKX_POSITIONS);
        let positions = Derivatives::positions(&mut okx, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(0.5));
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v5/account/positions"));
        assert!(mock.recorded_requests()[0].url.contains("instType=SWAP"));
    }

    #[test]
    fn derivatives_set_leverage_preserves_margin_mode() {
        let (mut okx, mock) = signed_futures_client(1000);
        mock.push_json(200, OKX_POSITIONS); // current mgnMode lookup
        mock.push_json(
            200,
            r#"{"code":"0","msg":"","data":[{"lever":"20","mgnMode":"isolated","instId":"BTC-USDT-SWAP"}]}"#,
        );
        Derivatives::set_leverage(&mut okx, &symbol(), 20).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/api/v5/account/set-leverage"));
        let body = reqs[1].body.as_deref().unwrap();
        assert!(body.contains(r#""lever":"20""#));
        assert!(body.contains(r#""mgnMode":"isolated""#));
    }

    #[test]
    fn derivatives_close_position_reduce_only() {
        let (mut okx, mock) = signed_futures_client(1000);
        mock.push_json(200, OKX_POSITIONS);
        mock.push_json(
            200,
            r#"{"code":"0","msg":"","data":[{"ordId":"9","clOrdId":"","sCode":"0","sMsg":""}]}"#,
        );
        Derivatives::close_position(&mut okx, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/api/v5/trade/order"));
        let body = reqs[1].body.as_deref().unwrap();
        assert!(body.contains(r#""side":"sell""#));
        assert!(body.contains(r#""reduceOnly":true"#));
    }

    #[test]
    fn iso8601_conversion() {
        assert_eq!(iso8601_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso8601_from_ms(1_700_000_000_000),
            "2023-11-14T22:13:20.000Z"
        );
        assert_eq!(
            iso8601_from_ms(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn wire_symbol_uses_dash() {
        assert_eq!(Okx::wire_symbol(&symbol()), "BTC-USDT");
    }

    #[test]
    fn interval_mapping() {
        assert_eq!(map_bar("1m"), "1m");
        assert_eq!(map_bar("1h"), "1H");
        assert_eq!(map_bar("1d"), "1D");
    }

    #[test]
    fn ticker_unwraps_envelope() {
        let (okx, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"0","msg":"","data":[{"instId":"BTC-USDT","last":"20000.5",
            "bidPx":"20000","askPx":"20001","vol24h":"1234"}]}"#,
        );
        let ticker = okx.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(20000));
        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://www.okx.com/api/v5/market/ticker?instId=BTC-USDT"
        );
    }

    #[test]
    fn klines_reversed() {
        let (okx, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"0","data":[
            ["1700000060000","105","106","104","105.5","2","0","0","1"],
            ["1700000000000","100","110","95","105","12","0","0","1"]]}"#,
        );
        let candles = okx.klines(&symbol(), "1H", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert_eq!(candles[1].timestamp, 1_700_000_060_000);
    }

    #[test]
    fn order_book_parses_four_field_levels() {
        let (okx, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"ts":"99","bids":[["100","1","0","2"]],
            "asks":[["101","2","0","1"]]}]}"#,
        );
        let book = okx.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 99);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101), dec!(2)));
    }

    #[test]
    fn error_code_maps_to_taxonomy() {
        let (okx, mock) = client();
        mock.push_json(200, r#"{"code":"51008","msg":"balance","data":[]}"#);
        assert!(matches!(
            okx.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn place_order_signs_with_ok_access_headers() {
        let (okx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"ordId":"312","clOrdId":"abc","sCode":"0","sMsg":""}]}"#,
        );
        let order = okx
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "312");
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
        let ts = header("OK-ACCESS-TIMESTAMP");
        assert_eq!(ts, "1970-01-01T00:00:00.000Z");
        let body = req.body.as_ref().unwrap();
        let prehash = format!("{ts}POST/api/v5/trade/order{body}");
        let expected = hmac_sha256_base64(b"SECRET", prehash.as_bytes());
        assert_eq!(header("OK-ACCESS-SIGN"), expected);
        assert_eq!(header("OK-ACCESS-KEY"), "APIKEY");
        assert_eq!(header("OK-ACCESS-PASSPHRASE"), "PASS");
    }

    #[test]
    fn place_order_rejection_surfaces_scode() {
        let (okx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"ordId":"","clOrdId":"","sCode":"51008","sMsg":"insufficient"}]}"#,
        );
        assert!(matches!(
            okx.place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn query_order_parses_state() {
        let (okx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"instId":"BTC-USDT","ordId":"312","clOrdId":"",
            "side":"sell","ordType":"market","state":"filled","sz":"2","accFillSz":"2",
            "px":"0","avgPx":"100"}]}"#,
        );
        let order = okx.query_order(&symbol(), "312").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.average_price, Some(dec!(100)));
        assert_eq!(order.price, None);
    }

    #[test]
    fn balances_parse() {
        let (okx, mock) = signed_client(0);
        mock.push_json(
            200,
            r#"{"code":"0","data":[{"details":[{"ccy":"USDT","availBal":"100.5","frozenBal":"25.5"}]}]}"#,
        );
        let bals = okx.balances().unwrap();
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].total(), dec!(126));
    }

    #[test]
    fn signed_requires_passphrase() {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        // Credentials without a passphrase.
        let okx = Okx::with_credentials(
            Box::new(ArcTransport(mock)),
            &opts,
            Credentials::new("k", "s"),
        );
        assert!(matches!(
            okx.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    fn signed_ws_client(now_ms: i64) -> (Okx, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let okx = Okx::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (okx, ws)
    }

    #[test]
    fn subscribe_user_data_logs_in_and_streams_orders_and_account() {
        let (mut okx, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(r#"{"event":"login","code":"0"}"#.to_string())),
            Ok(Some(r#"{"event":"subscribe"}"#.to_string())),
            Ok(Some(
                r#"{"arg":{"channel":"orders","instType":"SPOT"},"data":[{"instId":"BTC-USDT",
                "ordId":"55","clOrdId":"my","side":"buy","ordType":"limit","state":"filled",
                "sz":"1","accFillSz":"1","px":"100","avgPx":"100"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"arg":{"channel":"account"},"data":[{"details":[{"ccy":"USDT",
                "availBal":"900","frozenBal":"50"}]}]}"#
                    .to_string(),
            )),
        ]);
        okx.subscribe_user_data().unwrap();
        assert_eq!(
            ws.connected_urls()[0],
            "wss://ws.okx.com:8443/ws/v5/private"
        );
        assert!(ws.sent()[0].contains(r#""op":"login""#));
        assert!(ws.sent()[0].contains(r#""apiKey":"APIKEY""#));
        assert!(ws.sent()[0].contains(r#""sign""#));
        assert!(ws.sent()[1].contains(r#""channel":"orders""#));
        assert!(ws.sent()[1].contains(r#""channel":"account""#));

        let events = okx.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "55");
        assert_eq!(order.client_order_id.as_deref(), Some("my"));
        assert_eq!(order.symbol, symbol());
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.status, OrderStatus::Filled);
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
    fn subscribe_user_data_requires_a_passphrase() {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut okx = Okx::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            okx.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut okx, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![]);
        okx.subscribe_user_data().unwrap();
        okx.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == "ping"));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut okx, ws) = signed_ws_client(1_700_000_000_000);
        okx.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_a_fresh_login() {
        let (mut okx, ws) = signed_ws_client(1_700_000_000_000);
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        okx.subscribe_user_data().unwrap();

        let events = okx.poll_events();
        assert!(events.contains(&Event::Disconnected));
        assert!(events.contains(&Event::Reconnected));
        // Two private connections (initial + reconnect), each re-signing op:login.
        let login_frames = ws
            .sent()
            .into_iter()
            .filter(|f| f.contains(r#""op":"login""#))
            .count();
        assert_eq!(login_frames, 2);
        assert_eq!(ws.connected_urls().len(), 2);
        assert_eq!(
            ws.connected_urls()[1],
            "wss://ws.okx.com:8443/ws/v5/private"
        );
    }

    #[test]
    fn place_and_cancel_order_over_ws() {
        let (mut okx, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(r#"{"event":"login","code":"0"}"#.to_string())),
            Ok(Some(
                r#"{"id":"1700000000","op":"order","code":"0","msg":"","data":[{"ordId":"55",
                "clOrdId":"my","sCode":"0","sMsg":""}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"id":"1700000000","op":"cancel-order","code":"0","msg":"","data":[{"ordId":"55",
                "sCode":"0","sMsg":""}]}"#
                    .to_string(),
            )),
        ]);
        let order = okx
            .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "55");
        assert_eq!(order.client_order_id.as_deref(), Some("my"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(
            ws.connected_urls()[0],
            "wss://ws.okx.com:8443/ws/v5/private"
        );
        // The login frame is sent first, then the order request.
        assert!(ws.sent()[0].contains(r#""op":"login""#));
        assert!(ws.sent()[1].contains(r#""op":"order""#));
        assert!(ws.sent()[1].contains(r#""instId":"BTC-USDT""#));

        okx.cancel_order_ws(&symbol(), "55").unwrap();
        assert!(ws.sent()[2].contains(r#""op":"cancel-order""#));
        assert!(ws.sent()[2].contains(r#""ordId":"55""#));
    }

    #[test]
    fn ws_order_surfaces_rejection() {
        let (mut okx, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(r#"{"event":"login","code":"0"}"#.to_string())),
            Ok(Some(
                r#"{"id":"1000","op":"order","code":"1","msg":"","data":[{"ordId":"","clOrdId":"",
                "sCode":"51008","sMsg":"insufficient balance"}]}"#
                    .to_string(),
            )),
        ]);
        assert!(matches!(
            okx.place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn ws_order_requires_a_passphrase() {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut okx = Okx::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            okx.place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_subscribe_and_parse() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[
                {"instId":"BTC-USDT","px":"100","sz":"0.5","side":"buy","ts":"1"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"snapshot","data":[
                {"ts":"5","bids":[["100","1","0","1"]],"asks":[["101","2","0","1"]]}]}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"event":"subscribe"}"#.to_string())),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut okx = Okx::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        okx.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""channel":"trades""#));
        assert_eq!(ws.connected_urls()[0], "wss://ws.okx.com:8443/ws/v5/public");

        let events = okx.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert!(matches!(events[1], Event::BookSnapshot(_)));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (okx, mock) = signed_client(0);
        mock.push_json(200, r#"{"code":"0","data":[{"ordId":"9","sCode":"0"}]}"#);
        let mut exchange: Box<dyn Exchange> = Box::new(okx);
        assert_eq!(exchange.name(), "okx");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "9");
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }
}
