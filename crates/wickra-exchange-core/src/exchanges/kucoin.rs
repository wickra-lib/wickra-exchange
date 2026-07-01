//! KuCoin (v1/v2 API) — the fifth exchange.
//!
//! KuCoin signs with base64(HMAC-SHA256) over `msTimestamp + METHOD + endpoint +
//! body` under `KC-API-*` headers, and — unlike OKX/Bitget — the passphrase
//! itself is HMAC-signed (`KC-API-PASSPHRASE = base64(HMAC-SHA256(secret,
//! passphrase))`, key version 2). Symbols are dash-form (`BTC-USDT`); the
//! envelope success code is the string `"200000"`. KuCoin candles are
//! newest-first and ordered `[time, open, close, high, low, volume, turnover]`.
//!
//! [`AdvancedOrders`]: STP via the `stp` flag on order create, native spot batch
//! place (`/api/v1/orders/multi`, per-order results), native OCO
//! (`/api/v3/oco/order`, returned as one order-list). KuCoin has no
//! batch-cancel-by-id (so `cancel_batch` cancels sequentially) and no in-place
//! amend (`amend_order` is a documented gap).

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType, SelfTradePrevention};
use crate::positions::{Position, PositionSide};
use crate::signing::hmac_sha256_base64;
use crate::symbol::Symbol;
use crate::traits::{AdvancedOrders, Derivatives, Exchange, Execution, MarketData, WsUserData};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{
    Balance, OcoRequest, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker,
};
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

/// A KuCoin client over injected transports.
pub struct KuCoin {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    market_type: MarketType,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    /// Leverage applied to futures orders. KuCoin sets leverage per order rather
    /// than per account, so [`set_leverage`](Self::set_leverage) records it here.
    leverage: u32,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
}

