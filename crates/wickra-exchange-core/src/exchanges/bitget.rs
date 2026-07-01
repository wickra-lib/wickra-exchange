//! Bitget (v2 API) — the fourth exchange.
//!
//! Bitget's signing is close to OKX's — base64(HMAC-SHA256) with a passphrase —
//! but over a **millisecond** timestamp rather than ISO-8601, with `ACCESS-*`
//! headers, concatenated symbols (`BTCUSDT`) and a `{code, msg, data}` envelope
//! whose success code is the string `"00000"`.
//!
//! On a futures [`MarketType`](crate::MarketType), `query_order`/`cancel_order`/
//! `open_orders` route to the mix order endpoints (`/api/v2/mix/order/detail`,
//! `/cancel-order`, `/orders-pending`) with `productType=USDT-FUTURES` and the
//! mix order object (`state` instead of `status`, `entrustedList` wrapper).
//!
//! [`AdvancedOrders`]: STP via `stpMode` on order create and native batch
//! place/cancel — spot (`/api/v2/spot/trade/batch-orders`, `.../batch-cancel-order`)
//! or mix (`/api/v2/mix/order/batch-place-order`, `.../batch-cancel-orders`),
//! per-order results. Bitget has no in-place amend and no OCO order-list — both
//! documented gaps.

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
    TimeInForce,
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
    market_type: MarketType,
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
}

