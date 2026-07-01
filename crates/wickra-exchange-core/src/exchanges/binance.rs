//! Binance — the reference exchange implementation.
//!
//! This module is generic over the injected [`HttpTransport`], so the entire
//! request-build → parse → normalise path is exercised offline against
//! [`MockHttpTransport`] with recorded Binance responses. Only the production
//! wiring of a real socket lives elsewhere.
//!
//! Covered here: the public REST market data (ticker, klines, depth), the
//! URL/symbol mapping, the Binance error taxonomy, HMAC-SHA256 signed execution
//! (place/cancel/query/open orders, balances) with `exchangeInfo` filter
//! validation, and the pull-based WebSocket market streams (trade/depth/ticker
//! subscribe + `poll_events`). The user-data stream (listenKey) and the real
//! socket adapter land in a later slice.
//!
//! Binance is also the reference for [`AdvancedOrders`]: self-trade-prevention
//! (the `stp` field maps to `selfTradePreventionMode`), amend (native
//! `PUT /fapi/v1/order` on futures, atomic `cancelReplace` on spot), batch place
//! and cancel (native single-call `/fapi/v1/batchOrders` on futures, sequential
//! on spot), and OCO brackets (`/api/v3/order/oco`, a spot order-list;
//! unsupported on USDⓈ-M futures, which has no order-list).

use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::instruments::{Instrument, InstrumentCache, InstrumentFilters};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType, SelfTradePrevention};
use crate::positions::{Position, PositionSide};
use crate::signing::hmac_sha256_hex;
use crate::symbol::Symbol;
use crate::traits::{AdvancedOrders, Derivatives, Exchange, Execution, MarketData};
use crate::transport::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, WsConnection, WsTransport,
};
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

/// A Binance client over an injected HTTP transport.
pub struct Binance {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    market_type: MarketType,
    testnet: bool,
    credentials: Option<Credentials>,
    recv_window_ms: u64,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    connection: Option<Box<dyn WsConnection>>,
    subscriptions: Vec<(String, Symbol)>,
    sub_id: u64,
    sub_messages: Vec<String>,
    instruments: InstrumentCache,
}

impl Binance {
    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: rest_base_url(options.market_type, options.testnet).to_string(),
            market_type: options.market_type,
            testnet: options.testnet,
            credentials,
            recv_window_ms: options.recv_window_ms,
            now_ms: Box::new(system_now_ms),
            connection: None,
            subscriptions: Vec::new(),
            sub_id: 0,
            sub_messages: Vec::new(),
            instruments: InstrumentCache::new(),
        }
    }

    /// Build a public (unauthenticated) Binance client over the given transport.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Binance client for signed endpoints.
    #[must_use]
    pub fn with_credentials(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Credentials,
    ) -> Self {
        Self::build(http, options, Some(credentials))
    }

    /// Override the timestamp source (used for deterministic signing in tests).
    #[must_use]
    pub fn with_clock(mut self, now_ms: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now_ms = now_ms;
        self
    }

    /// Attach a WebSocket transport, enabling the streaming subscriptions.
    #[must_use]
    pub fn with_ws(mut self, ws: Box<dyn WsTransport>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// The market type this client is configured for.
    #[must_use]
    pub fn market_type(&self) -> MarketType {
        self.market_type
    }

    /// Whether this client targets the USDⓈ-M futures market (fapi paths) rather
    /// than spot (api/v3 paths).
    fn is_futures(&self) -> bool {
        matches!(self.market_type, MarketType::UsdMFutures)
    }

    /// The single-order endpoint (place/cancel/query) for this market.
    fn order_path(&self) -> &'static str {
        if self.is_futures() {
            "/fapi/v1/order"
        } else {
            "/api/v3/order"
        }
    }

    /// The open-orders endpoint for this market.
    fn open_orders_path(&self) -> &'static str {
        if self.is_futures() {
            "/fapi/v1/openOrders"
        } else {
            "/api/v3/openOrders"
        }
    }

    /// The Binance wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTCUSDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated()
    }

    /// A 24-hour ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!("symbol={}", Self::wire_symbol(symbol));
        if self.is_futures() {
            // The futures 24-hour ticker carries no bid/ask, so combine it with
            // the book ticker for the top-of-book quote.
            let stats: RawFuturesTicker = deserialize(&self.get("/fapi/v1/ticker/24hr", &query)?)?;
            let book: RawBookTicker =
                deserialize(&self.get("/fapi/v1/ticker/bookTicker", &query)?)?;
            return Ok(Ticker {
                symbol: symbol.clone(),
                last: parse_decimal(&stats.last_price)?,
                bid: parse_decimal(&book.bid_price)?,
                ask: parse_decimal(&book.ask_price)?,
                volume: parse_decimal(&stats.volume)?,
            });
        }
        let raw: RawTicker = deserialize(&self.get("/api/v3/ticker/24hr", &query)?)?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&raw.last_price)?,
            bid: parse_decimal(&raw.bid_price)?,
            ask: parse_decimal(&raw.ask_price)?,
            volume: parse_decimal(&raw.volume)?,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (e.g. `"1m"`, `"1h"`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "symbol={}&interval={interval}&limit={limit}",
            Self::wire_symbol(symbol)
        );
        let path = if self.is_futures() {
            "/fapi/v1/klines"
        } else {
            "/api/v3/klines"
        };
        let body = self.get(path, &query)?;
        let rows: Vec<Vec<serde_json::Value>> = deserialize(&body)?;
        rows.iter().map(|row| parse_kline_row(row)).collect()
    }

    /// A depth snapshot of `symbol` up to `depth` levels per side.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn order_book(&self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        let query = format!("symbol={}&limit={depth}", Self::wire_symbol(symbol));
        let path = if self.is_futures() {
            "/fapi/v1/depth"
        } else {
            "/api/v3/depth"
        };
        let body = self.get(path, &query)?;
        let raw: RawDepth = deserialize(&body)?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.last_update_id,
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
        })
    }

    /// Fetch `exchangeInfo` and populate the instrument/filter cache, so that
    /// [`place_order`](Self::place_order) validates against the venue's per-symbol
    /// filters (lot size, price tick, min-notional).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn load_instruments(&mut self) -> Result<()> {
        let path = if self.is_futures() {
            "/fapi/v1/exchangeInfo"
        } else {
            "/api/v3/exchangeInfo"
        };
        let body = self.get(path, "")?;
        let raw: RawExchangeInfo = deserialize(&body)?;
        let now = (self.now_ms)();
        let instruments: Vec<Instrument> = raw.symbols.iter().map(parse_instrument).collect();
        self.instruments.replace(instruments, now);
        Ok(())
    }

    /// The cached instrument metadata for `symbol`, if [`load_instruments`](Self::load_instruments)
    /// has been called.
    #[must_use]
    pub fn instrument(&self, symbol: &Symbol) -> Option<&Instrument> {
        self.instruments.get(symbol)
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured,
    /// or a transport error if the connection or subscription fails.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "trade")
    }

    /// Subscribe to the order-book (diff-depth) stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "depth")
    }

    /// Subscribe to the 24-hour ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        self.subscribe(symbol, "ticker")
    }

    /// Open the connection if needed, send a SUBSCRIBE for `<symbol>@<channel>`,
    /// and register the symbol for wire-name resolution.
    fn subscribe(&mut self, symbol: &Symbol, channel: &str) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect(ws_base_url(self.market_type, self.testnet))?;
            self.connection = Some(connection);
        }
        self.sub_id += 1;
        let stream = format!("{}@{channel}", wire.to_lowercase());
        let message = format!(
            r#"{{"method":"SUBSCRIBE","params":["{stream}"],"id":{}}}"#,
            self.sub_id
        );
        self.connection
            .as_mut()
            .expect("connection just ensured")
            .send(&message)?;
        if !self.subscriptions.iter().any(|(w, _)| w == &wire) {
            self.subscriptions.push((wire, symbol.clone()));
        }
        if !self.sub_messages.contains(&message) {
            self.sub_messages.push(message);
        }
        Ok(())
    }

    /// Drain all stream events available since the last call. Non-blocking:
    /// returns an empty vector when nothing is pending or no stream is open.
    /// Frames that fail to parse are skipped.
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
                if let Ok(Some(event)) = parse_ws_message(&frame, &resolve) {
                    events.push(event);
                }
            }
        }
        let url = ws_base_url(self.market_type, self.testnet);
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Place an order. The order is validated locally first, then sent signed.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        // When exchangeInfo has been loaded, reject filter violations before the
        // round trip.
        if let Some(instrument) = self.instruments.get(&request.symbol) {
            instrument
                .filters
                .validate(request.quantity, request.price)?;
        }
        let type_str = if request.post_only && request.order_type == OrderType::Limit {
            "LIMIT_MAKER"
        } else {
            order_type_str(request.order_type)
        };
        let mut params = format!(
            "symbol={}&side={}&type={type_str}&quantity={}",
            Self::wire_symbol(&request.symbol),
            side_str(request.side),
            format_decimal(request.quantity),
        );
        if let Some(price) = request.price {
            params.push_str("&price=");
            params.push_str(&format_decimal(price));
        }
        if let Some(stop) = request.stop_price {
            params.push_str("&stopPrice=");
            params.push_str(&format_decimal(stop));
        }
        if matches!(type_str, "LIMIT" | "STOP_LOSS_LIMIT" | "TAKE_PROFIT_LIMIT") {
            params.push_str("&timeInForce=");
            params.push_str(tif_str(request.time_in_force));
        }
        if let Some(id) = &request.client_order_id {
            params.push_str("&newClientOrderId=");
            params.push_str(id);
        }
        if request.reduce_only {
            params.push_str("&reduceOnly=true");
        }
        if let Some(mode) = stp_str(request.stp) {
            params.push_str("&selfTradePreventionMode=");
            params.push_str(mode);
        }
        let body = self.signed_request(HttpMethod::Post, self.order_path(), &params)?;
        parse_order(&request.symbol, &body)
    }

    /// Cancel an open order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the venue rejects it.
    pub fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let params = format!("symbol={}&orderId={order_id}", Self::wire_symbol(symbol));
        self.signed_request(HttpMethod::Delete, self.order_path(), &params)?;
        Ok(())
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let params = format!("symbol={}&orderId={order_id}", Self::wire_symbol(symbol));
        let body = self.signed_request(HttpMethod::Get, self.order_path(), &params)?;
        parse_order(symbol, &body)
    }

    /// Account balances (assets with a non-zero free or locked amount are
    /// included as the venue reports them).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        if self.is_futures() {
            let body = self.signed_request(HttpMethod::Get, "/fapi/v2/balance", "")?;
            let raw: Vec<RawFuturesBalance> = deserialize(&body)?;
            return raw
                .iter()
                .map(|b| {
                    let total = parse_decimal(&b.balance)?;
                    let free = parse_decimal(&b.available_balance)?;
                    Ok(Balance {
                        asset: b.asset.clone(),
                        free,
                        locked: total - free,
                    })
                })
                .collect();
        }
        let body = self.signed_request(HttpMethod::Get, "/api/v3/account", "")?;
        let raw: RawAccount = deserialize(&body)?;
        raw.balances
            .iter()
            .map(|b| {
                Ok(Balance {
                    asset: b.asset.clone(),
                    free: parse_decimal(&b.free)?,
                    locked: parse_decimal(&b.locked)?,
                })
            })
            .collect()
    }

    /// All open orders, optionally filtered to one `symbol`. When unfiltered, the
    /// venue reports each order's wire symbol, which is mapped back to a canonical
    /// [`Symbol`] via the known quote-asset suffixes.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let params = match symbol {
            Some(s) => format!("symbol={}", Self::wire_symbol(s)),
            None => String::new(),
        };
        let body = self.signed_request(HttpMethod::Get, self.open_orders_path(), &params)?;
        let raws: Vec<RawOrder> = deserialize(&body)?;
        raws.iter()
            .map(|raw| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| split_wire_symbol(&raw.symbol));
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Issue a GET and return the body, mapping non-2xx responses onto the error
    /// taxonomy.
    fn get(&self, path: &str, query: &str) -> Result<String> {
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let response = self.http.execute(&HttpRequest::get(url))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(map_error(&response))
        }
    }

    /// Sign `params` (HMAC-SHA256 over the query with `recvWindow` + `timestamp`)
    /// and issue the request with the API-key header.
    fn signed_request(&self, method: HttpMethod, path: &str, params: &str) -> Result<String> {
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let timestamp = (self.now_ms)();
        let payload = if params.is_empty() {
            format!("recvWindow={}&timestamp={timestamp}", self.recv_window_ms)
        } else {
            format!(
                "{params}&recvWindow={}&timestamp={timestamp}",
                self.recv_window_ms
            )
        };
        let signature = hmac_sha256_hex(creds.api_secret.as_bytes(), payload.as_bytes());
        let url = format!("{}{path}?{payload}&signature={signature}", self.rest_base);
        let request =
            HttpRequest::new(method, url).with_header("X-MBX-APIKEY", creds.api_key.clone());
        let response = self.http.execute(&request)?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(map_error(&response))
        }
    }
}