impl KuCoin {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        let futures = options.market_type.is_derivatives();
        Self {
            http,
            ws: None,
            rest_base: if futures {
                "https://api-futures.kucoin.com".to_string()
            } else {
                "https://api.kucoin.com".to_string()
            },
            market_type: options.market_type,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            leverage: 1,
            private_connection: None,
        }
    }

    /// Whether this client targets KuCoin Futures (a separate host and API with
    /// contract symbols like `XBTUSDTM`) rather than spot.
    fn is_futures(&self) -> bool {
        self.market_type.is_derivatives()
    }

    /// The KuCoin **futures** contract symbol for a canonical [`Symbol`]:
    /// `BTC/USDT` -> `XBTUSDTM` (KuCoin uses `XBT` for Bitcoin and a trailing
    /// `M` for the USDⓈ-M perpetual).
    fn futures_symbol(symbol: &Symbol) -> String {
        let base = if symbol.base() == "BTC" {
            "XBT"
        } else {
            symbol.base()
        };
        format!("{base}{}M", symbol.quote())
    }

    /// Build a public KuCoin client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated KuCoin client (credentials must carry a passphrase).
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

    /// The KuCoin wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTC-USDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}-{}", symbol.base(), symbol.quote())
    }

    /// A ticker for `symbol` (from the 24-hour stats endpoint).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        let data = self.get("/api/v1/market/stats", &query)?;
        let raw: RawStats = parse_json(data)?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&raw.last)?,
            bid: parse_decimal(&raw.buy)?,
            ask: parse_decimal(&raw.sell)?,
            volume: parse_decimal(&raw.vol)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). KuCoin returns
    /// newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, _limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "symbol={}&type={}",
            Self::wire_symbol(symbol),
            map_type(interval),
        );
        let data = self.get("/api/v1/market/candles", &query)?;
        let rows: Vec<Vec<String>> = parse_json(data)?;
        let mut candles = rows
            .iter()
            .map(|row| parse_kline_row(row))
            .collect::<Result<Vec<_>>>()?;
        candles.reverse();
        Ok(candles)
    }

    /// A depth snapshot of `symbol` (top 20 levels per side).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, _depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        let data = self.get("/api/v1/market/orderbook/level2_20", &query)?;
        let raw: RawDepth = parse_json(data)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.sequence.parse().unwrap_or(0),
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "/market/match")
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "/market/level2")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "/market/ticker")
    }

    fn subscribe(&mut self, symbol: &Symbol, topic_prefix: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            // The bullet-token negotiation and instance endpoint are handled by
            // the real transport adapter; the module only produces the topics.
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://ws-api-spot.kucoin.com/")?;
            self.connection = Some(connection);
        }
        let id = (self.now_ms)();
        let message = format!(
            r#"{{"id":"{id}","type":"subscribe","topic":"{topic_prefix}:{wire}","response":true}}"#
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
                .unwrap_or_else(|| wire.parse().unwrap_or_else(|_| Symbol::new(wire, "")))
        };
        let mut events = Vec::new();
        if let Some(connection) = self.connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(Some(event)) = parse_ws_message(&frame, &resolve) {
                    events.push(event);
                }
            }
        }
        // Drain the private user-data stream (tradeOrders/account.balance), if open.
        // Re-negotiating a fresh bullet token on reconnect is a keepalive follow-up.
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(Some(event)) = parse_ws_message(&frame, &resolve) {
                    events.push(event);
                }
            }
        }
        let url = "wss://ws-api-spot.kucoin.com/";
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Open the private user-data stream. Negotiates a bullet-private token over
    /// REST (`POST /api/v1/bullet-private`, signed), connects the returned
    /// instance endpoint (`<endpoint>?token=<token>&connectId=<id>`), then
    /// subscribes to the `/spotMarket/tradeOrders` and `/account/balance` private
    /// channels. Afterwards [`poll_events`](Self::poll_events) also surfaces the
    /// account's own [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// The bullet token expires (KuCoin pings keep it alive); re-negotiating it on
    /// reconnect is a keepalive follow-up.
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials or a passphrase,
    /// [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the token negotiation or subscription fails.
    pub fn subscribe_user_data(&mut self) -> Result<()> {
        let data = self.signed_request(HttpMethod::Post, "/api/v1/bullet-private", "", "")?;
        let bullet: BulletToken = parse_json(data)?;
        let server = bullet.instance_servers.into_iter().next().ok_or_else(|| {
            Error::NotFound("bullet-private returned no instance server".to_string())
        })?;
        let connect_id = (self.now_ms)();
        let url = format!(
            "{}?token={}&connectId={connect_id}",
            server.endpoint, bullet.token
        );
        let orders = format!(
            r#"{{"id":"{connect_id}","type":"subscribe","topic":"/spotMarket/tradeOrders","privateChannel":true,"response":true}}"#
        );
        let account = format!(
            r#"{{"id":"{}","type":"subscribe","topic":"/account/balance","privateChannel":true,"response":true}}"#,
            connect_id + 1
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect(&url)?;
        connection.send(&orders)?;
        connection.send(&account)?;
        self.private_connection = Some(connection);
        Ok(())
    }

    /// Place an order. KuCoin requires a client order id; one is generated from
    /// the clock when the request does not supply it.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let client_oid = request
            .client_order_id
            .clone()
            .unwrap_or_else(|| format!("wkex-{}", (self.now_ms)()));
        let symbol_wire = if self.is_futures() {
            Self::futures_symbol(&request.symbol)
        } else {
            Self::wire_symbol(&request.symbol)
        };
        let mut body = serde_json::json!({
            "clientOid": client_oid,
            "side": side_str(request.side),
            "symbol": symbol_wire,
            "type": order_type_str(request.order_type),
            "size": format_decimal(request.quantity),
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if request.post_only {
            body["postOnly"] = serde_json::json!(true);
        }
        if let Some(stp) = stp_flag(request.stp) {
            body["stp"] = serde_json::json!(stp);
        }
        if self.is_futures() {
            // Futures orders carry the per-order leverage; size is in contracts.
            body["leverage"] = serde_json::json!(self.leverage.to_string());
            if request.reduce_only {
                body["reduceOnly"] = serde_json::json!(true);
            }
        }
        let data =
            self.signed_request(HttpMethod::Post, "/api/v1/orders", "", &body.to_string())?;
        let placed: PlaceResult = parse_json(data)?;
        Ok(Order {
            id: placed.order_id,
            client_order_id: Some(client_oid),
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
        let path = format!("/api/v1/orders/{order_id}");
        self.signed_request(HttpMethod::Delete, &path, "", "")?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let path = format!("/api/v1/orders/{order_id}");
        let data = self.signed_request(HttpMethod::Get, &path, "", "")?;
        let raw: RawOrder = parse_json(data)?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// All open orders, optionally filtered to one `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let mut query = "status=active".to_string();
        if let Some(s) = symbol {
            query.push_str("&symbol=");
            query.push_str(&Self::wire_symbol(s));
        }
        let data = self.signed_request(HttpMethod::Get, "/api/v1/orders", &query, "")?;
        let page: OrderPage = parse_json(data)?;
        page.items
            .iter()
            .map(|raw| {
                let sym = symbol.cloned().unwrap_or_else(|| {
                    raw.symbol
                        .parse()
                        .unwrap_or_else(|_| Symbol::new(&raw.symbol, ""))
                });
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Trade-account balances.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let data = self.signed_request(HttpMethod::Get, "/api/v1/accounts", "type=trade", "")?;
        let list: Vec<RawAccount> = parse_json(data)?;
        Ok(list
            .iter()
            .map(|a| Balance {
                asset: a.currency.clone(),
                free: dec_or_zero(&a.available),
                locked: dec_or_zero(&a.holds),
            })
            .collect())
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_envelope(&response.body)
    }

    /// Sign with the `KC-API-*` headers (key version 2, HMAC-signed passphrase).
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
            .ok_or(Error::InvalidCredentials("KuCoin requires a passphrase"))?;
        let timestamp = (self.now_ms)().to_string();
        let endpoint = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let prehash = format!("{timestamp}{}{endpoint}{body}", method.as_str());
        let signature = hmac_sha256_base64(creds.api_secret.as_bytes(), prehash.as_bytes());
        let signed_passphrase =
            hmac_sha256_base64(creds.api_secret.as_bytes(), passphrase.as_bytes());
        let url = format!("{}{endpoint}", self.rest_base);
        let mut request = HttpRequest::new(method, url)
            .with_header("KC-API-KEY", creds.api_key.clone())
            .with_header("KC-API-SIGN", signature)
            .with_header("KC-API-TIMESTAMP", timestamp)
            .with_header("KC-API-PASSPHRASE", signed_passphrase)
            .with_header("KC-API-KEY-VERSION", "2");
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_envelope(&response.body)
    }
}

fn map_type(interval: &str) -> String {
    match interval {
        "1m" => "1min",
        "3m" => "3min",
        "5m" => "5min",
        "15m" => "15min",
        "30m" => "30min",
        "1h" => "1hour",
        "2h" => "2hour",
        "4h" => "4hour",
        "6h" => "6hour",
        "12h" => "12hour",
        "1d" => "1day",
        "1w" => "1week",
        other => other,
    }
    .to_string()
}

fn unwrap_envelope(body: &str) -> Result<serde_json::Value> {
    let envelope: Envelope =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if envelope.code != "200000" {
        return Err(map_error(&envelope.code, &envelope.msg));
    }
    Ok(envelope.data)
}

fn parse_json<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn map_error(code: &str, msg: &str) -> Error {
    match code {
        "429000" => Error::RateLimited { retry_after: None },
        "400003" | "400004" | "400005" | "400006" | "400007" => Error::Auth(msg.to_string()),
        "200004" | "400100" => Error::InsufficientBalance,
        "400600" | "404000" => Error::NotFound(msg.to_string()),
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

/// The KuCoin `stp` flag for a self-trade-prevention policy, or `None` to omit.
/// KuCoin cancels the oldest (`CO`), newest (`CN`) or both (`CB`) order.
fn stp_flag(stp: SelfTradePrevention) -> Option<&'static str> {
    match stp {
        SelfTradePrevention::None => None,
        SelfTradePrevention::ExpireMaker => Some("CO"),
        SelfTradePrevention::ExpireTaker => Some("CN"),
        SelfTradePrevention::ExpireBoth => Some("CB"),
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
    // KuCoin candle: [time, open, close, high, low, volume, turnover].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let ts = row[0]
        .parse::<i64>()
        .map_err(|e| Error::Deserialization(format!("kline time not an integer: {e}")))?;
    let f = |i: usize| -> Result<f64> {
        row[i]
            .parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
    };
    // Note the KuCoin field order: open, close, high, low.
    Candle::new(f(1)?, f(3)?, f(4)?, f(2)?, f(5)?, ts)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    let status = if raw.is_active {
        if dec_or_zero(&raw.deal_size) > Decimal::ZERO {
            OrderStatus::PartiallyFilled
        } else {
            OrderStatus::New
        }
    } else if raw.cancel_exist {
        OrderStatus::Canceled
    } else {
        OrderStatus::Filled
    };
    Ok(Order {
        id: raw.id.clone(),
        client_order_id: (!raw.client_oid.is_empty()).then(|| raw.client_oid.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status,
        quantity: parse_decimal(&raw.size)?,
        filled_quantity: dec_or_zero(&raw.deal_size),
        price: nonzero_decimal(&raw.price),
        average_price: None,
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

/// Read a `u64` that a venue may encode as either a JSON number or a string.
fn opt_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|v| {
            v.as_u64()
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
    if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
        return Ok(None); // ack/welcome/pong
    }
    let Some(topic) = value.get("topic").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let wire = topic.rsplit(':').next().unwrap_or("");
    let symbol = resolve(wire);
    let null = serde_json::Value::Null;
    let data = value.get("data").unwrap_or(&null);

    if topic.starts_with("/market/match:") {
        Ok(Some(Event::Trade(TradePrint {
            symbol,
            price: parse_decimal(field_str(data, "price")?)?,
            quantity: parse_decimal(field_str(data, "size")?)?,
            aggressor: parse_side(field_str(data, "side")?)?,
            timestamp: opt_str(data, "time").parse().unwrap_or(0),
        })))
    } else if topic.starts_with("/market/ticker:") {
        Ok(Some(Event::Ticker(Ticker {
            symbol,
            last: parse_decimal(field_str(data, "price")?)?,
            bid: dec_or_zero(opt_str(data, "bestBid")),
            ask: dec_or_zero(opt_str(data, "bestAsk")),
            volume: dec_or_zero(opt_str(data, "size")),
        })))
    } else if topic.starts_with("/market/level2:") {
        let changes = data.get("changes");
        let bids = parse_ws_levels(changes.and_then(|c| c.get("bids")))?;
        let asks = parse_ws_levels(changes.and_then(|c| c.get("asks")))?;
        let update_id = opt_u64(data, "sequenceEnd");
        Ok(Some(Event::BookDelta(BookDelta {
            symbol,
            first_update_id: update_id,
            final_update_id: update_id,
            bids,
            asks,
        })))
    } else if topic == "/spotMarket/tradeOrders" {
        Ok(Some(Event::OrderUpdate(ws_order_from_data(data)?)))
    } else if topic == "/account/balance" {
        Ok(Some(Event::BalanceUpdate(vec![Balance {
            asset: field_str(data, "currency")?.to_string(),
            free: dec_or_zero(opt_str(data, "available")),
            locked: dec_or_zero(opt_str(data, "hold")),
        }])))
    } else {
        Ok(None)
    }
}

/// Map a `/spotMarket/tradeOrders` status (with the event `type` disambiguating a
/// finished order) to an [`OrderStatus`].
fn ws_order_status(status: &str, event_type: &str) -> OrderStatus {
    match status {
        "match" => OrderStatus::PartiallyFilled,
        "done" if event_type == "canceled" => OrderStatus::Canceled,
        "done" => OrderStatus::Filled,
        _ => OrderStatus::New,
    }
}

/// Build an [`Order`] from a `/spotMarket/tradeOrders` message payload.
fn ws_order_from_data(data: &serde_json::Value) -> Result<Order> {
    let wire = field_str(data, "symbol")?;
    let symbol = wire.parse().unwrap_or_else(|_| Symbol::new(wire, ""));
    let client_oid = opt_str(data, "clientOid");
    Ok(Order {
        id: field_str(data, "orderId")?.to_string(),
        client_order_id: (!client_oid.is_empty()).then(|| client_oid.to_string()),
        symbol,
        side: parse_side(field_str(data, "side")?)?,
        order_type: parse_order_type(field_str(data, "orderType")?)?,
        status: ws_order_status(opt_str(data, "status"), opt_str(data, "type")),
        quantity: parse_decimal(field_str(data, "size")?)?,
        filled_quantity: dec_or_zero(opt_str(data, "filledSize")),
        price: nonzero_decimal(opt_str(data, "price")),
        average_price: None,
    })
}

/// The bullet-private token negotiation response.
#[derive(Deserialize)]
struct BulletToken {
    token: String,
    #[serde(rename = "instanceServers", default)]
    instance_servers: Vec<BulletServer>,
}

#[derive(Deserialize)]
struct BulletServer {
    endpoint: String,
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
struct RawStats {
    last: String,
    buy: String,
    sell: String,
    vol: String,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(default)]
    sequence: String,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
struct PlaceResult {
    #[serde(rename = "orderId", default)]
    order_id: String,
}

#[derive(Deserialize)]
struct OrderPage {
    items: Vec<RawOrder>,
}

#[derive(Deserialize)]
struct RawOrder {
    id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: String,
    #[serde(default)]
    symbol: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    size: String,
    #[serde(rename = "dealSize", default)]
    deal_size: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "isActive", default)]
    is_active: bool,
    #[serde(rename = "cancelExist", default)]
    cancel_exist: bool,
}

#[derive(Deserialize)]
struct RawAccount {
    currency: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    holds: String,
}

impl MarketData for KuCoin {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        KuCoin::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        KuCoin::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        KuCoin::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        KuCoin::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        KuCoin::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        KuCoin::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        KuCoin::poll_events(self)
    }
}

impl Execution for KuCoin {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        KuCoin::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        KuCoin::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        KuCoin::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        KuCoin::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        KuCoin::balances(self)
    }
}

impl KuCoin {
    /// Open futures positions (`/api/v1/positions` on the futures host); flat
    /// positions are omitted. Quantities are in **contracts**, KuCoin's native
    /// futures unit.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let data = self.signed_request(HttpMethod::Get, "/api/v1/positions", "", "")?;
        let list: Vec<RawKuPosition> = parse_json(data)?;
        list.iter()
            .filter(|p| symbol.is_none_or(|s| p.symbol == Self::futures_symbol(s)))
            .filter_map(parse_ku_position)
            .collect()
    }

    /// Record the leverage applied to subsequent futures orders. KuCoin has no
    /// standalone set-leverage endpoint — leverage is a per-order field — so this
    /// stores it locally rather than issuing a request.
    ///
    /// # Errors
    /// Never errors; the signature matches the [`Derivatives`] trait.
    pub fn set_leverage(&mut self, _symbol: &Symbol, leverage: u32) -> Result<()> {
        self.leverage = leverage.max(1);
        Ok(())
    }

    /// Set the margin mode for `symbol` (`/api/v1/position/changeMarginMode`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let margin = match mode {
            MarginMode::Cross => "CROSS",
            MarginMode::Isolated => "ISOLATED",
        };
        let body = serde_json::json!({
            "symbol": Self::futures_symbol(symbol),
            "marginMode": margin,
        });
        self.signed_request(
            HttpMethod::Post,
            "/api/v1/position/changeMarginMode",
            "",
            &body.to_string(),
        )?;
        Ok(())
    }

    /// Flatten the open position in `symbol` with a reduce-only market order
    /// (size in contracts).
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

    /// Place several spot orders on one symbol in one request
    /// (`/api/v1/orders/multi`). KuCoin returns the results in request order, so
    /// each element maps to its request's own [`Result`].
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails, or if called on a
    /// futures client (multi is a spot endpoint).
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "KuCoin multi-order is a spot endpoint".to_string(),
            });
        }
        let wire = Self::wire_symbol(&requests[0].symbol);
        let order_list: Vec<serde_json::Value> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let coid = r
                    .client_order_id
                    .clone()
                    .unwrap_or_else(|| format!("wkex-{i}"));
                let mut o = serde_json::json!({
                    "clientOid": coid,
                    "side": side_str(r.side),
                    "type": order_type_str(r.order_type),
                    "size": format_decimal(r.quantity),
                });
                if let Some(price) = r.price {
                    o["price"] = serde_json::json!(format_decimal(price));
                }
                o
            })
            .collect();
        let body = serde_json::json!({ "symbol": wire, "orderList": order_list });
        let data = self.signed_request(
            HttpMethod::Post,
            "/api/v1/orders/multi",
            "",
            &body.to_string(),
        )?;
        let batch: MultiBatch = parse_json(data)?;
        Ok(requests
            .iter()
            .zip(batch.data)
            .map(|(req, res)| {
                if res.order_id.is_empty() || res.status == "fail" {
                    return Err(Error::OrderRejected {
                        code: "batch".to_string(),
                        message: if res.fail_msg.is_empty() {
                            "order rejected in batch".to_string()
                        } else {
                            res.fail_msg
                        },
                    });
                }
                Ok(Order {
                    id: res.order_id,
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

    /// Cancel several orders by id. KuCoin has no batch-cancel-by-id endpoint, so
    /// the ids are cancelled sequentially.
    ///
    /// # Errors
    /// Returns an [`Error`] if any cancel fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        for id in order_ids {
            self.cancel_order(symbol, id)?;
        }
        Ok(())
    }

    /// Place a one-cancels-other bracket (`/api/v3/oco/order`). KuCoin models an
    /// OCO as one order-list, so the returned vector holds one order carrying the
    /// list id.
    ///
    /// # Errors
    /// Returns an [`Error`] if the OCO is invalid or rejected.
    pub fn place_oco(&self, request: &OcoRequest) -> Result<Vec<Order>> {
        request.validate()?;
        let client_oid = request
            .client_order_id
            .clone()
            .unwrap_or_else(|| format!("woco-{}", (self.now_ms)()));
        let stop_limit = request.stop_limit_price.unwrap_or(request.stop_price);
        let body = serde_json::json!({
            "symbol": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "size": format_decimal(request.quantity),
            "price": format_decimal(request.price),
            "stopPrice": format_decimal(request.stop_price),
            "limitPrice": format_decimal(stop_limit),
            "clientOid": client_oid,
        });
        let data =
            self.signed_request(HttpMethod::Post, "/api/v3/oco/order", "", &body.to_string())?;
        let placed: PlaceResult = parse_json(data)?;
        Ok(vec![Order {
            id: placed.order_id,
            client_order_id: Some(client_oid),
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

impl AdvancedOrders for KuCoin {
    /// KuCoin has no in-place amend (orders are cancelled and re-placed), so this
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
            message: "KuCoin has no in-place amend; cancel and re-place the order".to_string(),
        })
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        KuCoin::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        KuCoin::cancel_batch(self, symbol, order_ids)
    }
    fn place_oco(&mut self, request: &OcoRequest) -> Result<Vec<Order>> {
        KuCoin::place_oco(self, request)
    }
}

#[derive(Deserialize)]
struct MultiBatch {
    #[serde(default)]
    data: Vec<MultiResult>,
}

#[derive(Deserialize)]
struct MultiResult {
    #[serde(rename = "orderId", default)]
    order_id: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "failMsg", default)]
    fail_msg: String,
}

impl Exchange for KuCoin {
    fn name(&self) -> &'static str {
        "kucoin"
    }
}

impl WsUserData for KuCoin {
    fn subscribe_user_data(&mut self) -> Result<()> {
        KuCoin::subscribe_user_data(self)
    }
}

impl Derivatives for KuCoin {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        KuCoin::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        KuCoin::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        KuCoin::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        KuCoin::close_position(self, symbol)
    }
}

