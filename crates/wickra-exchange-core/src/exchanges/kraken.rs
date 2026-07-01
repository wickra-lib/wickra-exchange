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
//! `sendorder`/`openpositions`/`leveragepreferences`. Documented gaps: the WS
//! stream stays on the spot v2 feed, `query_order`/`cancel_order`/`open_orders`
//! keep the spot shape, `openpositions` omits mark price and unrealized PnL, and
//! `set_margin_mode(Isolated)` is unsupported within the flex (cross) account.

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType};
use crate::positions::{Position, PositionSide};
use crate::signing::{hmac_sha512_base64_with_b64_secret, sha256};
use crate::symbol::Symbol;
use crate::traits::{Derivatives, Exchange, Execution, MarketData};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};
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
        self.signed_post("/0/private/CancelOrder", &[("txid", order_id.to_string())])?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
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
        _ => Ok(Vec::new()),
    }
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