impl Bitget {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: "https://api.bitget.com".to_string(),
            market_type: options.market_type,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            private_connection: None,
            user_data_active: false,
        }
    }

    /// Whether this client targets a USDⓈ-M futures market (Bitget's `mix`
    /// product line, `USDT-FUTURES` / margin coin `USDT`) rather than spot.
    fn is_futures(&self) -> bool {
        self.market_type.is_derivatives()
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
        let (path, query) = if self.is_futures() {
            (
                "/api/v2/mix/market/ticker",
                format!(
                    "symbol={}&productType=USDT-FUTURES",
                    Self::wire_symbol(symbol)
                ),
            )
        } else {
            (
                "/api/v2/spot/market/tickers",
                format!("symbol={}", Self::wire_symbol(symbol)),
            )
        };
        let data = self.get(path, &query)?;
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
        let (path, extra) = if self.is_futures() {
            ("/api/v2/mix/market/candles", "&productType=USDT-FUTURES")
        } else {
            ("/api/v2/spot/market/candles", "")
        };
        let query = format!(
            "symbol={}&granularity={}&limit={limit}{extra}",
            Self::wire_symbol(symbol),
            map_granularity(interval),
        );
        let data = self.get(path, &query)?;
        let rows: Vec<Vec<String>> = parse_json(data)?;
        rows.iter().map(|row| parse_kline_row(row)).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let (path, query) = if self.is_futures() {
            (
                "/api/v2/mix/market/merge-depth",
                format!(
                    "symbol={}&productType=USDT-FUTURES&limit={depth}",
                    Self::wire_symbol(symbol)
                ),
            )
        } else {
            (
                "/api/v2/spot/market/orderbook",
                format!("symbol={}&limit={depth}", Self::wire_symbol(symbol)),
            )
        };
        let data = self.get(path, &query)?;
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
        let url = "wss://ws.bitget.com/v2/ws/public";
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Open the private user-data stream (`wss://ws.bitget.com/v2/ws/private`).
    /// Logs in with an `op:login` frame (sign = base64(HMAC-SHA256) over
    /// `<epochSeconds>GET/user/verify`), then subscribes to the `orders` and
    /// `account` channels. Afterwards [`poll_events`](Self::poll_events) also
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
    pub fn subscribe_user_data(&mut self) -> Result<()> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "user-data stream requires credentials",
        ))?;
        let passphrase = creds
            .passphrase
            .as_deref()
            .ok_or(Error::InvalidCredentials("Bitget requires a passphrase"))?;
        // Bitget WS login signs `<timestamp>GET/user/verify`, timestamp in seconds.
        let timestamp = (self.now_ms)() / 1000;
        let sign = hmac_sha256_base64(
            creds.api_secret.as_bytes(),
            format!("{timestamp}GET/user/verify").as_bytes(),
        );
        let login = format!(
            r#"{{"op":"login","args":[{{"apiKey":"{}","passphrase":"{passphrase}","timestamp":"{timestamp}","sign":"{sign}"}}]}}"#,
            creds.api_key
        );
        let subscribe = r#"{"op":"subscribe","args":[{"instType":"SPOT","channel":"orders","instId":"default"},{"instType":"SPOT","channel":"account","coin":"default"}]}"#.to_string();
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://ws.bitget.com/v2/ws/private")?;
        connection.send(&login)?;
        connection.send(&subscribe)?;
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send an application-level heartbeat (the `ping` text frame Bitget expects)
    /// on the private stream so it is not dropped for inactivity. A no-op before
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
        if let Some(mode) = stp_mode_str(request.stp) {
            body["stpMode"] = serde_json::json!(mode);
        }
        let path = if self.is_futures() {
            body["productType"] = serde_json::json!("USDT-FUTURES");
            body["marginMode"] = serde_json::json!("crossed");
            body["marginCoin"] = serde_json::json!("USDT");
            if request.reduce_only {
                body["reduceOnly"] = serde_json::json!("YES");
            }
            "/api/v2/mix/order/place-order"
        } else {
            "/api/v2/spot/trade/place-order"
        };
        let data = self.signed_request(HttpMethod::Post, path, "", &body.to_string())?;
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
        if self.is_futures() {
            let body = serde_json::json!({
                "symbol": Self::wire_symbol(symbol),
                "productType": "USDT-FUTURES",
                "marginCoin": "USDT",
                "orderId": order_id,
            });
            self.signed_request(
                HttpMethod::Post,
                "/api/v2/mix/order/cancel-order",
                "",
                &body.to_string(),
            )?;
            return Ok(());
        }
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
        if self.is_futures() {
            let query = format!(
                "symbol={}&productType=USDT-FUTURES&orderId={order_id}",
                Self::wire_symbol(symbol)
            );
            // The mix order-detail endpoint returns a single order object.
            let data =
                self.signed_request(HttpMethod::Get, "/api/v2/mix/order/detail", &query, "")?;
            let raw: RawOrder = parse_json(data)?;
            return order_from_raw(symbol.clone(), &raw);
        }
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
        if self.is_futures() {
            let mut query = "productType=USDT-FUTURES".to_string();
            if let Some(s) = symbol {
                query.push_str("&symbol=");
                query.push_str(&Self::wire_symbol(s));
            }
            let data = self.signed_request(
                HttpMethod::Get,
                "/api/v2/mix/order/orders-pending",
                &query,
                "",
            )?;
            // The mix pending endpoint wraps the orders in an `entrustedList`.
            let pending: RawMixPending = parse_json(data)?;
            return pending
                .entrusted_list
                .iter()
                .map(|raw| {
                    let sym = symbol
                        .cloned()
                        .unwrap_or_else(|| split_wire_symbol(&raw.symbol));
                    order_from_raw(sym, raw)
                })
                .collect();
        }
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
        if self.is_futures() {
            let data = self.signed_request(
                HttpMethod::Get,
                "/api/v2/mix/account/accounts",
                "productType=USDT-FUTURES",
                "",
            )?;
            let list: Vec<RawMixAccount> = parse_json(data)?;
            return Ok(list
                .iter()
                .map(|a| Balance {
                    asset: a.margin_coin.clone(),
                    free: dec_or_zero(&a.available),
                    locked: dec_or_zero(&a.locked),
                })
                .collect());
        }
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