/// Reconstruct a canonical [`Symbol`] from a KuCoin futures contract symbol
/// (`XBTUSDTM` -> `BTC/USDT`): drop the trailing `M`, split off a known quote
/// suffix, and map `XBT` back to `BTC`.
fn symbol_from_futures(contract: &str) -> Symbol {
    const QUOTES: &[&str] = &["USDT", "USDC", "USD"];
    let stripped = contract.strip_suffix('M').unwrap_or(contract);
    for quote in QUOTES {
        if let Some(base) = stripped.strip_suffix(quote) {
            if !base.is_empty() {
                let base = if base == "XBT" { "BTC" } else { base };
                return Symbol::new(base, *quote);
            }
        }
    }
    Symbol::new(stripped, "")
}

#[derive(Deserialize)]
struct RawKuPosition {
    symbol: String,
    #[serde(rename = "currentQty")]
    current_qty: f64,
    #[serde(rename = "avgEntryPrice", default)]
    avg_entry_price: f64,
    #[serde(rename = "markPrice", default)]
    mark_price: f64,
    #[serde(rename = "realLeverage", default)]
    real_leverage: f64,
    #[serde(rename = "unrealisedPnl", default)]
    unrealised_pnl: f64,
    #[serde(rename = "crossMode", default)]
    cross_mode: bool,
}