// The trait surface delegates to the inherent methods (fully qualified to avoid
// resolving back to the trait method), giving a `Box<dyn Exchange>` for the factory.
impl MarketData for Binance {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Binance::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Binance::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Binance::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Binance::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Binance::poll_events(self)
    }
}

impl Execution for Binance {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Binance::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Binance::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Binance::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Binance::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Binance::balances(self)
    }
}

impl Exchange for Binance {
    fn name(&self) -> &'static str {
        "binance"
    }
}

impl Binance {
    /// Open positions on the USDⓈ-M futures account (`/fapi/v2/positionRisk`).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let params =
            symbol.map_or_else(String::new, |s| format!("symbol={}", Self::wire_symbol(s)));
        let body = self.signed_request(HttpMethod::Get, "/fapi/v2/positionRisk", &params)?;
        parse_positions(&body)
    }

    /// Set the leverage for `symbol` (`/fapi/v1/leverage`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let params = format!("symbol={}&leverage={leverage}", Self::wire_symbol(symbol));
        self.signed_request(HttpMethod::Post, "/fapi/v1/leverage", &params)?;
        Ok(())
    }

    /// Set the margin mode for `symbol` (`/fapi/v1/marginType`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let margin = match mode {
            MarginMode::Isolated => "ISOLATED",
            MarginMode::Cross => "CROSSED",
        };
        let params = format!("symbol={}&marginType={margin}", Self::wire_symbol(symbol));
        self.signed_request(HttpMethod::Post, "/fapi/v1/marginType", &params)?;
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

    /// Amend a resting order's price and/or quantity. Binance futures amends in
    /// place (`PUT /fapi/v1/order`); spot has no in-place amend, so it is emulated
    /// as an atomic cancel-replace (`POST /api/v3/order/cancelReplace`). Either
    /// path first reads the existing order to preserve its side and type.
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
        let existing = self.query_order(symbol, order_id)?;
        let quantity = new_quantity.unwrap_or(existing.quantity);
        let price = new_price.or(existing.price);
        let wire = Self::wire_symbol(symbol);
        if self.is_futures() {
            let mut params = format!(
                "symbol={wire}&orderId={order_id}&side={}&quantity={}",
                side_str(existing.side),
                format_decimal(quantity),
            );
            if let Some(p) = price {
                params.push_str("&price=");
                params.push_str(&format_decimal(p));
            }
            let body = self.signed_request(HttpMethod::Put, self.order_path(), &params)?;
            return parse_order(symbol, &body);
        }
        let mut params = format!(
            "symbol={wire}&cancelReplaceMode=STOP_ON_FAILURE&cancelOrderId={order_id}\
             &side={}&type={}&quantity={}",
            side_str(existing.side),
            order_type_str(existing.order_type),
            format_decimal(quantity),
        );
        if let Some(p) = price {
            params.push_str("&price=");
            params.push_str(&format_decimal(p));
            if existing.order_type.requires_price() {
                params.push_str("&timeInForce=GTC");
            }
        }
        let body = self.signed_request(HttpMethod::Post, "/api/v3/order/cancelReplace", &params)?;
        let value: serde_json::Value = deserialize(&body)?;
        let new_order = value
            .get("newOrderResult")
            .ok_or_else(|| Error::Deserialization("missing newOrderResult".to_string()))?;
        let raw: RawOrder = serde_json::from_value(new_order.clone())
            .map_err(|e| Error::Deserialization(e.to_string()))?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// Place several orders in one round-trip. Binance futures has a native batch
    /// endpoint (`POST /fapi/v1/batchOrders`); spot has none, so the orders are
    /// placed sequentially. Each order's own outcome is preserved.
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails.
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if self.is_futures() {
            let items = requests
                .iter()
                .map(batch_order_json)
                .collect::<Vec<_>>()
                .join(",");
            let params = format!("batchOrders={}", percent_encode(&format!("[{items}]")));
            let body = self.signed_request(HttpMethod::Post, "/fapi/v1/batchOrders", &params)?;
            let arr: Vec<serde_json::Value> = deserialize(&body)?;
            return Ok(arr.iter().map(batch_element_to_order).collect());
        }
        Ok(requests.iter().map(|r| self.place_order(r)).collect())
    }

    /// Cancel several orders on one `symbol` in a single round-trip. Binance
    /// futures has a native batch cancel (`DELETE /fapi/v1/batchOrders`); spot has
    /// none, so the ids are cancelled sequentially.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        if self.is_futures() {
            let list = order_ids
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(",");
            let params = format!(
                "symbol={}&orderIdList={}",
                Self::wire_symbol(symbol),
                percent_encode(&format!("[{list}]")),
            );
            self.signed_request(HttpMethod::Delete, "/fapi/v1/batchOrders", &params)?;
            return Ok(());
        }
        for id in order_ids {
            self.cancel_order(symbol, id)?;
        }
        Ok(())
    }

    /// Place a one-cancels-other bracket (`POST /api/v3/order/oco`), returning the
    /// two order legs. OCO is a spot order-list; Binance USDⓈ-M futures has no
    /// order-list, so this returns an [`Error::Exchange`] on a futures client.
    ///
    /// # Errors
    /// Returns an [`Error`] if the OCO is invalid, unsupported on this market, or rejected.
    pub fn place_oco(&self, request: &OcoRequest) -> Result<Vec<Order>> {
        request.validate()?;
        if self.is_futures() {
            return Err(Error::Exchange {
                code: "unsupported".to_string(),
                message: "Binance USD-M futures has no OCO order-list; use separate \
                          take-profit / stop orders"
                    .to_string(),
            });
        }
        let mut params = format!(
            "symbol={}&side={}&quantity={}&price={}&stopPrice={}",
            Self::wire_symbol(&request.symbol),
            side_str(request.side),
            format_decimal(request.quantity),
            format_decimal(request.price),
            format_decimal(request.stop_price),
        );
        if let Some(slp) = request.stop_limit_price {
            params.push_str("&stopLimitPrice=");
            params.push_str(&format_decimal(slp));
            params.push_str("&stopLimitTimeInForce=GTC");
        }
        if let Some(id) = &request.client_order_id {
            params.push_str("&listClientOrderId=");
            params.push_str(id);
        }
        let body = self.signed_request(HttpMethod::Post, "/api/v3/order/oco", &params)?;
        let value: serde_json::Value = deserialize(&body)?;
        let reports = value
            .get("orderReports")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Deserialization("missing orderReports".to_string()))?;
        reports
            .iter()
            .map(|report| {
                let raw: RawOrder = serde_json::from_value(report.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                order_from_raw(request.symbol.clone(), &raw)
            })
            .collect()
    }
}