/// The Bitget `stpMode` value for a self-trade-prevention policy, or `None` to omit.
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
        "live" | "new" | "init" => Ok(OrderStatus::New),
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
    } else if channel == "orders" {
        // Private order channel: each element carries its own `instId`, which
        // `RawOrder` picks up as the wire symbol via the `instId` alias.
        data.iter()
            .map(|raw| {
                let order: RawOrder = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                let order_symbol = split_wire_symbol(&order.symbol);
                Ok(Event::OrderUpdate(order_from_raw(order_symbol, &order)?))
            })
            .collect()
    } else if channel == "account" {
        // Private account channel: `data` is an array of per-coin balances,
        // emitted together as one balance-update snapshot.
        let balances = data
            .iter()
            .map(|raw| {
                let asset: RawAsset = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                Ok(Balance {
                    asset: asset.coin,
                    free: dec_or_zero(&asset.available),
                    locked: dec_or_zero(&asset.frozen),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(vec![Event::BalanceUpdate(balances)])
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
    // REST reports `symbol`; the private `orders` stream reports `instId`.
    #[serde(default, alias = "instId")]
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: String,
    side: String,
    #[serde(rename = "orderType")]
    order_type: String,
    // Spot reports `status`; the mix (futures) order object reports `state`.
    #[serde(alias = "state")]
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

/// The mix (futures) pending-orders response wraps the orders in `entrustedList`.
#[derive(Deserialize)]
struct RawMixPending {
    #[serde(rename = "entrustedList", default)]
    entrusted_list: Vec<RawOrder>,
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

impl Bitget {
    /// Open positions on the USDⓈ-M futures account (`/api/v2/mix/position/all-position`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let data = self.signed_request(
            HttpMethod::Get,
            "/api/v2/mix/position/all-position",
            "productType=USDT-FUTURES&marginCoin=USDT",
            "",
        )?;
        let list: Vec<RawBitgetPosition> = parse_json(data)?;
        list.iter()
            .filter(|p| symbol.is_none_or(|s| p.symbol == Self::wire_symbol(s)))
            .filter_map(parse_bitget_position)
            .collect()
    }

    /// Set the leverage for `symbol` (`/api/v2/mix/account/set-leverage`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let body = serde_json::json!({
            "symbol": Self::wire_symbol(symbol),
            "productType": "USDT-FUTURES",
            "marginCoin": "USDT",
            "leverage": leverage.to_string(),
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v2/mix/account/set-leverage",
            "",
            &body.to_string(),
        )?;
        Ok(())
    }

    /// Set the margin mode for `symbol` (`/api/v2/mix/account/set-margin-mode`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let margin = match mode {
            MarginMode::Cross => "crossed",
            MarginMode::Isolated => "isolated",
        };
        let body = serde_json::json!({
            "symbol": Self::wire_symbol(symbol),
            "productType": "USDT-FUTURES",
            "marginCoin": "USDT",
            "marginMode": margin,
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v2/mix/account/set-margin-mode",
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

    /// Place several orders on one symbol in one request
    /// (`/api/v2/spot/trade/batch-orders`). Bitget returns separate success and
    /// failure lists, so a synthetic `clientOid` per index re-aligns them to each
    /// request's own [`Result`]. Batch requires a single symbol (that of the
    /// first request).
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails, or if called on a
    /// futures client (mix batch is a documented follow-up).
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let wire = Self::wire_symbol(&requests[0].symbol);
        let order_list: Vec<serde_json::Value> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let coid = batch_client_oid(r, i);
                let mut o = serde_json::json!({
                    "side": side_str(r.side),
                    "orderType": order_type_str(r.order_type),
                    "force": force_str(r.time_in_force),
                    "size": format_decimal(r.quantity),
                    "clientOid": coid,
                });
                if let Some(price) = r.price {
                    o["price"] = serde_json::json!(format_decimal(price));
                }
                o
            })
            .collect();
        let (path, body) = if self.is_futures() {
            (
                "/api/v2/mix/order/batch-place-order",
                serde_json::json!({
                    "symbol": wire,
                    "productType": "USDT-FUTURES",
                    "marginMode": "crossed",
                    "marginCoin": "USDT",
                    "orderList": order_list,
                }),
            )
        } else {
            (
                "/api/v2/spot/trade/batch-orders",
                serde_json::json!({ "symbol": wire, "orderList": order_list }),
            )
        };
        let data = self.signed_request(HttpMethod::Post, path, "", &body.to_string())?;
        let result: BatchOrdersResult = parse_json(data)?;
        let ok_map: HashMap<String, String> = result
            .success_list
            .into_iter()
            .map(|s| (s.client_oid, s.order_id))
            .collect();
        let fail_map: HashMap<String, String> = result
            .failure_list
            .into_iter()
            .map(|f| (f.client_oid, f.error_msg))
            .collect();
        Ok(requests
            .iter()
            .enumerate()
            .map(|(i, req)| {
                let coid = batch_client_oid(req, i);
                if let Some(order_id) = ok_map.get(&coid) {
                    Ok(Order {
                        id: order_id.clone(),
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
                } else {
                    Err(Error::OrderRejected {
                        code: "batch".to_string(),
                        message: fail_map
                            .get(&coid)
                            .cloned()
                            .unwrap_or_else(|| "order rejected in batch".to_string()),
                    })
                }
            })
            .collect())
    }

    /// Cancel several orders on one `symbol` in one request
    /// (`/api/v2/spot/trade/batch-cancel-order`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails, or if called on a futures client.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        let id_list: Vec<serde_json::Value> = order_ids
            .iter()
            .map(|id| serde_json::json!({ "orderId": id }))
            .collect();
        let (path, body) = if self.is_futures() {
            (
                "/api/v2/mix/order/batch-cancel-orders",
                serde_json::json!({
                    "symbol": wire,
                    "productType": "USDT-FUTURES",
                    "marginCoin": "USDT",
                    "orderIdList": id_list,
                }),
            )
        } else {
            (
                "/api/v2/spot/trade/batch-cancel-order",
                serde_json::json!({ "symbol": wire, "orderList": id_list }),
            )
        };
        self.signed_request(HttpMethod::Post, path, "", &body.to_string())?;
        Ok(())
    }
}

impl AdvancedOrders for Bitget {
    /// Bitget spot has no in-place amend (orders are cancelled and re-placed),
    /// so this returns an [`Error::Exchange`].
    fn amend_order(
        &mut self,
        _symbol: &Symbol,
        _order_id: &str,
        _new_price: Option<Decimal>,
        _new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Bitget has no in-place amend; cancel and re-place the order".to_string(),
        })
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Bitget::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Bitget::cancel_batch(self, symbol, order_ids)
    }
    /// Bitget has no OCO order-list (it uses standalone plan/trigger orders), so
    /// this returns an [`Error::Exchange`].
    fn place_oco(&mut self, _request: &OcoRequest) -> Result<Vec<Order>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Bitget has no OCO order-list; use plan/trigger orders".to_string(),
        })
    }
}

