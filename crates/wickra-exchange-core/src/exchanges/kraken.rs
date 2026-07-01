//! Kraken — the eighth exchange.
//!
//! Kraken's private signing is the most involved: `API-Sign = base64(HMAC-SHA512(
//! base64decode(secret), path_bytes ++ SHA256(nonce ++ postdata)))`, with the
//! request sent as a form-encoded POST carrying an increasing `nonce`. The API
//! secret is itself base64. REST uses concatenated symbols with Bitcoin spelled
//! `XBT` (`XBTUSDT`) and returns a `{error, result}` envelope where `result` is
//! keyed by the venue's own pair name; the v2 WebSocket uses slash symbols
//! (`BTC/USDT`).
//!
//! When built with a futures [`MarketType`](crate::MarketType), the client
//! targets **Kraken Futures** (`futures.kraken.com`), a separate product: flex
//! multi-collateral perpetuals named `PF_XBTUSD` (USD-settled), a distinct
//! `{"result":"success"|"error"}` envelope, and its own signing
//! (`Authent = base64(HMAC-SHA512(b64secret, SHA256(postData ++ nonce ++
//! path)))`). Market data uses `/derivatives/api/v3/{tickers,orderbook}` and the
//! `/api/charts/v1` OHLC feed; execution and the [`Derivatives`] trait use
//! `sendorder`/`openpositions`/`leveragepreferences`, and
//! `query_order`/`cancel_order`/`open_orders` route to
//! `/derivatives/api/v3/{orders/status,cancelorder,openorders}` with the Kraken
//! Futures order shape. The [`WsUserData`] stream routes to the separate Kraken
//! Futures feed (`wss://futures.kraken.com/ws/v1`, challenge/response auth →
//! `open_orders`/`balances`). Documented gaps: [`WsExecution`] stays REST-only
//! (Kraken Futures has no WebSocket order-entry API), `openpositions` omits mark
//! price and unrealized PnL, and `set_margin_mode(Isolated)` is unsupported within
//! the flex (cross) account.
//!
//! [`AdvancedOrders`]: native in-place amend (`/0/private/EditOrder`, which
//! assigns a new txid), native batch placement (`/0/private/AddOrderBatch`, whose
//! indexed `orders[i][…]` form array is built by a dedicated encoder) and native
//! batch cancel (`/0/private/CancelOrderBatch`). Kraken has no OCO order-list (it
//! uses conditional-close orders), so `place_oco` is a documented gap.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType};
use crate::positions::{Position, PositionSide};
use crate::signing::{hmac_sha512_base64_with_b64_secret, sha256};
use crate::symbol::Symbol;
use crate::traits::{
    AdvancedOrders, Derivatives, Exchange, Execution, MarketData, WsExecution, WsUserData,
};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{
    Balance, OcoRequest, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker,
};
use rust_decimal::Decimal;
use std::cell::Cell;
use wickra_core::Candle;

/// Spot REST host.
const SPOT_HOST: &str = "https://api.kraken.com";
/// Kraken Futures host (a separate product with its own signing and symbols).
const FUTURES_HOST: &str = "https://futures.kraken.com";

fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// A Kraken client over injected transports.
pub struct Kraken {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    market_type: MarketType,
    credentials: Option<Credentials>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    /// Leverage applied on Kraken Futures (`maxLeverage` preference), recorded so
    /// [`positions`](Self::positions) can report it (the venue omits per-position
    /// leverage in `openpositions`).
    leverage: Cell<u32>,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
    /// Set once the private stream is subscribed, so [`poll_events`](Self::poll_events)
    /// re-subscribes it (re-fetching a fresh single-use token) after a drop.
    user_data_active: bool,
    /// A dedicated connection to the v2 authenticated order API, opened lazily on
    /// the first [`place_order_ws`](Self::place_order_ws) / [`cancel_order_ws`](Self::cancel_order_ws)
    /// call, together with the WebSocket token each request carries.
    ws_api_connection: Option<Box<dyn WsConnection>>,
    ws_api_token: Option<String>,
}