impl Derivatives for Binance {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Binance::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Binance::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Binance::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Binance::close_position(self, symbol)
    }
}

impl AdvancedOrders for Binance {
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Binance::amend_order(self, symbol, order_id, new_price, new_quantity)
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Binance::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Binance::cancel_batch(self, symbol, order_ids)
    }
    fn place_oco(&mut self, request: &OcoRequest) -> Result<Vec<Order>> {
        Binance::place_oco(self, request)
    }
}

/// Percent-encode per RFC 3986 (unreserved characters pass through), used for the
/// JSON `batchOrders`/`orderIdList` values in the futures batch endpoints.
fn percent_encode(s: &str) -> String {
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

/// One element of a futures `batchOrders` JSON array.
fn batch_order_json(request: &OrderRequest) -> String {
    let mut obj = serde_json::json!({
        "symbol": Binance::wire_symbol(&request.symbol),
        "side": side_str(request.side),
        "type": order_type_str(request.order_type),
        "quantity": format_decimal(request.quantity),
    });
    if let Some(price) = request.price {
        obj["price"] = serde_json::json!(format_decimal(price));
    }
    if request.order_type.requires_price() {
        obj["timeInForce"] = serde_json::json!(tif_str(request.time_in_force));
    }
    if let Some(id) = &request.client_order_id {
        obj["newClientOrderId"] = serde_json::json!(id);
    }
    if request.reduce_only {
        obj["reduceOnly"] = serde_json::json!("true");
    }
    obj.to_string()
}

/// A batch-order response element is either a placed order or a `{code, msg}`
/// rejection; map it onto that order's own [`Result`].
fn batch_element_to_order(value: &serde_json::Value) -> Result<Order> {
    if let Some(code) = value.get("code").and_then(serde_json::Value::as_i64) {
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
    let raw: RawOrder =
        serde_json::from_value(value.clone()).map_err(|e| Error::Deserialization(e.to_string()))?;
    let symbol = split_wire_symbol(&raw.symbol);
    order_from_raw(symbol, &raw)
}

fn parse_positions(body: &str) -> Result<Vec<Position>> {
    let raw: Vec<RawPosition> = deserialize(body)?;
    let mut positions = Vec::new();
    for entry in raw {
        let amount = parse_decimal(&entry.position_amt)?;
        if amount.is_zero() {
            continue;
        }
        let side = if amount.is_sign_negative() {
            PositionSide::Short
        } else {
            PositionSide::Long
        };
        positions.push(Position {
            symbol: split_wire_symbol(&entry.symbol),
            side,
            quantity: amount.abs(),
            entry_price: parse_decimal(&entry.entry_price)?,
            mark_price: parse_decimal(&entry.mark_price)?,
            leverage: parse_decimal(&entry.leverage)?,
            unrealized_pnl: parse_decimal(&entry.unrealized)?,
            margin_mode: if entry.margin_type.eq_ignore_ascii_case("isolated") {
                MarginMode::Isolated
            } else {
                MarginMode::Cross
            },
        });
    }
    Ok(positions)
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    }
}

fn order_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market => "MARKET",
        OrderType::Limit => "LIMIT",
        OrderType::StopMarket => "STOP_LOSS",
        OrderType::StopLimit => "STOP_LOSS_LIMIT",
    }
}

fn tif_str(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
    }
}