fn parse_ku_position(raw: &RawKuPosition) -> Option<Result<Position>> {
    if raw.current_qty == 0.0 {
        return None; // flat position
    }
    let side = if raw.current_qty < 0.0 {
        PositionSide::Short
    } else {
        PositionSide::Long
    };
    let dec = |value: f64| Decimal::from_f64_retain(value).unwrap_or_default();
    Some(Ok(Position {
        symbol: symbol_from_futures(&raw.symbol),
        side,
        quantity: dec(raw.current_qty.abs()),
        entry_price: dec(raw.avg_entry_price),
        mark_price: dec(raw.mark_price),
        leverage: dec(raw.real_leverage),
        unrealized_pnl: dec(raw.unrealised_pnl),
        margin_mode: if raw.cross_mode {
            MarginMode::Cross
        } else {
            MarginMode::Isolated
        },
    }))
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

    fn client() -> (KuCoin, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            KuCoin::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (KuCoin, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let kucoin = KuCoin::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (kucoin, mock)
    }

    fn signed_ws_client(now_ms: i64) -> (KuCoin, Arc<MockHttpTransport>, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let kucoin = KuCoin::with_credentials(
            Box::new(ArcTransport(Arc::clone(&http))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (kucoin, http, ws)
    }

    #[test]
    fn subscribe_user_data_negotiates_token_and_streams_orders_and_balance() {
        let (mut kucoin, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"code":"200000","data":{"token":"tok","instanceServers":[
            {"endpoint":"wss://push.kucoin.com/endpoint"}]}}"#,
        );
        ws.push_connection(vec![
            Ok(Some(r#"{"type":"ack","id":"1000"}"#.to_string())),
            Ok(Some(
                r#"{"type":"message","topic":"/spotMarket/tradeOrders","subject":"orderChange",
                "data":{"symbol":"BTC-USDT","orderId":"55","clientOid":"my","side":"buy",
                "orderType":"limit","type":"filled","status":"done","size":"1","filledSize":"1",
                "price":"100"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"type":"message","topic":"/account/balance","subject":"account.balance",
                "data":{"currency":"USDT","available":"900","hold":"50","total":"950"}}"#
                    .to_string(),
            )),
        ]);
        kucoin.subscribe_user_data().unwrap();

        let reqs = http.recorded_requests();
        assert!(reqs[0].url.contains("/api/v1/bullet-private"));
        assert_eq!(reqs[0].method, HttpMethod::Post);
        assert_eq!(
            ws.connected_urls()[0],
            "wss://push.kucoin.com/endpoint?token=tok&connectId=1000"
        );
        assert!(ws.sent()[0].contains("/spotMarket/tradeOrders"));
        assert!(ws.sent()[1].contains("/account/balance"));

        let events = kucoin.poll_events();
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
        let mut kucoin = KuCoin::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        assert!(matches!(
            kucoin.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    fn signed_futures_client(now_ms: i64) -> (KuCoin, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let kucoin = KuCoin::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET").with_passphrase("PASS"),
        )
        .with_clock(Box::new(move || now_ms));
        (kucoin, mock)
    }

    const KU_POSITIONS: &str = r#"{"code":"200000","data":[
        {"symbol":"XBTUSDTM","currentQty":3,"avgEntryPrice":20000.0,"markPrice":20100.0,"realLeverage":10.0,"unrealisedPnl":30.0,"crossMode":false}
    ]}"#;

    #[test]
    fn stp_maps_to_stp_flag() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"1"}}"#);
        kucoin
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .with_stp(SelfTradePrevention::ExpireBoth),
            )
            .unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[0].body.as_ref().unwrap().contains(r#""stp":"CB""#));
    }

    #[test]
    fn place_batch_multi_per_order_results() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"200000","data":{"data":[
            {"orderId":"o1","status":"success"},
            {"orderId":"","status":"fail","failMsg":"insufficient"}]}}"#,
        );
        let results = kucoin
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
            .contains("/api/v1/orders/multi"));
    }

    #[test]
    fn cancel_batch_is_sequential() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{}}"#);
        mock.push_json(200, r#"{"code":"200000","data":{}}"#);
        kucoin
            .cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        assert_eq!(mock.recorded_requests().len(), 2);
    }

    #[test]
    fn place_oco_is_a_single_order_list() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"oco1"}}"#);
        let legs = kucoin
            .place_oco(&OcoRequest::new(
                symbol(),
                OrderSide::Sell,
                dec!(1),
                dec!(110),
                dec!(95),
            ))
            .unwrap();
        assert_eq!(legs.len(), 1);
        assert_eq!(legs[0].id, "oco1");
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/api/v3/oco/order"));
        assert!(reqs[0]
            .body
            .as_ref()
            .unwrap()
            .contains(r#""stopPrice":"95""#));
    }

    #[test]
    fn amend_is_unsupported() {
        let (mut kucoin, _mock) = signed_client(1000);
        assert!(matches!(
            AdvancedOrders::amend_order(&mut kucoin, &symbol(), "1", Some(dec!(1)), None)
                .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn futures_client_uses_futures_host_and_contract_symbol() {
        let (kucoin, _mock) = signed_futures_client(1000);
        assert!(kucoin.rest_base.contains("api-futures.kucoin.com"));
        assert_eq!(KuCoin::futures_symbol(&symbol()), "XBTUSDTM");
    }

    #[test]
    fn futures_place_order_carries_leverage_and_contract_symbol() {
        let (mut kucoin, mock) = signed_futures_client(1000);
        kucoin.set_leverage(&symbol(), 5).unwrap();
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"9"}}"#);
        kucoin
            .place_order(&OrderRequest::market_buy(symbol(), dec!(2)))
            .unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[0].body.as_deref().unwrap();
        assert!(body.contains(r#""symbol":"XBTUSDTM""#));
        assert!(body.contains(r#""leverage":"5""#));
        assert!(reqs[0].url.contains("api-futures.kucoin.com"));
    }

    #[test]
    fn derivatives_positions_parse_contracts() {
        let (mut kucoin, mock) = signed_futures_client(1000);
        mock.push_json(200, KU_POSITIONS);
        let positions = Derivatives::positions(&mut kucoin, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(3)); // contracts
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/v1/positions"));
    }

    #[test]
    fn derivatives_set_margin_mode_changes_mode() {
        let (mut kucoin, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{}}"#);
        Derivatives::set_margin_mode(&mut kucoin, &symbol(), MarginMode::Cross).unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v1/position/changeMarginMode"));
        assert!(req
            .body
            .as_deref()
            .unwrap()
            .contains(r#""marginMode":"CROSS""#));
    }

    #[test]
    fn derivatives_close_position_reduce_only() {
        let (mut kucoin, mock) = signed_futures_client(1000);
        mock.push_json(200, KU_POSITIONS);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"9"}}"#);
        Derivatives::close_position(&mut kucoin, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[1].body.as_deref().unwrap();
        assert!(body.contains(r#""side":"sell""#));
        assert!(body.contains(r#""reduceOnly":true"#));
    }

    #[test]
    fn ticker_from_stats() {
        let (kucoin, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"200000","data":{"last":"20000","buy":"19999","sell":"20001","vol":"1234"}}"#,
        );
        let ticker = kucoin.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.ask, dec!(20001));
    }

    #[test]
    fn klines_field_order_and_reverse() {
        let (kucoin, mock) = client();
        // [time, open, close, high, low, volume, turnover], newest-first.
        mock.push_json(
            200,
            r#"{"code":"200000","data":[
            ["1700000060","105","105.5","106","104","2","0"],
            ["1700000000","100","105","110","95","12","0"]]}"#,
        );
        let candles = kucoin.klines(&symbol(), "1h", 2).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        // open=100, high=110, low=95, close=105.
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!((candles[0].low - 95.0).abs() < 1e-9);
        assert!((candles[0].close - 105.0).abs() < 1e-9);
    }

    #[test]
    fn order_book_parses() {
        let (kucoin, mock) = client();
        mock.push_json(
            200,
            r#"{"code":"200000","data":{"sequence":"55","bids":[["100","1"]],"asks":[["101","2"]]}}"#,
        );
        let book = kucoin.order_book(&symbol(), 20).unwrap();
        assert_eq!(book.last_update_id, 55);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
    }

    #[test]
    fn error_mapping() {
        let (kucoin, mock) = client();
        mock.push_json(200, r#"{"code":"200004","msg":"balance","data":null}"#);
        assert!(matches!(
            kucoin.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn place_order_signs_with_kc_headers_and_signed_passphrase() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"99"}}"#);
        let order = kucoin
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "99");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));

        let req = &mock.recorded_requests()[0];
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        let ts = header("KC-API-TIMESTAMP");
        assert_eq!(ts, "1000");
        let body = req.body.as_ref().unwrap();
        let prehash = format!("{ts}POST/api/v1/orders{body}");
        assert_eq!(
            header("KC-API-SIGN"),
            hmac_sha256_base64(b"SECRET", prehash.as_bytes())
        );
        // The passphrase header is itself HMAC-signed.
        assert_eq!(
            header("KC-API-PASSPHRASE"),
            hmac_sha256_base64(b"SECRET", b"PASS")
        );
        assert_eq!(header("KC-API-KEY-VERSION"), "2");
    }

    #[test]
    fn place_order_generates_client_oid_when_absent() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"1"}}"#);
        let order = kucoin
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.client_order_id.as_deref(), Some("wkex-1000"));
    }

    #[test]
    fn query_order_status_from_flags() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"200000","data":{"id":"99","clientOid":"","symbol":"BTC-USDT",
            "side":"sell","type":"limit","size":"2","dealSize":"0","price":"100",
            "isActive":false,"cancelExist":false}}"#,
        );
        let order = kucoin.query_order(&symbol(), "99").unwrap();
        // Inactive, not cancelled -> filled.
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
    }

    #[test]
    fn balances_and_open_orders() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"code":"200000","data":[{"currency":"USDT","available":"100.5","holds":"25.5"}]}"#,
        );
        let bals = kucoin.balances().unwrap();
        assert_eq!(bals[0].total(), dec!(126));

        mock.push_json(
            200,
            r#"{"code":"200000","data":{"items":[{"id":"7","clientOid":"","symbol":"ETH-USDT",
            "side":"buy","type":"limit","size":"1","dealSize":"0","price":"50",
            "isActive":true,"cancelExist":false}]}}"#,
        );
        let open = kucoin.open_orders(None).unwrap();
        assert_eq!(open[0].symbol, Symbol::new("ETH", "USDT"));
        assert_eq!(open[0].status, OrderStatus::New);
    }

    #[test]
    fn signed_requires_passphrase() {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let kucoin = KuCoin::with_credentials(
            Box::new(ArcTransport(mock)),
            &opts,
            Credentials::new("k", "s"),
        );
        assert!(matches!(
            kucoin.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_parses_trade_ticker_book() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"type":"message","topic":"/market/match:BTC-USDT","subject":"trade.l3match",
                "data":{"symbol":"BTC-USDT","side":"buy","price":"100","size":"0.5","time":"1"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"type":"message","topic":"/market/level2:BTC-USDT","subject":"trade.l2update",
                "data":{"sequenceEnd":9,"changes":{"bids":[["100","1","9"]],"asks":[]}}}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"type":"welcome"}"#.to_string())),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut kucoin = KuCoin::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        kucoin.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""topic":"/market/match:BTC-USDT""#));

        let events = kucoin.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        let Event::BookDelta(d) = &events[1] else {
            panic!("expected book delta")
        };
        assert_eq!(d.final_update_id, 9);
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (kucoin, mock) = signed_client(1000);
        mock.push_json(200, r#"{"code":"200000","data":{"orderId":"1"}}"#);
        let mut exchange: Box<dyn Exchange> = Box::new(kucoin);
        assert_eq!(exchange.name(), "kucoin");
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