impl Kraken {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        let futures = options.market_type.is_derivatives();
        Self {
            http,
            ws: None,
            rest_base: if futures { FUTURES_HOST } else { SPOT_HOST }.to_string(),
            market_type: options.market_type,
            credentials,
            now_ms: Box::new(system_now_ms),
            connection: None,
            sub_messages: Vec::new(),
            leverage: Cell::new(1),
            private_connection: None,
            user_data_active: false,
            ws_api_connection: None,
            ws_api_token: None,
        }
    }

    /// Whether this client targets Kraken Futures (`futures.kraken.com`) rather
    /// than the spot REST API.
    fn is_futures(&self) -> bool {
        self.market_type.is_derivatives()
    }

    /// The Kraken **Futures** perpetual symbol for a canonical [`Symbol`]
    /// (`BTC/USD` -> `PF_XBTUSD`). Kraken flex perpetuals are USD-settled, so a
    /// `USDT` quote maps to the `USD` contract.
    fn futures_symbol(symbol: &Symbol) -> String {
        let quote = if symbol.quote() == "USDT" {
            "USD"
        } else {
            symbol.quote()
        };
        format!("PF_{}{}", kraken_asset(symbol.base()), quote)
    }

    /// Build a public Kraken client.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Kraken client (the secret must be base64).
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

    /// The Kraken REST wire symbol (`BTC/USDT` -> `XBTUSDT`, Bitcoin spelled XBT).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        format!("{}{}", kraken_asset(symbol.base()), symbol.quote())
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        if self.is_futures() {
            return self.futures_ticker(symbol);
        }
        let query = format!("pair={}", Self::wire_symbol(symbol));
        let result = self.get("/0/public/Ticker", &query)?;
        let tick = single_result(&result)?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: decimal_at(tick, "c", 0)?,
            bid: decimal_at(tick, "b", 0)?,
            ask: decimal_at(tick, "a", 0)?,
            volume: decimal_at(tick, "v", 1)?,
        })
    }

    fn futures_ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let wanted = Self::futures_symbol(symbol);
        let value = self.futures_get("/derivatives/api/v3/tickers", "")?;
        let tick = value
            .get("tickers")
            .and_then(serde_json::Value::as_array)
            .and_then(|list| {
                list.iter().find(|t| {
                    t.get("symbol")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s.eq_ignore_ascii_case(&wanted))
                })
            })
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: decimal_field(tick, "last").or_else(|_| decimal_field(tick, "markPrice"))?,
            bid: decimal_field(tick, "bid").unwrap_or(Decimal::ZERO),
            ask: decimal_field(tick, "ask").unwrap_or(Decimal::ZERO),
            volume: decimal_field(tick, "vol24h").unwrap_or(Decimal::ZERO),
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified). Kraken returns
    /// oldest-first.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, _limit: u32) -> Result<Vec<Candle>> {
        if self.is_futures() {
            return self.futures_klines(symbol, interval);
        }
        let query = format!(
            "pair={}&interval={}",
            Self::wire_symbol(symbol),
            map_interval(interval),
        );
        let result = self.get("/0/public/OHLC", &query)?;
        // result has the pair key (an array) plus a scalar "last".
        let rows = result
            .as_object()
            .and_then(|obj| {
                obj.iter()
                    .find(|(key, value)| *key != "last" && value.is_array())
            })
            .map(|(_, value)| value)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing OHLC rows".to_string()))?;
        rows.iter().map(parse_kline_row).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        if self.is_futures() {
            let query = format!("symbol={}", Self::futures_symbol(symbol));
            let value = self.futures_get("/derivatives/api/v3/orderbook", &query)?;
            let book = value
                .get("orderBook")
                .ok_or_else(|| Error::Deserialization("missing orderBook".to_string()))?;
            return Ok(OrderBookSnapshot {
                symbol: symbol.clone(),
                last_update_id: 0,
                bids: num_pair_levels(book.get("bids"))?,
                asks: num_pair_levels(book.get("asks"))?,
            });
        }
        let query = format!("pair={}&count={depth}", Self::wire_symbol(symbol));
        let result = self.get("/0/public/Depth", &query)?;
        let book = single_result(&result)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: 0,
            bids: rest_levels(book.get("bids"))?,
            asks: rest_levels(book.get("asks"))?,
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
        self.subscribe(symbol, "book")
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect("wss://ws.kraken.com/v2")?;
            self.connection = Some(connection);
        }
        // The v2 WebSocket uses the canonical slash symbol.
        let message = format!(
            r#"{{"method":"subscribe","params":{{"channel":"{channel}","symbol":["{symbol}"]}}}}"#
        );
        self.connection
            .as_mut()
            .expect("connection just ensured")
            .send(&message)?;
        if !self.sub_messages.contains(&message) {
            self.sub_messages.push(message.clone());
        }
        Ok(())
    }

    /// Drain all stream events available since the last call. Non-blocking.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(connection) = self.connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame) {
                    events.append(&mut parsed);
                }
            }
        }
        // Drain the private user-data stream, if open. The spot client streams the
        // v2 executions/balances channels; the futures client streams the separate
        // Kraken Futures open_orders/balances feeds, which use a different shape.
        let futures = self.is_futures();
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                let parsed = if futures {
                    parse_futures_private(&frame)
                } else {
                    parse_ws_message(&frame)
                };
                if let Ok(mut parsed) = parsed {
                    events.append(&mut parsed);
                }
            }
        }
        // A dropped private stream is re-subscribed with a fresh handshake — the
        // spot client re-fetches a single-use WebSockets token, the futures client
        // re-runs the challenge/response — neither of which can be replayed.
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
        let url = "wss://ws.kraken.com/v2";
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Open the private user-data stream. Fetches a single-use token over REST
    /// (`POST /0/private/GetWebSocketsToken`, signed), connects
    /// `wss://ws-auth.kraken.com/v2`, then subscribes to the token-authenticated
    /// `executions` and `balances` channels. Afterwards
    /// [`poll_events`](Self::poll_events) also surfaces the account's own
    /// [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// A dropped private stream is re-subscribed automatically on the next
    /// [`poll_events`](Self::poll_events), which re-fetches a fresh token; call
    /// [`keepalive_user_data`](Self::keepalive_user_data) periodically to keep it
    /// from being dropped for inactivity.
    ///
    /// The **futures** client routes to the separate Kraken Futures feed
    /// (`wss://futures.kraken.com/ws/v1`) with challenge/response auth; see
    /// [`subscribe_user_data_futures`](Self::subscribe_user_data_futures).
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials,
    /// [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the token request fails.
    pub fn subscribe_user_data(&mut self) -> Result<()> {
        if self.is_futures() {
            return self.subscribe_user_data_futures();
        }
        let result = self.signed_post("/0/private/GetWebSocketsToken", &[])?;
        let token = result
            .get("token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing WebSockets token".to_string()))?;
        let executions = format!(
            r#"{{"method":"subscribe","params":{{"channel":"executions","token":"{token}","snap_orders":true}}}}"#
        );
        let balances = format!(
            r#"{{"method":"subscribe","params":{{"channel":"balances","token":"{token}","snap_balances":true}}}}"#
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://ws-auth.kraken.com/v2")?;
        connection.send(&executions)?;
        connection.send(&balances)?;
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Open the Kraken Futures private feed (`wss://futures.kraken.com/ws/v1`).
    /// Runs the challenge/response handshake — request a challenge
    /// (`{"event":"challenge","api_key":…}`), read the `{"event":"challenge",
    /// "message":<uuid>}` reply, and compute `signed_challenge =
    /// base64(HMAC-SHA512(base64decode(secret), SHA256(challenge)))` — then
    /// subscribes to the `open_orders` and `balances` feeds with the api key, the
    /// original challenge and the signed challenge. Afterwards
    /// [`poll_events`](Self::poll_events) surfaces the account's own
    /// [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials,
    /// [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the handshake fails.
    fn subscribe_user_data_futures(&mut self) -> Result<()> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "user-data stream requires credentials",
        ))?;
        let api_key = creds.api_key.clone();
        let api_secret = creds.api_secret.clone();
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect("wss://futures.kraken.com/ws/v1")?;
        // 1. Request a challenge for this api key.
        connection.send(&format!(r#"{{"event":"challenge","api_key":"{api_key}"}}"#))?;
        // 2. Read the challenge message, skipping any unrelated frames.
        let challenge = loop {
            let Some(frame) = connection.recv()? else {
                return Err(Error::NotConnected);
            };
            let value: serde_json::Value =
                serde_json::from_str(&frame).map_err(|e| Error::Deserialization(e.to_string()))?;
            if value.get("event").and_then(serde_json::Value::as_str) == Some("challenge") {
                break value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::Deserialization("missing challenge message".to_string()))?
                    .to_string();
            }
        };
        // 3. signed_challenge = base64(HMAC-SHA512(base64decode(secret), SHA256(challenge))).
        let signed =
            hmac_sha512_base64_with_b64_secret(&api_secret, &sha256(challenge.as_bytes()))?;
        // 4. Subscribe to the private feeds with the signed challenge.
        for feed in ["open_orders", "balances"] {
            connection.send(&format!(
                r#"{{"event":"subscribe","feed":"{feed}","api_key":"{api_key}","original_challenge":"{challenge}","signed_challenge":"{signed}"}}"#
            ))?;
        }
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send the Kraken v2 application-level heartbeat (`{"method":"ping"}`) on the
    /// spot private stream so it is not dropped for inactivity. A no-op before
    /// [`subscribe_user_data`](Self::subscribe_user_data), and a no-op on the
    /// futures client (Kraken Futures keeps the feed alive server-side via its
    /// heartbeat feed, so no client ping is required).
    ///
    /// # Errors
    /// Returns an [`Error`] if the ping cannot be sent.
    pub fn keepalive_user_data(&mut self) -> Result<()> {
        if self.is_futures() {
            return Ok(());
        }
        if let Some(connection) = self.private_connection.as_mut() {
            connection.send(r#"{"method":"ping"}"#)?;
        }
        Ok(())
    }

    /// Place an order over the Kraken v2 authenticated WebSocket order API
    /// (`add_order`). Fetches a `GetWebSocketsToken` token over REST on first use, connects
    /// `wss://ws-auth.kraken.com/v2`, and sends the order with the token.
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] on the **futures** client (Kraken Futures uses a
    /// separate feed), [`Error::NotConnected`] without a WebSocket transport, or
    /// another [`Error`] if the order is invalid or rejected.
    pub fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        let mut params = serde_json::Map::new();
        params.insert(
            "order_type".to_string(),
            serde_json::json!(order_type_str(request.order_type)),
        );
        params.insert(
            "side".to_string(),
            serde_json::json!(side_str(request.side)),
        );
        params.insert("order_qty".to_string(), json_number(request.quantity));
        params.insert(
            "symbol".to_string(),
            serde_json::json!(request.symbol.to_string()),
        );
        if let Some(price) = request.price {
            params.insert("limit_price".to_string(), json_number(price));
        }
        if let Some(id) = &request.client_order_id {
            params.insert("cl_ord_id".to_string(), serde_json::json!(id.clone()));
        }
        let result = self.ws_order_request("add_order", params)?;
        let id = result
            .get("order_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(Order {
            id,
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

    /// Cancel an order over the Kraken v2 authenticated WebSocket order API
    /// (`cancel_order`).
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] on the futures client, [`Error::NotConnected`]
    /// without a WebSocket transport, or another [`Error`] if the request fails.
    pub fn cancel_order_ws(&mut self, _symbol: &Symbol, order_id: &str) -> Result<()> {
        let mut params = serde_json::Map::new();
        params.insert("order_id".to_string(), serde_json::json!([order_id]));
        self.ws_order_request("cancel_order", params)?;
        Ok(())
    }

    /// Open the authenticated order connection if needed: fetch a
    /// `GetWebSocketsToken` token over REST, connect `wss://ws-auth.kraken.com/v2`,
    /// and cache both.
    fn ensure_ws_api(&mut self) -> Result<()> {
        if self.ws_api_connection.is_some() {
            return Ok(());
        }
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Kraken Futures exposes a separate WebSocket order feed \
                          (challenge/response auth on futures.kraken.com); the spot \
                          v2 order API is not available for the futures client"
                    .to_string(),
            });
        }
        let result = self.signed_post("/0/private/GetWebSocketsToken", &[])?;
        let token = result
            .get("token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing WebSockets token".to_string()))?
            .to_string();
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let connection = ws.connect("wss://ws-auth.kraken.com/v2")?;
        self.ws_api_connection = Some(connection);
        self.ws_api_token = Some(token);
        Ok(())
    }

    /// Send a token-authenticated order request frame and return its `result`,
    /// mapping `success == false` onto the error taxonomy.
    fn ws_order_request(
        &mut self,
        method: &str,
        mut params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.ensure_ws_api()?;
        let token = self
            .ws_api_token
            .clone()
            .expect("token set alongside the connection");
        params.insert("token".to_string(), serde_json::json!(token));
        let req_id = (self.now_ms)();
        let frame = serde_json::json!({
            "method": method,
            "params": serde_json::Value::Object(params),
            "req_id": req_id,
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
        if value.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(value
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        } else {
            let message = value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("order rejected")
                .to_string();
            Err(Error::OrderRejected {
                code: "ws".to_string(),
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
        let volume = format_decimal(request.quantity);
        let mut params: Vec<(&str, String)> = vec![
            ("pair", Self::wire_symbol(&request.symbol)),
            ("type", side_str(request.side).to_string()),
            ("ordertype", order_type_str(request.order_type).to_string()),
            ("volume", volume),
        ];
        if let Some(price) = request.price {
            params.push(("price", format_decimal(price)));
        }
        if request.post_only {
            params.push(("oflags", "post".to_string()));
        }
        if let Some(id) = &request.client_order_id {
            params.push(("cl_ord_id", id.clone()));
        }
        let result = self.signed_post("/0/private/AddOrder", &params)?;
        let txid = result
            .get("txid")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing txid".to_string()))?;
        Ok(Order {
            id: txid.to_string(),
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
        if self.is_futures() {
            self.signed_futures(
                HttpMethod::Post,
                "/derivatives/api/v3/cancelorder",
                &[("order_id", order_id.to_string())],
            )?;
            return Ok(());
        }
        self.signed_post("/0/private/CancelOrder", &[("txid", order_id.to_string())])?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        if self.is_futures() {
            let value = self.signed_futures(
                HttpMethod::Post,
                "/derivatives/api/v3/orders/status",
                &[("orderIds", format!("[\"{order_id}\"]"))],
            )?;
            let order = value
                .get("orders")
                .and_then(serde_json::Value::as_array)
                .and_then(|a| a.first())
                .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
            return kraken_futures_order(symbol.clone(), order_id, order);
        }
        let result =
            self.signed_post("/0/private/QueryOrders", &[("txid", order_id.to_string())])?;
        let order = result
            .get(order_id)
            .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
        order_from_value(symbol.clone(), order_id, order)
    }

    /// Open orders (Kraken returns them all; the `symbol` filter is applied locally).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        if self.is_futures() {
            let value =
                self.signed_futures(HttpMethod::Get, "/derivatives/api/v3/openorders", &[])?;
            let open = value
                .get("openOrders")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| Error::Deserialization("missing openOrders".to_string()))?;
            let want = symbol.map(Self::futures_symbol);
            return open
                .iter()
                .filter(|order| {
                    want.as_ref().is_none_or(|w| {
                        order
                            .get("symbol")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s.eq_ignore_ascii_case(w))
                    })
                })
                .map(|order| {
                    let sym = symbol.cloned().unwrap_or_else(|| {
                        symbol_from_futures(
                            order
                                .get("symbol")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(""),
                        )
                    });
                    let id = order
                        .get("order_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    kraken_futures_order(sym, id, order)
                })
                .collect();
        }
        let result = self.signed_post("/0/private/OpenOrders", &[])?;
        let open = result
            .get("open")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| Error::Deserialization("missing open orders".to_string()))?;
        let want = symbol.map(Self::wire_symbol);
        open.iter()
            .filter(|(_, order)| match &want {
                None => true,
                Some(w) => descr_pair(order) == *w,
            })
            .map(|(id, order)| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| unmap_pair(&descr_pair(order)));
                order_from_value(sym, id, order)
            })
            .collect()
    }

    /// Account balances (free/locked from `BalanceEx`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        if self.is_futures() {
            return self.futures_balances();
        }
        let result = self.signed_post("/0/private/BalanceEx", &[])?;
        let map = result
            .as_object()
            .ok_or_else(|| Error::Deserialization("missing balances".to_string()))?;
        let mut balances: Vec<Balance> = map
            .iter()
            .map(|(asset, detail)| {
                let balance = decimal_field(detail, "balance").unwrap_or(Decimal::ZERO);
                let hold = decimal_field(detail, "hold_trade").unwrap_or(Decimal::ZERO);
                Balance {
                    asset: asset.clone(),
                    free: balance - hold,
                    locked: hold,
                }
            })
            .collect();
        balances.sort_by(|a, b| a.asset.cmp(&b.asset));
        Ok(balances)
    }

    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_result(&response.body)
    }

    /// Sign a private POST: `API-Sign = base64(HMAC-SHA512(base64decode(secret),
    /// path ++ SHA256(nonce ++ postdata)))`, form-encoded body with the nonce.
    fn signed_post(&self, path: &str, params: &[(&str, String)]) -> Result<serde_json::Value> {
        let owned: Vec<(String, String)> = params
            .iter()
            .map(|(key, val)| ((*key).to_string(), val.clone()))
            .collect();
        self.signed_post_pairs(path, &owned)
    }

    /// Signed POST with owned form keys, for endpoints whose field names are built
    /// dynamically (e.g. `AddOrderBatch`'s `orders[i][…]` indexed array).
    fn signed_post_pairs(
        &self,
        path: &str,
        params: &[(String, String)],
    ) -> Result<serde_json::Value> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let nonce = (self.now_ms)().to_string();
        let mut form = vec![format!("nonce={nonce}")];
        for (key, val) in params {
            form.push(format!("{key}={val}"));
        }
        let postdata = form.join("&");
        let mut message = path.as_bytes().to_vec();
        message.extend_from_slice(&sha256(format!("{nonce}{postdata}").as_bytes()));
        let signature = hmac_sha512_base64_with_b64_secret(&creds.api_secret, &message)?;
        let url = format!("{}{path}", self.rest_base);
        let request = HttpRequest::new(HttpMethod::Post, url)
            .with_header("API-Key", creds.api_key.clone())
            .with_header("API-Sign", signature)
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body(postdata);
        let response = self.http.execute(&request)?;
        unwrap_result(&response.body)
    }

    /// Public Kraken Futures GET (no signing).
    fn futures_get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_futures(&response.body)
    }

    /// Signed Kraken Futures request. The signature is
    /// `Authent = base64(HMAC-SHA512(base64decode(secret), SHA256(postData ++
    /// nonce ++ endpointPath)))`, where `endpointPath` is the path **without** the
    /// `/derivatives` prefix. `postData` is the form body (empty for a plain GET).
    fn signed_futures(
        &self,
        method: HttpMethod,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let nonce = (self.now_ms)().to_string();
        let post_data = params
            .iter()
            .map(|(key, val)| format!("{key}={val}"))
            .collect::<Vec<_>>()
            .join("&");
        // The signed endpoint path drops the leading `/derivatives`.
        let endpoint_path = path.strip_prefix("/derivatives").unwrap_or(path);
        let concat = format!("{post_data}{nonce}{endpoint_path}");
        let message = sha256(concat.as_bytes());
        let signature = hmac_sha512_base64_with_b64_secret(&creds.api_secret, &message)?;
        let is_get = matches!(method, HttpMethod::Get);
        let url = if is_get && !post_data.is_empty() {
            format!("{}{path}?{post_data}", self.rest_base)
        } else {
            format!("{}{path}", self.rest_base)
        };
        let mut request = HttpRequest::new(method, url)
            .with_header("APIKey", creds.api_key.clone())
            .with_header("Nonce", nonce)
            .with_header("Authent", signature);
        if !is_get {
            request = request
                .with_header("Content-Type", "application/x-www-form-urlencoded")
                .with_body(post_data);
        }
        let response = self.http.execute(&request)?;
        unwrap_futures(&response.body)
    }

    fn futures_klines(&self, symbol: &Symbol, interval: &str) -> Result<Vec<Candle>> {
        // Charts live under a separate base path (no `/derivatives`).
        let path = format!(
            "/api/charts/v1/trade/{}/{}",
            Self::futures_symbol(symbol),
            map_futures_resolution(interval),
        );
        let value = self.futures_get(&path, "")?;
        let candles = value
            .get("candles")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing candles".to_string()))?;
        candles.iter().map(parse_futures_candle).collect()
    }

    /// Place a Kraken Futures order (`/derivatives/api/v3/sendorder`): `orderType`
    /// (`mkt`/`lmt`/`post`), `symbol`, `side`, `size`, optional `limitPrice`, and
    /// `reduceOnly`.
    fn place_futures_order(&self, request: &OrderRequest) -> Result<Order> {
        let order_type = match (request.order_type, request.post_only) {
            (OrderType::Market | OrderType::StopMarket, _) => "mkt",
            (OrderType::Limit | OrderType::StopLimit, true) => "post",
            (OrderType::Limit | OrderType::StopLimit, false) => "lmt",
        };
        let mut params: Vec<(&str, String)> = vec![
            ("orderType", order_type.to_string()),
            ("symbol", Self::futures_symbol(&request.symbol)),
            ("side", side_str(request.side).to_string()),
            ("size", format_decimal(request.quantity)),
            ("reduceOnly", request.reduce_only.to_string()),
        ];
        if let Some(price) = request.price {
            params.push(("limitPrice", format_decimal(price)));
        }
        if let Some(id) = &request.client_order_id {
            params.push(("cliOrdId", id.clone()));
        }
        let value =
            self.signed_futures(HttpMethod::Post, "/derivatives/api/v3/sendorder", &params)?;
        let send_status = value
            .get("sendStatus")
            .ok_or_else(|| Error::Deserialization("missing sendStatus".to_string()))?;
        let order_id = send_status
            .get("order_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing order_id".to_string()))?;
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

    /// Flex (multi-collateral) account balances
    /// (`/derivatives/api/v3/accounts`).
    fn futures_balances(&self) -> Result<Vec<Balance>> {
        let value = self.signed_futures(HttpMethod::Get, "/derivatives/api/v3/accounts", &[])?;
        let currencies = value
            .get("accounts")
            .and_then(|a| a.get("flex"))
            .and_then(|f| f.get("currencies"))
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| Error::Deserialization("missing flex currencies".to_string()))?;
        let mut balances: Vec<Balance> = currencies
            .iter()
            .map(|(asset, detail)| {
                let quantity = decimal_field(detail, "quantity").unwrap_or(Decimal::ZERO);
                let available = decimal_field(detail, "available").unwrap_or(quantity);
                Balance {
                    asset: asset.clone(),
                    free: available,
                    locked: (quantity - available).max(Decimal::ZERO),
                }
            })
            .collect();
        balances.sort_by(|a, b| a.asset.cmp(&b.asset));
        Ok(balances)
    }

    /// Open positions on the futures account
    /// (`/derivatives/api/v3/openpositions`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let value =
            self.signed_futures(HttpMethod::Get, "/derivatives/api/v3/openpositions", &[])?;
        let list = value
            .get("openPositions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing openPositions".to_string()))?;
        let wanted = symbol.map(Self::futures_symbol);
        list.iter()
            .filter(|p| {
                wanted.as_ref().is_none_or(|w| {
                    p.get("symbol")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s.eq_ignore_ascii_case(w))
                })
            })
            .map(|p| self.parse_futures_position(p))
            .collect()
    }

    /// Set the leverage preference for `symbol`
    /// (`/derivatives/api/v3/leveragepreferences`, `maxLeverage`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request is rejected or credentials are missing.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let lever = leverage.max(1);
        self.leverage.set(lever);
        let params = vec![
            ("symbol", Self::futures_symbol(symbol)),
            ("maxLeverage", lever.to_string()),
        ];
        self.signed_futures(
            HttpMethod::Put,
            "/derivatives/api/v3/leveragepreferences",
            &params,
        )?;
        Ok(())
    }

    /// Set the margin mode for `symbol`.
    ///
    /// Kraken Futures uses a **flex (multi-collateral, cross) account**, so
    /// `Cross` is a no-op success; there is no per-symbol switch to isolated
    /// within the flex account, so `Isolated` returns an [`Error::Exchange`].
    ///
    /// # Errors
    /// Returns [`Error::Exchange`] when `Isolated` is requested.
    pub fn set_margin_mode(&self, _symbol: &Symbol, mode: MarginMode) -> Result<()> {
        match mode {
            MarginMode::Cross => Ok(()),
            MarginMode::Isolated => Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Kraken Futures flex account is cross-margin; isolated is \
                          not switchable per-symbol"
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
        let want = Self::futures_symbol(symbol);
        let position = self
            .positions(Some(symbol))?
            .into_iter()
            .find(|p| Self::futures_symbol(&p.symbol).eq_ignore_ascii_case(&want))
            .ok_or_else(|| Error::NotFound(format!("no open position for {symbol}")))?;
        let request = match position.side {
            PositionSide::Long => OrderRequest::market_sell(symbol.clone(), position.quantity),
            PositionSide::Short => OrderRequest::market_buy(symbol.clone(), position.quantity),
        }
        .reduce_only();
        self.place_order(&request)
    }

    fn parse_futures_position(&self, data: &serde_json::Value) -> Result<Position> {
        let side_raw = data
            .get("side")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let side = match side_raw {
            "long" => PositionSide::Long,
            "short" => PositionSide::Short,
            other => {
                return Err(Error::Deserialization(format!(
                    "unknown position side {other:?}"
                )))
            }
        };
        let contract = data
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(Position {
            symbol: symbol_from_futures(contract),
            side,
            quantity: decimal_field(data, "size").unwrap_or(Decimal::ZERO),
            entry_price: decimal_field(data, "price").unwrap_or(Decimal::ZERO),
            // `openpositions` omits mark price and unrealized PnL; leverage is a
            // preference, not reported per-position, so the recorded value is used.
            mark_price: Decimal::ZERO,
            leverage: Decimal::from(self.leverage.get()),
            unrealized_pnl: Decimal::ZERO,
            margin_mode: MarginMode::Cross,
        })
    }
}