/// The Binance `selfTradePreventionMode` value, or `None` for no STP.
fn stp_str(stp: SelfTradePrevention) -> Option<&'static str> {
    match stp {
        SelfTradePrevention::None => None,
        SelfTradePrevention::ExpireMaker => Some("EXPIRE_MAKER"),
        SelfTradePrevention::ExpireTaker => Some("EXPIRE_TAKER"),
        SelfTradePrevention::ExpireBoth => Some("EXPIRE_BOTH"),
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    match raw {
        "MARKET" => Ok(OrderType::Market),
        "LIMIT" | "LIMIT_MAKER" => Ok(OrderType::Limit),
        "STOP_LOSS" | "TAKE_PROFIT" => Ok(OrderType::StopMarket),
        "STOP_LOSS_LIMIT" | "TAKE_PROFIT_LIMIT" => Ok(OrderType::StopLimit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "NEW" => Ok(OrderStatus::New),
        "PARTIALLY_FILLED" => Ok(OrderStatus::PartiallyFilled),
        "FILLED" => Ok(OrderStatus::Filled),
        "CANCELED" | "PENDING_CANCEL" => Ok(OrderStatus::Canceled),
        "REJECTED" => Ok(OrderStatus::Rejected),
        "EXPIRED" | "EXPIRED_IN_MATCH" => Ok(OrderStatus::Expired),
        other => Err(Error::Deserialization(format!("unknown status {other:?}"))),
    }
}

fn parse_order(symbol: &Symbol, body: &str) -> Result<Order> {
    let raw: RawOrder = deserialize(body)?;
    order_from_raw(symbol.clone(), &raw)
}

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    let executed = parse_decimal(&raw.executed_qty)?;
    // Futures reports the fill price directly as `avgPrice`; spot reports the
    // cumulative quote quantity, from which the average is derived.
    let avg = parse_decimal(&raw.avg_price).unwrap_or(Decimal::ZERO);
    let average_price = if avg > Decimal::ZERO {
        Some(avg)
    } else if executed > Decimal::ZERO && !raw.cummulative_quote_qty.is_empty() {
        Some(parse_decimal(&raw.cummulative_quote_qty)? / executed)
    } else {
        None
    };
    let parsed_price = parse_decimal(&raw.price)?;
    let price = (parsed_price > Decimal::ZERO).then_some(parsed_price);
    Ok(Order {
        id: raw.order_id.to_string(),
        client_order_id: (!raw.client_order_id.is_empty()).then(|| raw.client_order_id.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status: parse_status(&raw.status)?,
        quantity: parse_decimal(&raw.orig_qty)?,
        filled_quantity: executed,
        price,
        average_price,
    })
}

/// Quote assets used to split a concatenated wire symbol (`BTCUSDT` -> `BTC/USDT`)
/// when the venue reports only the wire form. Longer quotes are tried first.
const KNOWN_QUOTES: &[&str] = &[
    "FDUSD", "USDT", "USDC", "TUSD", "BUSD", "DAI", "EUR", "TRY", "BTC", "ETH", "BNB", "USD",
];

/// Map a concatenated Binance wire symbol back to a canonical [`Symbol`] using
/// the known quote-asset suffixes. Falls back to the whole string as the base.
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

fn field_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Deserialization(format!("missing string field {key:?}")))
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

/// Parse one Binance WebSocket frame into an [`Event`], resolving the wire
/// symbol with `resolve`. Non-data frames (subscription acks) and unhandled
/// event types return `Ok(None)`.
fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Option<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    // A combined stream wraps the payload as {"stream":..,"data":..}.
    let data = value.get("data").unwrap_or(&value);
    let Some(event_type) = data.get("e").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    match event_type {
        "trade" => {
            // `m` = "is the buyer the market maker?"; if so the taker (aggressor)
            // is the seller.
            let is_maker_buyer = data.get("m").and_then(serde_json::Value::as_bool) == Some(true);
            Ok(Some(Event::Trade(TradePrint {
                symbol: resolve(field_str(data, "s")?),
                price: parse_decimal(field_str(data, "p")?)?,
                quantity: parse_decimal(field_str(data, "q")?)?,
                aggressor: if is_maker_buyer {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                },
                timestamp: data
                    .get("T")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })))
        }
        "depthUpdate" => Ok(Some(Event::BookDelta(BookDelta {
            symbol: resolve(field_str(data, "s")?),
            first_update_id: data
                .get("U")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            final_update_id: data
                .get("u")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            bids: parse_ws_levels(data.get("b"))?,
            asks: parse_ws_levels(data.get("a"))?,
        }))),
        "24hrTicker" => Ok(Some(Event::Ticker(Ticker {
            symbol: resolve(field_str(data, "s")?),
            last: parse_decimal(field_str(data, "c")?)?,
            bid: parse_decimal(field_str(data, "b")?)?,
            ask: parse_decimal(field_str(data, "a")?)?,
            volume: parse_decimal(field_str(data, "v")?)?,
        }))),
        _ => Ok(None),
    }
}

/// The REST base URL for a market type and network.
fn rest_base_url(market_type: MarketType, testnet: bool) -> &'static str {
    match (market_type, testnet) {
        (MarketType::UsdMFutures, false) => "https://fapi.binance.com",
        (MarketType::UsdMFutures, true) => "https://testnet.binancefuture.com",
        (_, true) => "https://testnet.binance.vision",
        (_, false) => "https://api.binance.com",
    }
}

/// The WebSocket base URL for a market type and network.
fn ws_base_url(market_type: MarketType, testnet: bool) -> &'static str {
    match (market_type, testnet) {
        (MarketType::UsdMFutures, false) => "wss://fstream.binance.com/ws",
        (MarketType::UsdMFutures, true) => "wss://stream.binancefuture.com/ws",
        (_, true) => "wss://testnet.binance.vision/ws",
        (_, false) => "wss://stream.binance.com:9443/ws",
    }
}

#[derive(Deserialize)]
struct RawTicker {
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "bidPrice")]
    bid_price: String,
    #[serde(rename = "askPrice")]
    ask_price: String,
    volume: String,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
struct BinanceError {
    code: i64,
    msg: String,
}

#[derive(Deserialize)]
struct RawOrder {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: u64,
    #[serde(rename = "clientOrderId", default)]
    client_order_id: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    status: String,
    #[serde(rename = "origQty")]
    orig_qty: String,
    #[serde(rename = "executedQty")]
    executed_qty: String,
    #[serde(rename = "cummulativeQuoteQty", default)]
    cummulative_quote_qty: String,
    #[serde(rename = "avgPrice", default)]
    avg_price: String,
    price: String,
}

#[derive(Deserialize)]
struct RawAccount {
    balances: Vec<RawBalance>,
}

#[derive(Deserialize)]
struct RawBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Deserialize)]
struct RawFuturesTicker {
    #[serde(rename = "lastPrice")]
    last_price: String,
    volume: String,
}

#[derive(Deserialize)]
struct RawBookTicker {
    #[serde(rename = "bidPrice")]
    bid_price: String,
    #[serde(rename = "askPrice")]
    ask_price: String,
}

#[derive(Deserialize)]
struct RawFuturesBalance {
    asset: String,
    balance: String,
    #[serde(rename = "availableBalance")]
    available_balance: String,
}

#[derive(Deserialize)]
struct RawPosition {
    symbol: String,
    #[serde(rename = "positionAmt")]
    position_amt: String,
    #[serde(rename = "entryPrice")]
    entry_price: String,
    #[serde(rename = "markPrice")]
    mark_price: String,
    #[serde(rename = "unRealizedProfit")]
    unrealized: String,
    leverage: String,
    #[serde(rename = "marginType")]
    margin_type: String,
}

#[derive(Deserialize)]
struct RawExchangeInfo {
    symbols: Vec<RawSymbol>,
}

#[derive(Deserialize)]
struct RawSymbol {
    #[serde(rename = "baseAsset")]
    base_asset: String,
    #[serde(rename = "quoteAsset")]
    quote_asset: String,
    #[serde(rename = "baseAssetPrecision", default)]
    base_asset_precision: u32,
    #[serde(rename = "quoteAssetPrecision", default)]
    quote_asset_precision: u32,
    #[serde(default)]
    filters: Vec<serde_json::Value>,
}