/// The client order id used to align a batch element to its request: the
/// caller's `clientOid` if set, else a synthetic index-based one.
fn batch_client_oid(request: &OrderRequest, index: usize) -> String {
    request
        .client_order_id
        .clone()
        .unwrap_or_else(|| format!("wbatch-{index}"))
}

#[derive(Deserialize)]
struct BatchOrdersResult {
    #[serde(rename = "successList", default)]
    success_list: Vec<BatchSuccess>,
    #[serde(rename = "failureList", default)]
    failure_list: Vec<BatchFailure>,
}

#[derive(Deserialize)]
struct BatchSuccess {
    #[serde(rename = "orderId", default)]
    order_id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: String,
}

#[derive(Deserialize)]
struct BatchFailure {
    #[serde(rename = "clientOid", default)]
    client_oid: String,
    #[serde(rename = "errorMsg", default)]
    error_msg: String,
}

impl Exchange for Bitget {
    fn name(&self) -> &'static str {
        "bitget"
    }
}

impl WsUserData for Bitget {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Bitget::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Bitget::keepalive_user_data(self)
    }
}

impl WsExecution for Bitget {
    /// Bitget exposes no public WebSocket order-entry API — its WebSocket surface
    /// is subscription-only (market data plus the private order/account push
    /// channels). Orders are placed over REST, so this returns a documented
    /// [`Error::Exchange`].
    fn place_order_ws(&mut self, _request: &OrderRequest) -> Result<Order> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Bitget has no WebSocket order-entry API; place orders over \
                      REST (POST /api/v2/spot/trade/place-order)"
                .to_string(),
        })
    }

    fn cancel_order_ws(&mut self, _symbol: &Symbol, _order_id: &str) -> Result<()> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Bitget has no WebSocket order-entry API; cancel orders over \
                      REST (POST /api/v2/spot/trade/cancel-order)"
                .to_string(),
        })
    }
}