fn kraken_asset(asset: &str) -> String {
    match asset {
        "BTC" => "XBT".to_string(),
        other => other.to_string(),
    }
}

fn unmap_pair(pair: &str) -> Symbol {
    // Best-effort inverse of `wire_symbol` for the common quotes.
    for quote in ["USDT", "USDC", "EUR", "USD", "XBT", "ETH"] {
        if let Some(base) = pair.strip_suffix(quote) {
            if !base.is_empty() {
                let base = if base == "XBT" { "BTC" } else { base };
                return Symbol::new(base, quote);
            }
        }
    }
    Symbol::new(pair, "")
}

fn map_interval(interval: &str) -> &'static str {
    match interval {
        "1m" => "1",
        "5m" => "5",
        "15m" => "15",
        "30m" => "30",
        "4h" => "240",
        "1d" => "1440",
        "1w" => "10080",
        // Default (and "1h") map to 60-minute candles.
        _ => "60",
    }
}

/// The single value of a one-entry result object (Kraken keys it by pair name).
fn single_result(result: &serde_json::Value) -> Result<&serde_json::Value> {
    result
        .as_object()
        .and_then(|obj| obj.values().next())
        .ok_or_else(|| Error::Deserialization("empty result".to_string()))
}

fn unwrap_result(body: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if let Some(errors) = value.get("error").and_then(serde_json::Value::as_array) {
        if let Some(first) = errors.first().and_then(serde_json::Value::as_str) {
            return Err(map_error(first));
        }
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| Error::Deserialization("missing result".to_string()))
}