fn find_filter<'a>(filters: &'a [serde_json::Value], kind: &str) -> Option<&'a serde_json::Value> {
    filters
        .iter()
        .find(|f| f.get("filterType").and_then(serde_json::Value::as_str) == Some(kind))
}

fn filter_decimal(filter: Option<&serde_json::Value>, key: &str) -> Decimal {
    filter
        .and_then(|f| f.get(key))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| parse_decimal(s).ok())
        .unwrap_or(Decimal::ZERO)
}

fn parse_instrument(raw: &RawSymbol) -> Instrument {
    let lot = find_filter(&raw.filters, "LOT_SIZE");
    let price = find_filter(&raw.filters, "PRICE_FILTER");
    let notional =
        find_filter(&raw.filters, "NOTIONAL").or_else(|| find_filter(&raw.filters, "MIN_NOTIONAL"));
    Instrument {
        symbol: Symbol::new(&raw.base_asset, &raw.quote_asset),
        base_precision: raw.base_asset_precision,
        quote_precision: raw.quote_asset_precision,
        filters: InstrumentFilters {
            min_quantity: filter_decimal(lot, "minQty"),
            max_quantity: filter_decimal(lot, "maxQty"),
            step_size: filter_decimal(lot, "stepSize"),
            min_price: filter_decimal(price, "minPrice"),
            max_price: filter_decimal(price, "maxPrice"),
            tick_size: filter_decimal(price, "tickSize"),
            min_notional: filter_decimal(notional, "minNotional"),
        },
    }
}

fn deserialize<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))
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