impl Derivatives for Bitget {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Bitget::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Bitget::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Bitget::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Bitget::close_position(self, symbol)
    }
}

#[derive(Deserialize)]
struct RawMixAccount {
    #[serde(rename = "marginCoin")]
    margin_coin: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    locked: String,
}

#[derive(Deserialize)]
struct RawBitgetPosition {
    symbol: String,
    #[serde(rename = "holdSide")]
    hold_side: String,
    total: String,
    #[serde(rename = "openPriceAvg", default)]
    open_price_avg: String,
    #[serde(rename = "markPrice", default)]
    mark_price: String,
    leverage: String,
    #[serde(rename = "unrealizedPL", default)]
    unrealized_pl: String,
    #[serde(rename = "marginMode", default)]
    margin_mode: String,
}

fn parse_bitget_position(raw: &RawBitgetPosition) -> Option<Result<Position>> {
    let total = match parse_decimal(&raw.total) {
        Ok(total) if !total.is_zero() => total,
        Ok(_) => return None, // flat position
        Err(e) => return Some(Err(e)),
    };
    let side = if raw.hold_side == "short" {
        PositionSide::Short
    } else {
        PositionSide::Long
    };
    let build = || -> Result<Position> {
        Ok(Position {
            symbol: split_wire_symbol(&raw.symbol),
            side,
            quantity: total,
            entry_price: parse_decimal(&raw.open_price_avg)?,
            mark_price: parse_decimal(&raw.mark_price)?,
            leverage: parse_decimal(&raw.leverage)?,
            unrealized_pnl: parse_decimal(&raw.unrealized_pl)?,
            margin_mode: if raw.margin_mode.eq_ignore_ascii_case("isolated") {
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

    fn signed_ws_client(now_ms: i64) -> (Bitget, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let bitget = Bitget::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (bitget, ws)
    }

    #[test]
    fn subscribe_user_data_logs_in_and_streams_orders_and_account() {
        let (mut bitget, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![
            Ok(Some(r#"{"event":"login","code":"0"}"#.to_string())),
            Ok(Some(r#"{"event":"subscribe"}"#.to_string())),
            Ok(Some(
                r#"{"arg":{"instType":"SPOT","channel":"orders","instId":"default"},"data":[
                {"instId":"BTCUSDT","orderId":"55","clientOid":"my","side":"buy","orderType":"limit",
                "status":"filled","size":"1","baseVolume":"1","price":"100","priceAvg":"100"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"arg":{"instType":"SPOT","channel":"account","coin":"default"},"data":[
                {"coin":"USDT","available":"900","frozen":"50"}]}"#
                    .to_string(),
            )),
        ]);
        bitget.subscribe_user_data().unwrap();
        assert_eq!(ws.connected_urls()[0], "wss://ws.bitget.com/v2/ws/private");
        assert!(ws.sent()[0].contains(r#""op":"login""#));
        assert!(ws.sent()[0].contains(r#""apiKey":"APIKEY""#));
        assert!(ws.sent()[1].contains(r#""channel":"orders""#));
        assert!(ws.sent()[1].contains(r#""channel":"account""#));

        let events = bitget.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "55");
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
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut bitget = Bitget::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            bitget.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut bitget, ws) = signed_ws_client(1_700_000_000_000);
        ws.push_connection(vec![]);
        bitget.subscribe_user_data().unwrap();
        bitget.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == "ping"));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut bitget, ws) = signed_ws_client(1_700_000_000_000);
        bitget.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_a_fresh_login() {
        let (mut bitget, ws) = signed_ws_client(1_700_000_000_000);
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        bitget.subscribe_user_data().unwrap();

        let events = bitget.poll_events();
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
        assert_eq!(ws.connected_urls()[1], "wss://ws.bitget.com/v2/ws/private");
    }

    #[test]
    fn ws_execution_is_a_documented_gap() {
        // Bitget has no WebSocket order-entry API; the trait methods return a
        // documented error rather than faking a round trip.
        let (mut bitget, _mock) = client();
        assert!(matches!(
            bitget
                .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::Exchange { .. }
        ));
        assert!(matches!(
            bitget.cancel_order_ws(&symbol(), "1").unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    fn signed_futures_client(now_ms: i64) -> (Bitget, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let bitget = Bitget::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (bitget, mock)
    }

    const MIX_POSITIONS: &str = r#"{"code":"00000","msg":"success","data":[
        {"symbol":"BTCUSDT","holdSide":"long","total":"0.5","openPriceAvg":"20000","markPrice":"20100","leverage":"10","unrealizedPL":"50","marginMode":"isolated"}
    ]}"#;

    #[test]
    fn stp_maps_to_stp_mode() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"orderId":"1","clientOid":""}}"#,
        );
        bitget
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .with_stp(SelfTradePrevention::ExpireBoth),
            )
            .unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[0]
            .body
            .as_ref()
            .unwrap()
            .contains(r#""stpMode":"cancel_both""#));
    }

    #[test]
    fn place_batch_aligns_success_and_failure() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{
            "successList":[{"orderId":"o1","clientOid":"wbatch-0"}],
            "failureList":[{"clientOid":"wbatch-1","errorMsg":"insufficient"}]}}"#,
        );
        let results = bitget
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
        assert!(reqs[0].url.contains("/api/v2/spot/trade/batch-orders"));
        assert!(reqs[0]
            .body
            .as_ref()
            .unwrap()
            .contains(r#""clientOid":"wbatch-0""#));
    }

    #[test]
    fn cancel_batch_is_one_call() {
        let (bitget, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"00000","data":{"successList":[]}}"#);
        bitget
            .cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0]
            .url
            .contains("/api/v2/spot/trade/batch-cancel-order"));
    }

    #[test]
    fn amend_and_oco_are_unsupported() {
        let (mut bitget, _mock) = signed_client(1000);
        assert!(matches!(
            AdvancedOrders::amend_order(&mut bitget, &symbol(), "1", Some(dec!(1)), None)
                .unwrap_err(),
            Error::Exchange { .. }
        ));
        assert!(matches!(
            AdvancedOrders::place_oco(
                &mut bitget,
                &OcoRequest::new(symbol(), OrderSide::Sell, dec!(1), dec!(110), dec!(95))
            )
            .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn futures_query_order_uses_mix_detail_and_state() {
        let (bitget, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"symbol":"BTCUSDT","orderId":"88","clientOid":"",
            "side":"buy","orderType":"limit","state":"filled","size":"2","baseVolume":"2",
            "price":"100","priceAvg":"100"}}"#,
        );
        let order = bitget.query_order(&symbol(), "88").unwrap();
        assert_eq!(order.id, "88");
        assert_eq!(order.status, OrderStatus::Filled);
        let url = &mock.recorded_requests()[0].url;
        assert!(url.contains("/api/v2/mix/order/detail"));
        assert!(url.contains("productType=USDT-FUTURES"));
    }

    #[test]
    fn futures_cancel_and_open_orders_use_mix_endpoints() {
        let (bitget, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"code":"00000","data":{"orderId":"88"}}"#);
        bitget.cancel_order(&symbol(), "88").unwrap();
        mock.push_json(
            200,
            r#"{"code":"00000","data":{"entrustedList":[{"symbol":"BTCUSDT","orderId":"90",
            "clientOid":"","side":"sell","orderType":"limit","state":"live","size":"3",
            "baseVolume":"0","price":"21000","priceAvg":"0"}]}}"#,
        );
        let orders = bitget.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/api/v2/mix/order/cancel-order"));
        assert!(reqs[1].url.contains("/api/v2/mix/order/orders-pending"));
    }

    #[test]
    fn futures_place_batch_uses_mix_endpoint() {
        let (bitget, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","data":{
            "successList":[{"orderId":"o1","clientOid":"wbatch-0"}],
            "failureList":[]}}"#,
        );
        let results = bitget
            .place_batch(&[OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))])
            .unwrap();
        assert_eq!(results[0].as_ref().unwrap().id, "o1");
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v2/mix/order/batch-place-order"));
        assert!(req
            .body
            .as_ref()
            .unwrap()
            .contains(r#""productType":"USDT-FUTURES""#));
    }

    #[test]
    fn futures_ticker_uses_mix_path() {
        let (bitget, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","msg":"success","data":[{"symbol":"BTCUSDT","lastPr":"20000","bidPr":"19999","askPr":"20001","baseVolume":"1000"}]}"#,
        );
        let ticker = bitget.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        let url = &mock.recorded_requests()[0].url;
        assert!(url.contains("/api/v2/mix/market/ticker"));
        assert!(url.contains("productType=USDT-FUTURES"));
    }

    #[test]
    fn futures_place_order_uses_mix_path() {
        let (bitget, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"code":"00000","msg":"success","data":{"orderId":"9","clientOid":""}}"#,
        );
        bitget
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(20000)))
            .unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v2/mix/order/place-order"));
        assert!(req.body.as_deref().unwrap().contains("USDT-FUTURES"));
    }

    #[test]
    fn derivatives_positions_parse() {
        let (mut bitget, mock) = signed_futures_client(1000);
        mock.push_json(200, MIX_POSITIONS);
        let positions = Derivatives::positions(&mut bitget, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(0.5));
        assert_eq!(positions[0].leverage, dec!(10));
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v2/mix/position/all-position"));
    }

    #[test]
    fn derivatives_set_leverage_hits_endpoint() {
        let (mut bitget, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"code":"00000","msg":"success","data":{}}"#);
        Derivatives::set_leverage(&mut bitget, &symbol(), 20).unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v2/mix/account/set-leverage"));
        assert!(req.body.as_deref().unwrap().contains(r#""leverage":"20""#));
    }

    #[test]
    fn derivatives_close_position_reduce_only() {
        let (mut bitget, mock) = signed_futures_client(1000);
        mock.push_json(200, MIX_POSITIONS);
        mock.push_json(
            200,
            r#"{"code":"00000","msg":"success","data":{"orderId":"9","clientOid":""}}"#,
        );
        Derivatives::close_position(&mut bitget, &symbol()).unwrap();
        let req = &mock.recorded_requests()[1];
        assert!(req.url.contains("/api/v2/mix/order/place-order"));
        let body = req.body.as_deref().unwrap();
        assert!(body.contains(r#""side":"sell""#));
        assert!(body.contains(r#""reduceOnly":"YES""#));
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