/// Unwrap a Kraken Futures envelope: `{"result":"success", ...}` on success,
/// `{"result":"error","error":"..."}` on failure. The payload fields are
/// siblings of `result`, so the whole value is returned.
fn unwrap_futures(body: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    match value.get("result").and_then(serde_json::Value::as_str) {
        Some("error") => {
            let error = value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            Err(map_futures_error(error))
        }
        _ => Ok(value),
    }
}

fn map_futures_error(error: &str) -> Error {
    match error {
        "insufficientFunds" | "insufficientAvailableFunds" => Error::InsufficientBalance,
        "apiLimitExceeded" => Error::RateLimited { retry_after: None },
        "authenticationError" | "invalidAccount" => Error::Auth(error.to_string()),
        "invalidUnit" | "invalidArgument" | "marketSuspended" => {
            Error::InvalidSymbol(error.to_string())
        }
        "orderNotFound" | "notFound" => Error::NotFound(error.to_string()),
        other => Error::Exchange {
            code: "kraken-futures".to_string(),
            message: other.to_string(),
        },
    }
}

/// Kraken Futures resolution for a unified interval.
fn map_futures_resolution(interval: &str) -> &'static str {
    match interval {
        "1m" => "1m",
        "5m" => "5m",
        "15m" => "15m",
        "30m" => "30m",
        "4h" => "4h",
        "12h" => "12h",
        "1d" => "1d",
        "1w" => "1w",
        // Default (and "1h") map to the 1-hour resolution.
        _ => "1h",
    }
}

/// Kraken Futures order-book levels are `[price, size]` numeric arrays.
fn num_pair_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
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