fn parse_kline_row(row: &[serde_json::Value]) -> Result<Candle> {
    // Binance kline: [openTime, open, high, low, close, volume, closeTime, ...].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let open_time = row[0]
        .as_i64()
        .ok_or_else(|| Error::Deserialization("kline open time not an integer".to_string()))?;
    let open = kline_f64(&row[1])?;
    let high = kline_f64(&row[2])?;
    let low = kline_f64(&row[3])?;
    let close = kline_f64(&row[4])?;
    let volume = kline_f64(&row[5])?;
    Candle::new(open, high, low, close, volume, open_time)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

fn kline_f64(value: &serde_json::Value) -> Result<f64> {
    value
        .as_str()
        .ok_or_else(|| Error::Deserialization("kline field not a string".to_string()))?
        .parse::<f64>()
        .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
}

/// Map a non-success Binance response onto the unified error taxonomy.
fn map_error(response: &HttpResponse) -> Error {
    let Ok(err) = serde_json::from_str::<BinanceError>(&response.body) else {
        return Error::Exchange {
            code: response.status.to_string(),
            message: response.body.clone(),
        };
    };
    match err.code {
        -1121 => Error::InvalidSymbol(err.msg),
        -2010 | -2018 | -2019 => Error::InsufficientBalance,
        -1003 => Error::RateLimited { retry_after: None },
        -1022 | -2014 | -2015 => Error::Auth(err.msg),
        _ => Error::Exchange {
            code: err.code.to_string(),
            message: err.msg,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{MockHttpTransport, MockWsTransport};
    use rust_decimal_macros::dec;
    use std::sync::Arc;

    /// A `WsTransport` that forwards to a shared `MockWsTransport`, so a test
    /// keeps a handle after the client takes ownership.
    struct ArcWs(Arc<MockWsTransport>);
    impl WsTransport for ArcWs {
        fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>> {
            self.0.connect(url)
        }
    }

    fn symbol() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    /// A Binance client over a mock transport, returning the mock so the test can
    /// queue responses and inspect requests.
    fn client(market_type: MarketType, testnet: bool) -> (Binance, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = if testnet {
            ExchangeOptions::testnet(market_type)
        } else {
            ExchangeOptions::mainnet(market_type)
        };
        let binance = Binance::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts);
        (binance, mock)
    }

    /// A transport that forwards to a shared `MockHttpTransport` so the test keeps
    /// a handle after the client takes ownership.
    struct ArcTransport(Arc<MockHttpTransport>);
    impl HttpTransport for ArcTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.0.execute(request)
        }
    }

    #[test]
    fn wire_symbol_concatenates() {
        assert_eq!(Binance::wire_symbol(&symbol()), "BTCUSDT");
    }

    #[test]
    fn rest_base_urls_by_market_and_network() {
        assert_eq!(
            rest_base_url(MarketType::Spot, false),
            "https://api.binance.com"
        );
        assert_eq!(
            rest_base_url(MarketType::Spot, true),
            "https://testnet.binance.vision"
        );
        assert_eq!(
            rest_base_url(MarketType::UsdMFutures, false),
            "https://fapi.binance.com"
        );
        assert_eq!(
            rest_base_url(MarketType::UsdMFutures, true),
            "https://testnet.binancefuture.com"
        );
    }

    #[test]
    fn ticker_parses_and_targets_the_right_url() {
        let (binance, mock) = client(MarketType::Spot, false);
        assert_eq!(binance.market_type(), MarketType::Spot);
        mock.push_json(
            200,
            r#"{"lastPrice":"20000.50","bidPrice":"20000.00","askPrice":"20001.00","volume":"1234.5"}"#,
        );
        let ticker = binance.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.50));
        assert_eq!(ticker.bid, dec!(20000.00));
        assert_eq!(ticker.ask, dec!(20001.00));
        assert_eq!(ticker.volume, dec!(1234.5));

        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://api.binance.com/api/v3/ticker/24hr?symbol=BTCUSDT"
        );
    }

    #[test]
    // The kline fields parse from exact decimal strings, so an exact f64 compare
    // is correct here.
    #[allow(clippy::float_cmp)]
    fn klines_parse_into_candles() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(
            200,
            r#"[[1499040000000,"100.0","110.0","95.0","105.0","12.5",1499040059999,"0",1,"0","0","0"]]"#,
        );
        let candles = binance.klines(&symbol(), "1h", 1).unwrap();
        assert_eq!(candles.len(), 1);
        let c = candles[0];
        assert_eq!(c.open, 100.0);
        assert_eq!(c.high, 110.0);
        assert_eq!(c.low, 95.0);
        assert_eq!(c.close, 105.0);
        assert_eq!(c.volume, 12.5);
        assert_eq!(c.timestamp, 1_499_040_000_000);
    }

    #[test]
    fn order_book_parses_levels() {
        let (binance, mock) = client(MarketType::Spot, true);
        mock.push_json(
            200,
            r#"{"lastUpdateId":42,"bids":[["100.0","1.5"],["99.0","2.0"]],"asks":[["101.0","1.0"]]}"#,
        );
        let book = binance.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 42);
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100.0), dec!(1.5)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101.0), dec!(1.0)));
        // Testnet base.
        let req = &mock.recorded_requests()[0];
        assert!(req
            .url
            .starts_with("https://testnet.binance.vision/api/v3/depth"));
    }

    #[test]
    fn invalid_symbol_error_is_mapped() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(400, r#"{"code":-1121,"msg":"Invalid symbol."}"#);
        assert!(matches!(
            binance.ticker(&symbol()).unwrap_err(),
            Error::InvalidSymbol(_)
        ));
    }

    #[test]
    fn error_taxonomy_mapping() {
        let cases = [
            (r#"{"code":-2010,"msg":"x"}"#, "balance"),
            (r#"{"code":-1003,"msg":"x"}"#, "rate"),
            (r#"{"code":-2015,"msg":"x"}"#, "auth"),
            (r#"{"code":-9999,"msg":"weird"}"#, "exchange"),
        ];
        for (body, kind) in cases {
            let (binance, mock) = client(MarketType::Spot, false);
            mock.push_json(400, body);
            let err = binance.ticker(&symbol()).unwrap_err();
            match kind {
                "balance" => assert!(matches!(err, Error::InsufficientBalance)),
                "rate" => assert!(matches!(err, Error::RateLimited { .. })),
                "auth" => assert!(matches!(err, Error::Auth(_))),
                _ => assert!(matches!(err, Error::Exchange { .. })),
            }
        }
    }

    #[test]
    fn non_json_error_body_falls_back_to_exchange() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(502, "<html>bad gateway</html>");
        assert!(matches!(
            binance.ticker(&symbol()).unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn short_kline_row_is_rejected() {
        let (binance, mock) = client(MarketType::Spot, false);
        mock.push_json(200, r#"[[1499040000000,"100.0"]]"#);
        assert!(matches!(
            binance.klines(&symbol(), "1h", 1).unwrap_err(),
            Error::Deserialization(_)
        ));
    }

    const ORDER_JSON: &str = r#"{"symbol":"BTCUSDT","orderId":28,"clientOrderId":"abc",
        "price":"100.00000000","origQty":"1.00000000","executedQty":"0.00000000",
        "cummulativeQuoteQty":"0.00000000","status":"NEW","type":"LIMIT","side":"BUY"}"#;

    /// An authenticated client over a mock transport with a fixed clock.
    fn signed_client(now_ms: i64) -> (Binance, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let binance = Binance::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (binance, mock)
    }

    /// An authenticated USDⓈ-M futures client over a mock transport.
    fn signed_futures_client(now_ms: i64) -> (Binance, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
        let binance = Binance::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (binance, mock)
    }

    #[test]
    fn stp_mode_appends_param() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        binance
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .with_stp(SelfTradePrevention::ExpireMaker),
            )
            .unwrap();
        assert!(mock.recorded_requests()[0]
            .url
            .contains("selfTradePreventionMode=EXPIRE_MAKER"));
    }

    #[test]
    fn stp_none_omits_param() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert!(!mock.recorded_requests()[0]
            .url
            .contains("selfTradePreventionMode"));
    }

    #[test]
    fn amend_spot_uses_cancel_replace() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON); // query_order
        mock.push_json(
            200,
            r#"{"cancelResult":"SUCCESS","newOrderResult":{"symbol":"BTCUSDT","orderId":456,
            "clientOrderId":"","side":"BUY","type":"LIMIT","status":"NEW","origQty":"2",
            "executedQty":"0","price":"101"}}"#,
        );
        let order = binance
            .amend_order(&symbol(), "123", Some(dec!(101)), Some(dec!(2)))
            .unwrap();
        assert_eq!(order.id, "456");
        assert_eq!(order.quantity, dec!(2));
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/api/v3/order/cancelReplace"));
        assert!(reqs[1].url.contains("cancelOrderId=123"));
        assert!(reqs[1].url.contains("quantity=2"));
        assert!(reqs[1].url.contains("price=101"));
        assert_eq!(reqs[1].method, HttpMethod::Post);
    }

    #[test]
    fn amend_futures_puts_in_place() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(200, ORDER_JSON); // query_order
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","orderId":123,"clientOrderId":"","side":"BUY","type":"LIMIT",
            "status":"NEW","origQty":"1","executedQty":"0","price":"105","avgPrice":"0"}"#,
        );
        let order = binance
            .amend_order(&symbol(), "123", Some(dec!(105)), None)
            .unwrap();
        assert_eq!(order.price, Some(dec!(105)));
        let reqs = mock.recorded_requests();
        assert_eq!(reqs[1].method, HttpMethod::Put);
        assert!(reqs[1].url.contains("/fapi/v1/order"));
        assert!(reqs[1].url.contains("price=105"));
        assert!(reqs[1].url.contains("orderId=123"));
    }

    #[test]
    fn place_batch_spot_is_sequential() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        mock.push_json(200, ORDER_JSON);
        let results = binance
            .place_batch(&[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(101)),
            ])
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(std::result::Result::is_ok));
        assert_eq!(mock.recorded_requests().len(), 2);
    }

    #[test]
    fn place_batch_futures_is_one_call_with_per_order_results() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"[{"symbol":"BTCUSDT","orderId":1,"clientOrderId":"","side":"BUY","type":"LIMIT",
            "status":"NEW","origQty":"1","executedQty":"0","price":"100","avgPrice":"0"},
            {"code":-2019,"msg":"Margin is insufficient."}]"#,
        );
        let results = binance
            .place_batch(&[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(101)),
            ])
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(matches!(
            results[1].as_ref().unwrap_err(),
            Error::OrderRejected { .. }
        ));
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/fapi/v1/batchOrders"));
        assert!(reqs[0].url.contains("batchOrders=%5B")); // url-encoded '['
    }

    #[test]
    fn cancel_batch_spot_is_sequential() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, "{}");
        mock.push_json(200, "{}");
        binance
            .cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        assert_eq!(mock.recorded_requests().len(), 2);
    }

    #[test]
    fn cancel_batch_futures_is_one_call() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(200, "[{}]");
        binance
            .cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, HttpMethod::Delete);
        assert!(reqs[0].url.contains("/fapi/v1/batchOrders"));
        assert!(reqs[0].url.contains("orderIdList=%5B")); // url-encoded '['
    }

    #[test]
    fn place_oco_spot_returns_both_legs() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"orderListId":99,"orderReports":[
            {"symbol":"BTCUSDT","orderId":1,"clientOrderId":"","side":"SELL","type":"LIMIT_MAKER",
            "status":"NEW","origQty":"1","executedQty":"0","price":"110"},
            {"symbol":"BTCUSDT","orderId":2,"clientOrderId":"","side":"SELL","type":"STOP_LOSS_LIMIT",
            "status":"NEW","origQty":"1","executedQty":"0","price":"90"}]}"#,
        );
        let legs = binance
            .place_oco(&OcoRequest::new(
                symbol(),
                OrderSide::Sell,
                dec!(1),
                dec!(110),
                dec!(95),
            ))
            .unwrap();
        assert_eq!(legs.len(), 2);
        assert_eq!(legs[0].id, "1");
        assert_eq!(legs[1].id, "2");
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/api/v3/order/oco"));
        assert!(req.url.contains("stopPrice=95"));
    }

    #[test]
    fn place_oco_futures_is_unsupported() {
        let (binance, _mock) = signed_futures_client(1000);
        assert!(matches!(
            binance
                .place_oco(&OcoRequest::new(
                    symbol(),
                    OrderSide::Sell,
                    dec!(1),
                    dec!(110),
                    dec!(95)
                ))
                .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    #[test]
    fn futures_ticker_combines_stats_and_book() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","lastPrice":"20000.0","volume":"1234.0"}"#,
        );
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","bidPrice":"19999.0","askPrice":"20001.0"}"#,
        );
        let ticker = binance.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, dec!(19999));
        assert_eq!(ticker.ask, dec!(20001));
        assert_eq!(ticker.volume, dec!(1234));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("fapi.binance.com/fapi/v1/ticker/24hr"));
        assert!(reqs[1].url.contains("/fapi/v1/ticker/bookTicker"));
    }

    #[test]
    fn futures_balances_use_fapi_v2_balance() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"[{"asset":"USDT","balance":"1000.0","availableBalance":"800.0"}]"#,
        );
        let balances = binance.balances().unwrap();
        assert_eq!(balances[0].asset, "USDT");
        assert_eq!(balances[0].free, dec!(800));
        assert_eq!(balances[0].locked, dec!(200));
        assert!(mock.recorded_requests()[0].url.contains("/fapi/v2/balance"));
    }

    #[test]
    fn futures_place_order_uses_fapi_order_path() {
        let (binance, mock) = signed_futures_client(1000);
        mock.push_json(200, ORDER_JSON);
        binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(20000)))
            .unwrap();
        assert!(mock.recorded_requests()[0].url.contains("/fapi/v1/order"));
    }

    #[test]
    fn derivatives_positions_parse_and_skip_flat() {
        let (mut binance, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"[
              {"symbol":"BTCUSDT","positionAmt":"0.5","entryPrice":"20000.0","markPrice":"20100.0","unRealizedProfit":"50.0","leverage":"10","marginType":"isolated"},
              {"symbol":"ETHUSDT","positionAmt":"0.0","entryPrice":"0.0","markPrice":"0.0","unRealizedProfit":"0.0","leverage":"5","marginType":"cross"},
              {"symbol":"XRPUSDT","positionAmt":"-100.0","entryPrice":"0.5","markPrice":"0.48","unRealizedProfit":"2.0","leverage":"20","marginType":"cross"}
            ]"#,
        );
        let positions = Derivatives::positions(&mut binance, None).unwrap();
        assert_eq!(positions.len(), 2); // the flat ETH position is skipped
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(0.5));
        assert_eq!(positions[0].leverage, dec!(10));
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert_eq!(positions[1].side, PositionSide::Short);
        assert_eq!(positions[1].quantity, dec!(100));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/fapi/v2/positionRisk"));
    }

    #[test]
    fn derivatives_set_leverage_and_margin_mode() {
        let (mut binance, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"leverage":10,"symbol":"BTCUSDT"}"#);
        Derivatives::set_leverage(&mut binance, &symbol(), 10).unwrap();
        mock.push_json(200, r#"{"code":200,"msg":"success"}"#);
        Derivatives::set_margin_mode(&mut binance, &symbol(), MarginMode::Isolated).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/fapi/v1/leverage"));
        assert!(reqs[0].url.contains("leverage=10"));
        assert!(reqs[1].url.contains("/fapi/v1/marginType"));
        assert!(reqs[1].url.contains("marginType=ISOLATED"));
    }

    #[test]
    fn derivatives_close_position_places_reduce_only_market() {
        let (mut binance, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"[{"symbol":"BTCUSDT","positionAmt":"0.5","entryPrice":"20000.0","markPrice":"20100.0","unRealizedProfit":"50.0","leverage":"10","marginType":"isolated"}]"#,
        );
        mock.push_json(200, ORDER_JSON);
        Derivatives::close_position(&mut binance, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/fapi/v1/order"));
        assert!(reqs[1].url.contains("side=SELL"));
        assert!(reqs[1].url.contains("reduceOnly=true"));
    }

    #[test]
    fn place_order_signs_request_and_parses_response() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        let order = binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "28");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.quantity, dec!(1));
        assert_eq!(order.price, Some(dec!(100)));
        assert_eq!(order.average_price, None);

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let payload = "symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=1&price=100\
                       &timeInForce=GTC&recvWindow=5000&timestamp=1000";
        let sig = crate::signing::hmac_sha256_hex(b"SECRET", payload.as_bytes());
        assert!(req.url.contains(payload), "payload mismatch: {}", req.url);
        assert!(req.url.ends_with(&format!("signature={sig}")));
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "X-MBX-APIKEY" && v == "APIKEY"));
    }

    #[test]
    fn post_only_limit_becomes_limit_maker_without_tif() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).post_only())
            .unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("type=LIMIT_MAKER"));
        assert!(!req.url.contains("timeInForce"));
    }

    #[test]
    fn signed_endpoint_without_credentials_errors() {
        let (binance, _) = client(MarketType::Spot, false);
        let err = binance
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap_err();
        assert!(matches!(err, Error::InvalidCredentials(_)));
    }

    #[test]
    fn cancel_order_is_a_signed_delete() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","orderId":28,"status":"CANCELED"}"#,
        );
        binance.cancel_order(&symbol(), "28").unwrap();
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Delete);
        assert!(req.url.contains("orderId=28"));
        assert!(req.url.contains("signature="));
    }

    #[test]
    fn query_order_computes_average_fill_price() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"symbol":"BTCUSDT","orderId":28,"clientOrderId":"","price":"0.00000000",
            "origQty":"2.0","executedQty":"2.0","cummulativeQuoteQty":"200.0",
            "status":"FILLED","type":"MARKET","side":"SELL"}"#,
        );
        let order = binance.query_order(&symbol(), "28").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.average_price, Some(dec!(100))); // 200 / 2
        assert_eq!(order.price, None); // 0 -> None
        assert_eq!(order.client_order_id, None); // empty -> None
        assert_eq!(order.filled_quantity, dec!(2));
    }

    #[test]
    fn balances_parse() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"balances":[{"asset":"USDT","free":"100.5","locked":"25.5"},
            {"asset":"BTC","free":"0.1","locked":"0"}]}"#,
        );
        let bals = binance.balances().unwrap();
        assert_eq!(bals.len(), 2);
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].total(), dec!(126));
        assert_eq!(bals[1].asset, "BTC");
    }

    #[test]
    fn system_clock_is_sane() {
        // Covers the production timestamp source: a plausible 2023+ epoch ms.
        assert!(system_now_ms() > 1_600_000_000_000);
    }

    fn resolve(_wire: &str) -> Symbol {
        symbol()
    }

    #[test]
    fn ws_trade_frame_maps_aggressor() {
        // m=false -> buyer is taker -> Buy aggressor.
        let buy = parse_ws_message(
            r#"{"e":"trade","s":"BTCUSDT","p":"100.5","q":"0.25","m":false,"T":1700000000000}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            buy,
            Event::Trade(TradePrint {
                symbol: symbol(),
                price: dec!(100.5),
                quantity: dec!(0.25),
                aggressor: OrderSide::Buy,
                timestamp: 1_700_000_000_000,
            })
        );
        // m=true -> seller is taker -> Sell aggressor.
        let sell = parse_ws_message(
            r#"{"e":"trade","s":"BTCUSDT","p":"100","q":"1","m":true,"T":1}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        let Event::Trade(print) = sell else {
            panic!("expected trade")
        };
        assert_eq!(print.aggressor, OrderSide::Sell);
    }

    #[test]
    fn ws_combined_stream_wrapper_is_unwrapped() {
        let event = parse_ws_message(
            r#"{"stream":"btcusdt@trade","data":{"e":"trade","s":"BTCUSDT","p":"1","q":"1","m":false,"T":1}}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(event, Event::Trade(_)));
    }

    #[test]
    fn ws_depth_update_maps_to_book_delta() {
        let event = parse_ws_message(
            r#"{"e":"depthUpdate","s":"BTCUSDT","U":10,"u":12,"b":[["100","1"],["99","0"]],"a":[["101","2"]]}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        let Event::BookDelta(delta) = event else {
            panic!("expected book delta")
        };
        assert_eq!(delta.first_update_id, 10);
        assert_eq!(delta.final_update_id, 12);
        assert_eq!(
            delta.bids,
            vec![
                BookLevel::new(dec!(100), dec!(1)),
                BookLevel::new(dec!(99), dec!(0))
            ]
        );
        assert_eq!(delta.asks, vec![BookLevel::new(dec!(101), dec!(2))]);
    }

    #[test]
    fn ws_ticker_frame_maps_to_ticker() {
        let event = parse_ws_message(
            r#"{"e":"24hrTicker","s":"BTCUSDT","c":"100","b":"99","a":"101","v":"1234"}"#,
            &resolve,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            event,
            Event::Ticker(Ticker {
                symbol: symbol(),
                last: dec!(100),
                bid: dec!(99),
                ask: dec!(101),
                volume: dec!(1234),
            })
        );
    }

    #[test]
    fn ws_non_event_frames_are_ignored() {
        // Subscription ack.
        assert!(parse_ws_message(r#"{"result":null,"id":1}"#, &resolve)
            .unwrap()
            .is_none());
        // Unhandled event type.
        assert!(parse_ws_message(r#"{"e":"kline","s":"BTCUSDT"}"#, &resolve)
            .unwrap()
            .is_none());
    }

    #[test]
    fn ws_malformed_frame_errors() {
        assert!(matches!(
            parse_ws_message("not json", &resolve).unwrap_err(),
            Error::Deserialization(_)
        ));
        // A trade frame missing a required field.
        assert!(parse_ws_message(r#"{"e":"trade","s":"BTCUSDT"}"#, &resolve).is_err());
    }

    fn streaming_client(ws: &Arc<MockWsTransport>) -> Binance {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        Binance::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(ws))))
    }

    #[test]
    fn subscribe_sends_frame_and_poll_returns_events() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"e":"trade","s":"BTCUSDT","p":"100","q":"1","m":false,"T":1}"#.to_string(),
            )),
            Ok(Some(
                r#"{"e":"trade","s":"BTCUSDT","p":"101","q":"2","m":true,"T":2}"#.to_string(),
            )),
        ]);
        let mut binance = streaming_client(&ws);
        binance.subscribe_trades(&symbol()).unwrap();

        assert_eq!(
            ws.connected_urls(),
            vec!["wss://stream.binance.com:9443/ws".to_string()]
        );
        let sent = ws.sent();
        assert!(sent[0].contains(r#""method":"SUBSCRIBE""#));
        assert!(sent[0].contains("btcusdt@trade"));

        let events = binance.poll_events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::Trade(_)));
        // Draining again yields nothing.
        assert!(binance.poll_events().is_empty());
    }

    #[test]
    fn book_and_ticker_subscriptions_reuse_one_connection() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![]);
        let mut binance = streaming_client(&ws);
        binance.subscribe_book(&symbol()).unwrap();
        binance.subscribe_ticker(&symbol()).unwrap();

        // One connection, two SUBSCRIBE frames on the right channels.
        assert_eq!(ws.connected_urls().len(), 1);
        let sent = ws.sent();
        assert!(sent[0].contains("btcusdt@depth"));
        assert!(sent[1].contains("btcusdt@ticker"));
    }

    #[test]
    fn peer_close_triggers_reconnect_and_resubscribe() {
        let ws = Arc::new(MockWsTransport::new());
        // First stream: one trade, then a clean peer close.
        ws.push_connection(vec![
            Ok(Some(
                r#"{"e":"trade","s":"BTCUSDT","p":"100","q":"1","m":false,"T":1}"#.to_string(),
            )),
            Ok(None),
        ]);
        // Reconnect target: another trade.
        ws.push_connection(vec![Ok(Some(
            r#"{"e":"trade","s":"BTCUSDT","p":"101","q":"2","m":true,"T":2}"#.to_string(),
        ))]);

        let mut binance = streaming_client(&ws);
        binance.subscribe_trades(&symbol()).unwrap();

        // First poll drains the trade, sees the close, reconnects and resubscribes.
        let first = binance.poll_events();
        assert!(matches!(first[0], Event::Trade(_)));
        assert!(first.contains(&Event::Disconnected));
        assert!(first.contains(&Event::Reconnected));

        // The reconnect opened a second connection and replayed the SUBSCRIBE.
        assert_eq!(ws.connected_urls().len(), 2);
        let sent = ws.sent();
        assert_eq!(sent.len(), 2);
        assert!(sent[1].contains("btcusdt@trade"));

        // The fresh connection delivers its trade on the next poll.
        let second = binance.poll_events();
        assert!(matches!(second[0], Event::Trade(_)));
    }

    #[test]
    fn subscribe_without_ws_transport_errors() {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut binance = Binance::with_http(Box::new(ArcTransport(http)), &opts);
        assert!(matches!(
            binance.subscribe_trades(&symbol()).unwrap_err(),
            Error::NotConnected
        ));
    }

    #[test]
    fn poll_without_connection_is_empty() {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut binance = Binance::with_http(Box::new(ArcTransport(http)), &opts);
        assert!(binance.poll_events().is_empty());
    }

    #[test]
    fn split_wire_symbol_uses_known_quotes() {
        assert_eq!(split_wire_symbol("BTCUSDT"), Symbol::new("BTC", "USDT"));
        assert_eq!(split_wire_symbol("ETHFDUSD"), Symbol::new("ETH", "FDUSD"));
        assert_eq!(split_wire_symbol("ETHBTC"), Symbol::new("ETH", "BTC"));
        // Unknown quote -> whole string as the base.
        assert_eq!(split_wire_symbol("WEIRD"), Symbol::new("WEIRD", ""));
    }

    #[test]
    fn open_orders_filtered_and_unfiltered() {
        let (binance, mock) = signed_client(1000);
        // Filtered: the symbol is known from the caller.
        mock.push_json(
            200,
            r#"[{"symbol":"BTCUSDT","orderId":1,"clientOrderId":"a","price":"100.0",
            "origQty":"1.0","executedQty":"0.0","cummulativeQuoteQty":"0.0",
            "status":"NEW","type":"LIMIT","side":"BUY"}]"#,
        );
        let orders = binance.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].symbol, symbol());

        // Unfiltered: the symbol is resolved from the wire form.
        mock.push_json(
            200,
            r#"[{"symbol":"ETHUSDT","orderId":2,"clientOrderId":"","price":"0.0",
            "origQty":"2.0","executedQty":"0.0","cummulativeQuoteQty":"0.0",
            "status":"NEW","type":"MARKET","side":"SELL"}]"#,
        );
        let orders = binance.open_orders(None).unwrap();
        assert_eq!(orders[0].symbol, Symbol::new("ETH", "USDT"));
        let req = &mock.recorded_requests()[1];
        assert!(!req.url.contains("symbol="));
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        let mut exchange: Box<dyn Exchange> = Box::new(binance);
        assert_eq!(exchange.name(), "binance");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "28");
    }

    const EXCHANGE_INFO: &str = r#"{"symbols":[{"symbol":"BTCUSDT","baseAsset":"BTC",
        "quoteAsset":"USDT","baseAssetPrecision":8,"quoteAssetPrecision":8,"filters":[
        {"filterType":"LOT_SIZE","minQty":"0.001","maxQty":"1000","stepSize":"0.001"},
        {"filterType":"PRICE_FILTER","minPrice":"0.01","maxPrice":"1000000","tickSize":"0.01"},
        {"filterType":"NOTIONAL","minNotional":"10"}]}]}"#;

    #[test]
    fn load_instruments_populates_filters() {
        let (mut binance, mock) = signed_client(1000);
        mock.push_json(200, EXCHANGE_INFO);
        binance.load_instruments().unwrap();
        let inst = binance.instrument(&symbol()).unwrap();
        assert_eq!(inst.filters.step_size, dec!(0.001));
        assert_eq!(inst.filters.min_notional, dec!(10));
        assert_eq!(inst.filters.tick_size, dec!(0.01));
        assert_eq!(inst.base_precision, 8);
        // The request hit exchangeInfo with no query string.
        assert!(mock.recorded_requests()[0]
            .url
            .ends_with("/api/v3/exchangeInfo"));
    }

    #[test]
    fn place_order_rejects_filter_violation_when_loaded() {
        let (mut binance, mock) = signed_client(1000);
        mock.push_json(200, EXCHANGE_INFO);
        binance.load_instruments().unwrap();
        // quantity 0.0005 < min 0.001 -> rejected locally, no order sent.
        let err = binance
            .place_order(&OrderRequest::limit_buy(
                symbol(),
                dec!(0.0005),
                dec!(20000),
            ))
            .unwrap_err();
        assert!(matches!(err, Error::Filter(_)));
        assert_eq!(mock.recorded_requests().len(), 1); // only exchangeInfo
    }

    #[test]
    fn place_order_skips_filter_check_without_instruments() {
        let (binance, mock) = signed_client(1000);
        mock.push_json(200, ORDER_JSON);
        // No load_instruments: the order is sent (best effort).
        binance
            .place_order(&OrderRequest::limit_buy(
                symbol(),
                dec!(0.0005),
                dec!(20000),
            ))
            .unwrap();
        assert!(mock.recorded_requests()[0].url.contains("/api/v3/order"));
    }
}