fn parse_futures_candle(row: &serde_json::Value) -> Result<Candle> {
    let ts = row
        .get("time")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| Error::Deserialization("candle time missing".to_string()))?;
    let f = |key: &str| -> Result<f64> {
        let field = row
            .get(key)
            .ok_or_else(|| Error::Deserialization(format!("candle {key} missing")))?;
        field
            .as_f64()
            .or_else(|| field.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| Error::Deserialization(format!("candle {key} not a number")))
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

/// Reconstruct a canonical [`Symbol`] from a Kraken Futures perpetual symbol
/// (`PF_XBTUSD` -> `BTC/USD`).
fn symbol_from_futures(contract: &str) -> Symbol {
    let core = contract
        .strip_prefix("PF_")
        .or_else(|| contract.strip_prefix("PI_"))
        .or_else(|| contract.strip_prefix("pf_"))
        .or_else(|| contract.strip_prefix("pi_"))
        .unwrap_or(contract)
        .to_uppercase();
    for quote in ["USDT", "USDC", "USD", "EUR"] {
        if let Some(base) = core.strip_suffix(quote) {
            if !base.is_empty() {
                let base = if base == "XBT" { "BTC" } else { base };
                return Symbol::new(base, quote);
            }
        }
    }
    Symbol::new(core, "")
}

/// Map a Kraken Futures order status string onto the unified status. Tolerant of
/// both the `orders/status` (`ENTERED_BOOK` / `FULLY_EXECUTED`) and `openorders`
/// (`untouched` / `partiallyFilled`) casings.
fn kraken_futures_status(raw: &str) -> OrderStatus {
    let s = raw.to_ascii_lowercase();
    if s.contains("cancel") || s.contains("expired") {
        OrderStatus::Canceled
    } else if s.contains("reject") {
        OrderStatus::Rejected
    } else if s.contains("partial") {
        OrderStatus::PartiallyFilled
    } else if s.contains("fully") || s.contains("filled") || s.contains("executed") {
        OrderStatus::Filled
    } else {
        OrderStatus::New
    }
}

fn kraken_futures_order_type(raw: &str) -> OrderType {
    let s = raw.to_ascii_lowercase();
    if s.contains("lmt") || s.contains("limit") || s.contains("post") {
        OrderType::Limit
    } else {
        OrderType::Market
    }
}

/// Parse a Kraken Futures order object. The `orders/status` endpoint nests the
/// detail under `order`; `openorders` is flat and reports `filledSize` /
/// `unfilledSize` instead of a total `quantity`.
fn kraken_futures_order(symbol: Symbol, id: &str, obj: &serde_json::Value) -> Result<Order> {
    let detail = obj.get("order").unwrap_or(obj);
    let side = detail
        .get("side")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let order_type = detail
        .get("orderType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let status = obj
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let filled = decimal_field(detail, "filled")
        .or_else(|_| decimal_field(detail, "filledSize"))
        .unwrap_or(Decimal::ZERO);
    let quantity = decimal_field(detail, "quantity").unwrap_or_else(|_| {
        filled + decimal_field(detail, "unfilledSize").unwrap_or(Decimal::ZERO)
    });
    let price = decimal_field(detail, "limitPrice")
        .ok()
        .filter(|d| *d > Decimal::ZERO);
    let client_order_id = detail
        .get("cliOrdId")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Order {
        id: id.to_string(),
        client_order_id,
        symbol,
        side: parse_side(side)?,
        order_type: kraken_futures_order_type(order_type),
        status: kraken_futures_status(status),
        quantity,
        filled_quantity: filled,
        price,
        average_price: None,
    })
}

/// Map a Kraken Futures instrument (`PI_XBTUSD`, `PF_XBTUSD`, `FI_XBTUSD_240628`)
/// to a [`Symbol`]: drop the product prefix and any dated suffix, then invert the
/// asset aliasing (`XBT` → `BTC`).
fn futures_instrument_symbol(instrument: &str) -> Symbol {
    let core = instrument
        .split_once('_')
        .map_or(instrument, |(_, rest)| rest);
    let core = core.split('_').next().unwrap_or(core);
    unmap_pair(core)
}

/// Parse one order object from a Kraken Futures `open_orders` feed frame. The
/// numeric `direction` is `0` = buy / `1` = sell; `qty`/`filled`/`limit_price`
/// are JSON numbers.
fn futures_feed_order(order: &serde_json::Value, status: OrderStatus) -> Result<Order> {
    let direction = order.get("direction").and_then(serde_json::Value::as_i64);
    let side = match direction {
        Some(0) => OrderSide::Buy,
        Some(1) => OrderSide::Sell,
        _ => {
            return Err(Error::Deserialization(
                "missing order direction".to_string(),
            ))
        }
    };
    let order_type = order
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let quantity = decimal_field(order, "qty").unwrap_or(Decimal::ZERO);
    let filled = decimal_field(order, "filled").unwrap_or(Decimal::ZERO);
    let price = decimal_field(order, "limit_price")
        .ok()
        .filter(|d| *d > Decimal::ZERO);
    let instrument = order
        .get("instrument")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let client_order_id = order
        .get("cli_ord_id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Order {
        id: order
            .get("order_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        client_order_id,
        symbol: futures_instrument_symbol(instrument),
        side,
        order_type: kraken_futures_order_type(order_type),
        status,
        quantity,
        filled_quantity: filled,
        price,
        average_price: None,
    })
}

/// Parse a Kraken Futures private feed frame (`open_orders` / `balances`) into
/// account events. Subscription acks, heartbeats and pure cancel-only frames
/// (which carry no order fields) yield no events.
fn parse_futures_private(frame: &str) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(frame).map_err(|e| Error::Deserialization(e.to_string()))?;
    let feed = value
        .get("feed")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match feed {
        "open_orders_snapshot" => value
            .get("orders")
            .and_then(serde_json::Value::as_array)
            .map_or_else(
                || Ok(Vec::new()),
                |orders| {
                    orders
                        .iter()
                        .map(|order| {
                            Ok(Event::OrderUpdate(futures_feed_order(
                                order,
                                OrderStatus::New,
                            )?))
                        })
                        .collect::<Result<Vec<_>>>()
                },
            ),
        "open_orders" => {
            let Some(order) = value.get("order") else {
                return Ok(Vec::new());
            };
            let status =
                if value.get("is_cancel").and_then(serde_json::Value::as_bool) == Some(true) {
                    OrderStatus::Canceled
                } else {
                    OrderStatus::New
                };
            Ok(vec![Event::OrderUpdate(futures_feed_order(order, status)?)])
        }
        "balances_snapshot" | "balances" => {
            let Some(holding) = value.get("holding").and_then(serde_json::Value::as_object) else {
                return Ok(Vec::new());
            };
            let balances = holding
                .iter()
                .map(|(asset, amount)| {
                    Ok(Balance {
                        asset: asset.clone(),
                        free: decimal_value(amount)?,
                        locked: Decimal::ZERO,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(vec![Event::BalanceUpdate(balances)])
        }
        _ => Ok(Vec::new()),
    }
}

fn map_error(error: &str) -> Error {
    if error.contains("Insufficient funds") {
        Error::InsufficientBalance
    } else if error.contains("Invalid key")
        || error.contains("Invalid signature")
        || error.contains("Permission denied")
        || error.contains("Invalid nonce")
    {
        Error::Auth(error.to_string())
    } else if error.contains("Rate limit") {
        Error::RateLimited { retry_after: None }
    } else if error.contains("Unknown order") {
        Error::NotFound(error.to_string())
    } else if error.contains("Unknown asset pair") {
        Error::InvalidSymbol(error.to_string())
    } else {
        Error::Exchange {
            code: "kraken".to_string(),
            message: error.to_string(),
        }
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

/// Build one `place_batch` outcome from an `AddOrderBatch` per-order entry: a
/// `txid` marks acceptance (mapped to a fresh [`OrderStatus::New`] order carrying
/// the request's own fields), an `error` marks a per-order rejection.
fn kraken_batch_order(request: &OrderRequest, entry: &serde_json::Value) -> Result<Order> {
    if let Some(error) = entry.get("error").and_then(serde_json::Value::as_str) {
        if !error.is_empty() {
            return Err(map_error(error));
        }
    }
    let txid = entry
        .get("txid")
        .and_then(|value| {
            value.as_str().map(str::to_string).or_else(|| {
                value
                    .as_array()
                    .and_then(|ids| ids.first())
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        })
        .ok_or_else(|| Error::Deserialization("missing txid in AddOrderBatch entry".to_string()))?;
    Ok(Order {
        id: txid,
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
        "pending" | "open" => Ok(OrderStatus::New),
        "closed" => Ok(OrderStatus::Filled),
        "canceled" => Ok(OrderStatus::Canceled),
        "expired" => Ok(OrderStatus::Expired),
        other => Err(Error::Deserialization(format!("unknown status {other:?}"))),
    }
}

fn nonzero(value: Decimal) -> Option<Decimal> {
    (value > Decimal::ZERO).then_some(value)
}

/// Render a [`Decimal`] as a JSON number, preserving its exact digits. Kraken's
/// v2 order API expects `order_qty` / `limit_price` as numbers.
fn json_number(value: Decimal) -> serde_json::Value {
    serde_json::from_str(&value.to_string())
        .unwrap_or_else(|_| serde_json::json!(value.to_string()))
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

/// Read `value[key][index]` as a decimal (Kraken ticker fields are arrays).
fn decimal_at(value: &serde_json::Value, key: &str, index: usize) -> Result<Decimal> {
    let field = value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.get(index))
        .ok_or_else(|| Error::Deserialization(format!("missing {key}[{index}]")))?;
    decimal_value(field)
}

/// REST depth levels: `[price, volume, timestamp]` string arrays.
fn rest_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
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

/// v2 WebSocket book levels: `{price, qty}` objects.
fn ws_levels(value: Option<&serde_json::Value>) -> Result<Vec<BookLevel>> {
    let array = value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Deserialization("missing depth levels".to_string()))?;
    array
        .iter()
        .map(|level| {
            Ok(BookLevel {
                price: decimal_field(level, "price")?,
                quantity: decimal_field(level, "qty")?,
            })
        })
        .collect()
}

fn parse_kline_row(row: &serde_json::Value) -> Result<Candle> {
    // Kraken OHLC: [time, open, high, low, close, vwap, volume, count].
    let arr = row
        .as_array()
        .ok_or_else(|| Error::Deserialization("kline row not an array".to_string()))?;
    if arr.len() < 7 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let ts = arr[0]
        .as_i64()
        .ok_or_else(|| Error::Deserialization("kline time not an integer".to_string()))?;
    let f = |i: usize| -> Result<f64> {
        let field = &arr[i];
        field
            .as_f64()
            .or_else(|| field.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| Error::Deserialization("kline field not a number".to_string()))
    };
    Candle::new(f(1)?, f(2)?, f(3)?, f(4)?, f(6)?, ts)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn descr_pair(order: &serde_json::Value) -> String {
    order
        .get("descr")
        .and_then(|d| d.get("pair"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn order_from_value(symbol: Symbol, id: &str, order: &serde_json::Value) -> Result<Order> {
    let descr = order.get("descr");
    let side = descr
        .and_then(|d| d.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let ordertype = descr
        .and_then(|d| d.get("ordertype"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let status = order
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let limit_price = descr
        .and_then(|d| d.get("price"))
        .map(decimal_value)
        .transpose()?
        .unwrap_or(Decimal::ZERO);
    Ok(Order {
        id: id.to_string(),
        client_order_id: order
            .get("cl_ord_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        symbol,
        side: parse_side(side)?,
        order_type: parse_order_type(ordertype)?,
        status: parse_status(status)?,
        quantity: decimal_field(order, "vol")?,
        filled_quantity: decimal_field(order, "vol_exec").unwrap_or(Decimal::ZERO),
        price: nonzero(limit_price),
        average_price: decimal_field(order, "price").ok().and_then(nonzero),
    })
}

fn parse_ws_message(text: &str) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let Some(channel) = value.get("channel").and_then(serde_json::Value::as_str) else {
        return Ok(Vec::new()); // status/heartbeat/ack
    };
    let msg_type = value.get("type").and_then(serde_json::Value::as_str);
    let empty = Vec::new();
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    match channel {
        "trade" => data
            .iter()
            .map(|t| {
                Ok(Event::Trade(TradePrint {
                    symbol: resolve_ws_symbol(t)?,
                    price: decimal_field(t, "price")?,
                    quantity: decimal_field(t, "qty")?,
                    aggressor: parse_side(
                        t.get("side")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                    )?,
                    timestamp: 0,
                }))
            })
            .collect(),
        "ticker" => data
            .iter()
            .map(|t| {
                Ok(Event::Ticker(Ticker {
                    symbol: resolve_ws_symbol(t)?,
                    last: decimal_field(t, "last")?,
                    bid: decimal_field(t, "bid").unwrap_or(Decimal::ZERO),
                    ask: decimal_field(t, "ask").unwrap_or(Decimal::ZERO),
                    volume: decimal_field(t, "volume").unwrap_or(Decimal::ZERO),
                }))
            })
            .collect(),
        "book" => data
            .iter()
            .map(|b| {
                let symbol = resolve_ws_symbol(b)?;
                let bids = ws_levels(b.get("bids"))?;
                let asks = ws_levels(b.get("asks"))?;
                if msg_type == Some("snapshot") {
                    Ok(Event::BookSnapshot(OrderBookSnapshot {
                        symbol,
                        last_update_id: 0,
                        bids,
                        asks,
                    }))
                } else {
                    Ok(Event::BookDelta(BookDelta {
                        symbol,
                        first_update_id: 0,
                        final_update_id: 0,
                        bids,
                        asks,
                    }))
                }
            })
            .collect(),
        // Private order-execution channel. An order update is emitted only when
        // the frame carries the order's static fields (the snapshot and full
        // new/amended updates); pure fill-delta frames that omit side/order_type
        // are not surfaced as standalone updates.
        "executions" => {
            let mut out = Vec::new();
            for exec in data {
                let (Some(side), Some(order_type)) = (
                    exec.get("side").and_then(serde_json::Value::as_str),
                    exec.get("order_type").and_then(serde_json::Value::as_str),
                ) else {
                    continue;
                };
                out.push(Event::OrderUpdate(ws_exec_order(exec, side, order_type)?));
            }
            Ok(out)
        }
        // Private balances channel. The v2 frame reports each asset's wallet
        // balance; a per-hold breakdown lives under `wallets` and is not surfaced,
        // so the locked amount is reported as zero.
        "balances" => {
            let balances = data
                .iter()
                .map(|entry| {
                    Ok(Balance {
                        asset: entry
                            .get("asset")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| {
                                Error::Deserialization("missing balance asset".to_string())
                            })?
                            .to_string(),
                        free: decimal_field(entry, "balance")?,
                        locked: Decimal::ZERO,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(vec![Event::BalanceUpdate(balances)])
        }
        _ => Ok(Vec::new()),
    }
}

/// Map a Kraken v2 `executions` order status to an [`OrderStatus`].
fn ws_exec_status(raw: &str) -> OrderStatus {
    match raw {
        "partially_filled" => OrderStatus::PartiallyFilled,
        "filled" => OrderStatus::Filled,
        "canceled" | "cancelled" => OrderStatus::Canceled,
        "expired" => OrderStatus::Expired,
        _ => OrderStatus::New, // new, pending_new, pending_cancel, ...
    }
}

/// Build an [`Order`] from a Kraken v2 `executions` frame that carries the
/// order's static fields.
fn ws_exec_order(exec: &serde_json::Value, side: &str, order_type: &str) -> Result<Order> {
    let id = exec
        .get("order_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing order_id".to_string()))?;
    let status = ws_exec_status(
        exec.get("order_status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("new"),
    );
    Ok(Order {
        id: id.to_string(),
        client_order_id: exec
            .get("cl_ord_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        symbol: resolve_ws_symbol(exec)?,
        side: parse_side(side)?,
        order_type: parse_order_type(order_type)?,
        status,
        quantity: decimal_field(exec, "order_qty").unwrap_or(Decimal::ZERO),
        filled_quantity: decimal_field(exec, "cum_qty").unwrap_or(Decimal::ZERO),
        price: decimal_field(exec, "limit_price").ok().and_then(nonzero),
        average_price: decimal_field(exec, "avg_price").ok().and_then(nonzero),
    })
}

fn resolve_ws_symbol(data: &serde_json::Value) -> Result<Symbol> {
    let raw = data
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization("missing ws symbol".to_string()))?;
    raw.parse()
        .map_err(|_| Error::Deserialization(format!("bad ws symbol {raw:?}")))
}

impl MarketData for Kraken {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Kraken::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Kraken::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Kraken::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Kraken::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Kraken::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Kraken::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Kraken::poll_events(self)
    }
}

impl Execution for Kraken {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Kraken::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Kraken::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Kraken::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Kraken::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Kraken::balances(self)
    }
}

impl Exchange for Kraken {
    fn name(&self) -> &'static str {
        "kraken"
    }
}

impl WsUserData for Kraken {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Kraken::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Kraken::keepalive_user_data(self)
    }
}

impl WsExecution for Kraken {
    fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        Kraken::place_order_ws(self, request)
    }
    fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Kraken::cancel_order_ws(self, symbol, order_id)
    }
}

impl Derivatives for Kraken {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Kraken::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Kraken::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Kraken::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Kraken::close_position(self, symbol)
    }
}

impl Kraken {
    /// Amend a resting spot order's price and/or volume in place
    /// (`/0/private/EditOrder`), then return the refreshed order (Kraken assigns a
    /// new txid on edit).
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is unknown, the amend is rejected, or the
    /// client targets Kraken Futures (a separate product).
    pub fn amend_order(
        &self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Kraken Futures edit is a separate endpoint".to_string(),
            });
        }
        let mut params: Vec<(&str, String)> = vec![
            ("txid", order_id.to_string()),
            ("pair", Self::wire_symbol(symbol)),
        ];
        if let Some(p) = new_price {
            params.push(("price", format_decimal(p)));
        }
        if let Some(q) = new_quantity {
            params.push(("volume", format_decimal(q)));
        }
        let result = self.signed_post("/0/private/EditOrder", &params)?;
        let new_txid = result
            .get("txid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("missing txid".to_string()))?
            .to_string();
        self.query_order(symbol, &new_txid)
    }

    /// Place several orders in one `AddOrderBatch` request. Kraken batches share a
    /// single `pair`, so the first order's symbol sets the pair for the batch; each
    /// order contributes its own indexed `orders[i][…]` fields. The per-order
    /// results are returned individually, so a partially-accepted batch keeps the
    /// successes.
    ///
    /// # Errors
    /// Returns an [`Error`] if any order is invalid, credentials are missing, or
    /// the request itself fails (a per-order rejection is carried in its own
    /// [`Result`], not the outer one).
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        for request in requests {
            request.validate()?;
        }
        // Kraken's `AddOrderBatch` carries one shared `pair` plus an indexed
        // `orders[i][…]` array — a form shape the flat helper cannot express.
        let mut params: Vec<(String, String)> =
            vec![("pair".to_string(), Self::wire_symbol(&requests[0].symbol))];
        for (index, request) in requests.iter().enumerate() {
            params.push((
                format!("orders[{index}][ordertype]"),
                order_type_str(request.order_type).to_string(),
            ));
            params.push((
                format!("orders[{index}][type]"),
                side_str(request.side).to_string(),
            ));
            params.push((
                format!("orders[{index}][volume]"),
                format_decimal(request.quantity),
            ));
            if let Some(price) = request.price {
                params.push((format!("orders[{index}][price]"), format_decimal(price)));
            }
            if request.post_only {
                params.push((format!("orders[{index}][oflags]"), "post".to_string()));
            }
            if let Some(id) = &request.client_order_id {
                params.push((format!("orders[{index}][cl_ord_id]"), id.clone()));
            }
        }
        let result = self.signed_post_pairs("/0/private/AddOrderBatch", &params)?;
        let orders = result
            .get("orders")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing orders in AddOrderBatch".to_string()))?;
        Ok(requests
            .iter()
            .zip(orders)
            .map(|(request, entry)| kraken_batch_order(request, entry))
            .collect())
    }

    /// Cancel several orders by id. The spot API cancels the whole set in one
    /// `CancelOrderBatch` request; the futures client has no such endpoint and
    /// cancels sequentially.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        if order_ids.is_empty() {
            return Ok(());
        }
        if self.is_futures() {
            for id in order_ids {
                self.cancel_order(symbol, id)?;
            }
            return Ok(());
        }
        let params: Vec<(String, String)> = order_ids
            .iter()
            .map(|id| ("orders[]".to_string(), id.clone()))
            .collect();
        self.signed_post_pairs("/0/private/CancelOrderBatch", &params)?;
        Ok(())
    }
}

impl AdvancedOrders for Kraken {
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Kraken::amend_order(self, symbol, order_id, new_price, new_quantity)
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Kraken::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Kraken::cancel_batch(self, symbol, order_ids)
    }
    /// Kraken has no OCO order-list (it uses conditional-close orders), so this
    /// returns an [`Error::Exchange`].
    fn place_oco(&mut self, _request: &OcoRequest) -> Result<Vec<Order>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Kraken has no OCO order-list; use conditional-close orders".to_string(),
        })
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

    fn client() -> (Kraken, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        (
            Kraken::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_client(now_ms: i64) -> (Kraken, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        // The secret is base64 (base64("secret") == "c2VjcmV0").
        let kraken = Kraken::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "c2VjcmV0"),
        )
        .with_clock(Box::new(move || now_ms));
        (kraken, mock)
    }

    fn futures_client() -> (Kraken, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        (
            Kraken::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts),
            mock,
        )
    }

    fn signed_futures_client(now_ms: i64) -> (Kraken, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let kraken = Kraken::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "c2VjcmV0"),
        )
        .with_clock(Box::new(move || now_ms));
        (kraken, mock)
    }

    fn signed_ws_client(now_ms: i64) -> (Kraken, Arc<MockHttpTransport>, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let kraken = Kraken::with_credentials(
            Box::new(ArcTransport(Arc::clone(&http))),
            &opts,
            Credentials::new("APIKEY", "c2VjcmV0"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (kraken, http, ws)
    }

    fn signed_futures_ws_client(now_ms: i64) -> (Kraken, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::UsdMFutures);
        let kraken = Kraken::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "c2VjcmV0"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (kraken, ws)
    }

    #[test]
    fn subscribe_user_data_fetches_token_and_streams_executions_and_balances() {
        let (mut kraken, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok","expires":900}}"#,
        );
        ws.push_connection(vec![
            Ok(Some(
                r#"{"method":"subscribe","success":true,"result":{"channel":"executions"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"channel":"executions","type":"snapshot","data":[{"order_id":"O123",
                "symbol":"BTC/USDT","side":"buy","order_type":"limit","order_status":"filled",
                "order_qty":1.0,"cum_qty":1.0,"limit_price":100,"avg_price":100,"cl_ord_id":"my"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"channel":"balances","type":"snapshot","data":[{"asset":"USDT",
                "balance":900.0}]}"#
                    .to_string(),
            )),
        ]);
        kraken.subscribe_user_data().unwrap();

        let reqs = http.recorded_requests();
        assert!(reqs[0].url.contains("/0/private/GetWebSocketsToken"));
        assert_eq!(ws.connected_urls()[0], "wss://ws-auth.kraken.com/v2");
        assert!(ws.sent()[0].contains(r#""channel":"executions""#));
        assert!(ws.sent()[0].contains(r#""token":"tok""#));
        assert!(ws.sent()[1].contains(r#""channel":"balances""#));

        let events = kraken.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "O123");
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
        assert_eq!(balances[0].asset, "USDT");
        assert_eq!(balances[0].free, dec!(900));
    }

    #[test]
    fn futures_subscribe_user_data_signs_the_challenge_and_subscribes_the_feeds() {
        let (mut kraken, ws) = signed_futures_ws_client(1000);
        ws.push_connection(vec![
            // The challenge response, then a subscription ack and two feed frames.
            Ok(Some(
                r#"{"event":"challenge","message":"challenge-uuid"}"#.to_string(),
            )),
            Ok(Some(
                r#"{"feed":"open_orders_snapshot","account":"acct","orders":[{"instrument":"PF_XBTUSD",
                "order_id":"O123","cli_ord_id":"my","direction":0,"type":"limit","qty":2.0,"filled":0.0,
                "limit_price":100.0}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"feed":"balances_snapshot","account":"acct","holding":{"USD":900.0}}"#.to_string(),
            )),
        ]);
        kraken.subscribe_user_data().unwrap();
        assert_eq!(ws.connected_urls()[0], "wss://futures.kraken.com/ws/v1");
        // First frame requests the challenge; the two subscribes carry the signed one.
        assert!(ws.sent()[0].contains(r#""event":"challenge""#));
        assert!(ws.sent()[0].contains(r#""api_key":"APIKEY""#));
        assert!(ws.sent()[1].contains(r#""feed":"open_orders""#));
        assert!(ws.sent()[1].contains(r#""original_challenge":"challenge-uuid""#));
        assert!(ws.sent()[1].contains(r#""signed_challenge""#));
        assert!(ws.sent()[2].contains(r#""feed":"balances""#));

        let events = kraken.poll_events();
        assert_eq!(events.len(), 2);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("first event must be an order update");
        };
        assert_eq!(order.id, "O123");
        assert_eq!(order.client_order_id.as_deref(), Some("my"));
        assert_eq!(order.symbol, Symbol::new("BTC", "USD"));
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.quantity, dec!(2));
        assert_eq!(order.price, Some(dec!(100)));
        let Event::BalanceUpdate(balances) = &events[1] else {
            panic!("second event must be a balance update");
        };
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].asset, "USD");
        assert_eq!(balances[0].free, dec!(900));
    }

    #[test]
    fn futures_open_orders_update_reports_a_cancel() {
        let (mut kraken, ws) = signed_futures_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"event":"challenge","message":"challenge-uuid"}"#.to_string(),
            )),
            Ok(Some(
                r#"{"feed":"open_orders","is_cancel":true,"order":{"instrument":"PF_ETHUSD",
                "order_id":"O9","direction":1,"type":"limit","qty":1.0,"filled":0.0,"limit_price":50.0}}"#
                    .to_string(),
            )),
        ]);
        kraken.subscribe_user_data().unwrap();
        let events = kraken.poll_events();
        assert_eq!(events.len(), 1);
        let Event::OrderUpdate(order) = &events[0] else {
            panic!("event must be an order update");
        };
        assert_eq!(order.id, "O9");
        assert_eq!(order.symbol, Symbol::new("ETH", "USD"));
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.status, OrderStatus::Canceled);
    }

    #[test]
    fn futures_keepalive_user_data_is_a_noop() {
        // Kraken Futures keeps the feed alive server-side, so keepalive sends nothing.
        let (mut kraken, ws) = signed_futures_ws_client(1000);
        ws.push_connection(vec![Ok(Some(
            r#"{"event":"challenge","message":"challenge-uuid"}"#.to_string(),
        ))]);
        kraken.subscribe_user_data().unwrap();
        let sent_after_subscribe = ws.sent().len();
        kraken.keepalive_user_data().unwrap();
        assert_eq!(ws.sent().len(), sent_after_subscribe);
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut kraken, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok","expires":900}}"#,
        );
        ws.push_connection(vec![]);
        kraken.subscribe_user_data().unwrap();
        kraken.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == r#"{"method":"ping"}"#));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut kraken, _http, ws) = signed_ws_client(1000);
        kraken.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_a_fresh_token() {
        let (mut kraken, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok-1","expires":900}}"#,
        );
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        kraken.subscribe_user_data().unwrap();
        // The reconnect re-fetches a fresh single-use token over REST.
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok-2","expires":900}}"#,
        );

        let events = kraken.poll_events();
        assert!(events.contains(&Event::Disconnected));
        assert!(events.contains(&Event::Reconnected));
        // Two token POSTs (initial + reconnect) and a second WS connection whose
        // subscribe carries the fresh token.
        let tokens = http
            .recorded_requests()
            .into_iter()
            .filter(|r| r.url.contains("/0/private/GetWebSocketsToken"))
            .count();
        assert_eq!(tokens, 2);
        assert_eq!(ws.connected_urls().len(), 2);
        assert!(ws.sent().iter().any(|f| f.contains(r#""token":"tok-2""#)));
    }

    #[test]
    fn place_and_cancel_order_over_ws() {
        let (mut kraken, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok","expires":900}}"#,
        );
        ws.push_connection(vec![
            Ok(Some(
                r#"{"method":"add_order","req_id":1000,"success":true,
                "result":{"order_id":"O123","order_userref":0}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"method":"cancel_order","req_id":1000,"success":true,
                "result":{"order_id":"O123"}}"#
                    .to_string(),
            )),
        ]);
        let order = kraken
            .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "O123");
        assert_eq!(order.status, OrderStatus::New);
        let reqs = http.recorded_requests();
        assert!(reqs[0].url.contains("/0/private/GetWebSocketsToken"));
        assert_eq!(ws.connected_urls()[0], "wss://ws-auth.kraken.com/v2");
        assert!(ws.sent()[0].contains(r#""method":"add_order""#));
        assert!(ws.sent()[0].contains(r#""token":"tok""#));
        assert!(ws.sent()[0].contains(r#""symbol":"BTC/USDT""#));

        kraken.cancel_order_ws(&symbol(), "O123").unwrap();
        assert!(ws.sent()[1].contains(r#""method":"cancel_order""#));
        assert!(ws.sent()[1].contains(r#""order_id":["O123"]"#));
    }

    #[test]
    fn ws_order_surfaces_rejection() {
        let (mut kraken, http, ws) = signed_ws_client(1000);
        http.push_json(
            200,
            r#"{"error":[],"result":{"token":"tok","expires":900}}"#,
        );
        ws.push_connection(vec![Ok(Some(
            r#"{"method":"add_order","req_id":1000,"success":false,
            "error":"EOrder:Insufficient funds"}"#
                .to_string(),
        ))]);
        assert!(matches!(
            kraken
                .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn ws_order_rejects_the_futures_client() {
        let (mut kraken, _mock) = futures_client();
        assert!(matches!(
            kraken
                .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn amend_order_edits_then_reads_new_txid() {
        let (kraken, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"error":[],"result":{"status":"ok","txid":"NEW1"}}"#,
        );
        mock.push_json(
            200,
            r#"{"error":[],"result":{"NEW1":{"status":"open","vol":"2","vol_exec":"0",
            "price":"101","descr":{"pair":"XBTUSDT","type":"buy","ordertype":"limit","price":"101"}}}}"#,
        );
        let order = kraken
            .amend_order(&symbol(), "OLD1", Some(dec!(101)), Some(dec!(2)))
            .unwrap();
        assert_eq!(order.id, "NEW1");
        assert_eq!(order.quantity, dec!(2));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/0/private/EditOrder"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains("txid=OLD1"));
        assert!(body.contains("volume=2"));
        assert!(body.contains("price=101"));
    }

    #[test]
    fn cancel_batch_uses_the_native_endpoint() {
        let (kraken, mock) = signed_client(1000);
        mock.push_json(200, r#"{"error":[],"result":{"count":2}}"#);
        kraken
            .cancel_batch(&symbol(), &["OID1".to_string(), "OID2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        // One CancelOrderBatch request carries both ids as a form array.
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/0/private/CancelOrderBatch"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains("orders[]=OID1"));
        assert!(body.contains("orders[]=OID2"));
    }

    #[test]
    fn place_batch_uses_the_indexed_add_order_batch_form() {
        let (mut kraken, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"error":[],"result":{"orders":[{"txid":"OID1","descr":{"order":"buy"}},
            {"error":"EOrder:Insufficient funds"}]}}"#,
        );
        let results = AdvancedOrders::place_batch(
            &mut kraken,
            &[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(2), dec!(90)),
            ],
        )
        .unwrap();
        // The first order is accepted; the second carries its own rejection.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().id, "OID1");
        assert!(matches!(
            results[1].as_ref().unwrap_err(),
            Error::InsufficientBalance
        ));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/0/private/AddOrderBatch"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains("pair=XBTUSDT"));
        assert!(body.contains("orders[0][ordertype]=limit"));
        assert!(body.contains("orders[0][type]=buy"));
        assert!(body.contains("orders[0][volume]=1"));
        assert!(body.contains("orders[0][price]=100"));
        assert!(body.contains("orders[1][volume]=2"));
        assert!(body.contains("orders[1][price]=90"));
    }

    #[test]
    fn place_oco_is_unsupported() {
        let (mut kraken, _mock) = signed_client(1000);
        assert!(matches!(
            AdvancedOrders::place_oco(
                &mut kraken,
                &OcoRequest::new(symbol(), OrderSide::Sell, dec!(1), dec!(110), dec!(95))
            )
            .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn futures_symbol_is_pf_usd_settled() {
        assert_eq!(Kraken::futures_symbol(&symbol()), "PF_XBTUSD");
        assert_eq!(
            Kraken::futures_symbol(&Symbol::new("ETH", "USD")),
            "PF_ETHUSD"
        );
    }

    #[test]
    fn futures_ticker_filters_by_symbol_on_the_futures_host() {
        let (kraken, mock) = futures_client();
        mock.push_json(
            200,
            r#"{"result":"success","tickers":[
            {"symbol":"pf_ethusd","last":3000,"bid":2999,"ask":3001,"vol24h":10},
            {"symbol":"pf_xbtusd","last":20000.5,"bid":19999,"ask":20001,"vol24h":1234}]}"#,
        );
        let ticker = kraken.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.volume, dec!(1234));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("futures.kraken.com/derivatives/api/v3/tickers"));
    }

    #[test]
    fn futures_order_book_numeric_pairs() {
        let (kraken, mock) = futures_client();
        mock.push_json(
            200,
            r#"{"result":"success","orderBook":{"bids":[[100.0,1.0]],"asks":[[101.0,2.0]]}}"#,
        );
        let book = kraken.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101), dec!(2)));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/derivatives/api/v3/orderbook?symbol=PF_XBTUSD"));
    }

    #[test]
    fn futures_klines_from_charts_api() {
        let (kraken, mock) = futures_client();
        mock.push_json(
            200,
            r#"{"candles":[{"time":1700000000,"open":100,"high":110,"low":95,"close":105,"volume":12}]}"#,
        );
        let candles = kraken.klines(&symbol(), "1h", 1).unwrap();
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/api/charts/v1/trade/PF_XBTUSD/1h"));
    }

    #[test]
    fn futures_market_order_uses_sendorder() {
        let (kraken, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"result":"success","sendStatus":{"order_id":"abc-1","status":"placed"}}"#,
        );
        let order = kraken
            .place_order(&OrderRequest::market_buy(symbol(), dec!(2)))
            .unwrap();
        assert_eq!(order.id, "abc-1");
        assert_eq!(order.status, OrderStatus::New);
        let req = &mock.recorded_requests()[0];
        assert!(req
            .url
            .contains("futures.kraken.com/derivatives/api/v3/sendorder"));
        let body = req.body.as_ref().unwrap();
        assert!(body.contains("orderType=mkt"));
        assert!(body.contains("symbol=PF_XBTUSD"));
        assert!(body.contains("side=buy"));
        assert!(body.contains("reduceOnly=false"));
        // Reconstruct the Authent signature over SHA256(postData ++ nonce ++ path).
        let concat = format!("{body}1000/api/v3/sendorder");
        let expected =
            hmac_sha512_base64_with_b64_secret("c2VjcmV0", &sha256(concat.as_bytes())).unwrap();
        let sign = req
            .headers
            .iter()
            .find(|(k, _)| k == "Authent")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(sign, expected);
    }

    #[test]
    fn futures_query_order_uses_orders_status() {
        let (kraken, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"result":"success","orders":[{"order_id":"o-1","status":"FULLY_EXECUTED",
            "order":{"orderType":"lmt","side":"buy","quantity":3,"filled":3,"limitPrice":100,
            "symbol":"pf_xbtusd","cliOrdId":""}}]}"#,
        );
        let order = kraken.query_order(&symbol(), "o-1").unwrap();
        assert_eq!(order.id, "o-1");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.quantity, dec!(3));
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/derivatives/api/v3/orders/status"));
    }

    #[test]
    fn futures_cancel_and_open_orders_use_derivatives_endpoints() {
        let (kraken, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"result":"success","cancelStatus":{"status":"cancelled"}}"#,
        );
        kraken.cancel_order(&symbol(), "o-1").unwrap();
        mock.push_json(
            200,
            r#"{"result":"success","openOrders":[{"order_id":"o-2","symbol":"pf_xbtusd",
            "side":"sell","orderType":"lmt","limitPrice":21000,"filledSize":1,"unfilledSize":2,
            "status":"partiallyFilled","cliOrdId":""}]}"#,
        );
        let orders = kraken.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);
        assert_eq!(orders[0].status, OrderStatus::PartiallyFilled);
        assert_eq!(orders[0].quantity, dec!(3)); // filledSize + unfilledSize
        assert_eq!(orders[0].filled_quantity, dec!(1));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/derivatives/api/v3/cancelorder"));
        assert!(reqs[1].url.contains("/derivatives/api/v3/openorders"));
    }

    #[test]
    fn derivatives_positions_parse() {
        let (mut kraken, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"result":"success"}"#);
        kraken.set_leverage(&symbol(), 5).unwrap();
        mock.push_json(
            200,
            r#"{"result":"success","openPositions":[
            {"symbol":"pf_xbtusd","side":"long","size":3,"price":20000}]}"#,
        );
        let positions = Derivatives::positions(&mut kraken, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USD"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(3));
        assert_eq!(positions[0].entry_price, dec!(20000));
        assert_eq!(positions[0].leverage, dec!(5)); // recorded via set_leverage
        assert_eq!(positions[0].margin_mode, MarginMode::Cross);
        let reqs = mock.recorded_requests();
        assert!(reqs[0]
            .url
            .contains("/derivatives/api/v3/leveragepreferences"));
        assert_eq!(reqs[0].method, HttpMethod::Put);
        assert!(reqs[1].url.contains("/derivatives/api/v3/openpositions"));
    }

    #[test]
    fn set_margin_mode_isolated_is_unsupported() {
        let (kraken, _mock) = signed_futures_client(1000);
        assert!(kraken.set_margin_mode(&symbol(), MarginMode::Cross).is_ok());
        assert!(matches!(
            kraken
                .set_margin_mode(&symbol(), MarginMode::Isolated)
                .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn close_position_is_reduce_only_opposite() {
        let (mut kraken, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"result":"success","openPositions":[
            {"symbol":"pf_xbtusd","side":"long","size":3,"price":20000}]}"#,
        );
        mock.push_json(
            200,
            r#"{"result":"success","sendStatus":{"order_id":"c-9","status":"placed"}}"#,
        );
        Derivatives::close_position(&mut kraken, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[1].body.as_ref().unwrap();
        assert!(body.contains("side=sell"));
        assert!(body.contains("reduceOnly=true"));
    }

    #[test]
    fn futures_balances_from_flex_currencies() {
        let (kraken, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"result":"success","accounts":{"flex":{"currencies":{
            "USD":{"quantity":1000,"available":800}}}}}"#,
        );
        let bals = kraken.balances().unwrap();
        assert_eq!(bals[0].asset, "USD");
        assert_eq!(bals[0].free, dec!(800));
        assert_eq!(bals[0].locked, dec!(200));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/derivatives/api/v3/accounts"));
    }

    #[test]
    fn futures_error_envelope_maps() {
        let (kraken, mock) = futures_client();
        mock.push_json(200, r#"{"result":"error","error":"insufficientFunds"}"#);
        assert!(matches!(
            kraken.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn wire_symbol_maps_btc_to_xbt() {
        assert_eq!(Kraken::wire_symbol(&symbol()), "XBTUSDT");
        assert_eq!(Kraken::wire_symbol(&Symbol::new("ETH", "USD")), "ETHUSD");
    }

    #[test]
    fn ticker_takes_single_result() {
        let (kraken, mock) = client();
        mock.push_json(
            200,
            r#"{"error":[],"result":{"XXBTZUSD":{"a":["20001","1","1"],"b":["19999","1","1"],
            "c":["20000.5","0.1"],"v":["10","1234"]}}}"#,
        );
        let ticker = kraken.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.volume, dec!(1234));
    }

    #[test]
    fn klines_skip_last_key() {
        let (kraken, mock) = client();
        mock.push_json(
            200,
            r#"{"error":[],"result":{"XXBTZUSD":[
            [1700000000,"100","110","95","105","103","12","5"],
            [1700003600,"105","106","104","105.5","105","2","3"]],"last":1700003600}}"#,
        );
        let candles = kraken.klines(&symbol(), "1h", 2).unwrap();
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].timestamp, 1_700_000_000);
        assert!((candles[0].high - 110.0).abs() < 1e-9);
    }

    #[test]
    fn order_book_parses() {
        let (kraken, mock) = client();
        mock.push_json(
            200,
            r#"{"error":[],"result":{"XXBTZUSD":{"bids":[["100","1","1700"]],
            "asks":[["101","2","1700"]]}}}"#,
        );
        let book = kraken.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.bids[0], BookLevel::new(dec!(100), dec!(1)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101), dec!(2)));
    }

    #[test]
    fn error_array_maps() {
        let (kraken, mock) = client();
        mock.push_json(
            200,
            r#"{"error":["EOrder:Insufficient funds"],"result":{}}"#,
        );
        assert!(matches!(
            kraken.ticker(&symbol()).unwrap_err(),
            Error::InsufficientBalance
        ));
    }

    #[test]
    fn place_order_signs_with_sha256_and_hmac512() {
        let (kraken, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"error":[],"result":{"txid":["OABC-123"],"descr":{"order":"x"}}}"#,
        );
        let order = kraken
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "OABC-123");

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let body = req.body.as_ref().unwrap();
        assert!(body.starts_with("nonce=1000&"));
        // Reconstruct API-Sign.
        let path = "/0/private/AddOrder";
        let mut message = path.as_bytes().to_vec();
        message.extend_from_slice(&sha256(format!("1000{body}").as_bytes()));
        let expected = hmac_sha512_base64_with_b64_secret("c2VjcmV0", &message).unwrap();
        let sign = req
            .headers
            .iter()
            .find(|(k, _)| k == "API-Sign")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(sign, expected);
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "API-Key" && v == "APIKEY"));
    }

    #[test]
    fn query_order_parses() {
        let (kraken, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"error":[],"result":{"OABC-123":{"status":"closed","vol":"2","vol_exec":"2",
            "price":"100","descr":{"pair":"XBTUSDT","type":"sell","ordertype":"limit","price":"100"}}}}"#,
        );
        let order = kraken.query_order(&symbol(), "OABC-123").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.filled_quantity, dec!(2));
        assert_eq!(order.average_price, Some(dec!(100)));
    }

    #[test]
    fn balances_free_minus_hold() {
        let (kraken, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"error":[],"result":{"USDT":{"balance":"126","hold_trade":"25.5"}}}"#,
        );
        let bals = kraken.balances().unwrap();
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].free, dec!(100.5));
        assert_eq!(bals[0].locked, dec!(25.5));
    }

    #[test]
    fn signed_requires_credentials() {
        let (kraken, _) = client();
        assert!(matches!(
            kraken.balances().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn ws_v2_parses_trade_and_book() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"channel":"trade","type":"update","data":[
                {"symbol":"BTC/USDT","side":"buy","price":100.0,"qty":0.5}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"channel":"book","type":"snapshot","data":[
                {"symbol":"BTC/USDT","bids":[{"price":100.0,"qty":1.0}],"asks":[{"price":101.0,"qty":2.0}]}]}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"channel":"status","data":[]}"#.to_string())),
        ]);
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(crate::MarketType::Spot);
        let mut kraken = Kraken::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
        kraken.subscribe_trades(&symbol()).unwrap();
        assert!(ws.sent()[0].contains(r#""symbol":["BTC/USDT"]"#));
        assert_eq!(ws.connected_urls()[0], "wss://ws.kraken.com/v2");

        let events = kraken.poll_events();
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
        let (kraken, mock) = signed_client(1000);
        mock.push_json(200, r#"{"error":[],"result":{"txid":["O1"],"descr":{}}}"#);
        let mut exchange: Box<dyn Exchange> = Box::new(kraken);
        assert_eq!(exchange.name(), "kraken");
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
