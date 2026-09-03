//! Bybit (v5 unified API) — the second exchange, proving the pattern scales.
//!
//! Like Binance it is generic over the injected [`HttpTransport`] and tested
//! offline against recorded responses. The shape is the same; the internals are
//! bespoke: Bybit wraps every response in a `{retCode, retMsg, result}` envelope,
//! uses a `category` (spot/linear/inverse) query parameter, reports klines
//! newest-first, and (in a later slice) signs with `timestamp + apiKey +
//! recvWindow + payload` rather than a signed query string.
//!
//! Covered here: the public REST market data (ticker, klines, depth), the
//! `{retCode, retMsg, result}` envelope handling, the error taxonomy,
//! `X-BAPI-*`-header signed execution (place/cancel/query/open orders, balances),
//! the pull-based WebSocket market streams (`op:subscribe`, topic-routed frames),
//! and the [`Exchange`] trait — so Bybit is usable as `Box<dyn Exchange>`.
//!
//! [`AdvancedOrders`] is native: STP via `smpType` on order create, amend via
//! `/v5/order/amend`, and batch place/cancel via `/v5/order/create-batch` and
//! `/v5/order/cancel-batch`. Bybit has no standalone OCO order-list (take-profit
//! / stop-loss attach to a position/order), so `place_oco` is a documented gap.

// Bybit `retCode`s are externally-defined numeric codes; grouping their digits
// with underscores would obscure them rather than aid reading.
#![allow(clippy::unreadable_literal)]

use crate::clock::ServerClock;
use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::events::{BookDelta, BookLevel, Event, OrderBookSnapshot, TradePrint};
use crate::feeds::{
    DerivativesChannel, DerivativesFeed, FundingRate, Liquidation, LongShortRatio, MarkIndex,
    OpenInterest,
};
use crate::normalize::{format_decimal, parse_decimal};
use crate::options::{ExchangeOptions, MarginMode, MarketType, PositionMode, SelfTradePrevention};
use crate::positions::{Position, PositionSide};
use crate::signing::hmac_sha256_hex;
use crate::symbol::Symbol;
use crate::traits::{
    AdvancedOrders, Derivatives, DerivativesStream, Exchange, Execution, MarketData, WsExecution,
    WsUserData,
};
use crate::transport::{HttpMethod, HttpRequest, HttpTransport, WsConnection, WsTransport};
use crate::types::{
    Balance, OcoRequest, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker,
    TimeInForce,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use wickra_core::Candle;

/// The current Unix time in milliseconds, from the system clock.
fn system_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

/// A Bybit client over an injected HTTP transport.
pub struct Bybit {
    http: Box<dyn HttpTransport>,
    ws: Option<Box<dyn WsTransport>>,
    rest_base: String,
    category: &'static str,
    /// The market this client was built for.
    ///
    /// Kept alongside `category` rather than derived from it: two market types
    /// map to the same category, so the string cannot say which was asked for.
    market_type: MarketType,
    /// One-way or hedge. Bybit names the side with `positionIdx`, which is only
    /// meaningful on a derivatives category.
    position_mode: PositionMode,
    testnet: bool,
    credentials: Option<Credentials>,
    recv_window_ms: u64,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    /// Offset between this machine's clock and the venue's, applied to every
    /// signed timestamp. Zero until [`sync_time`](Self::sync_time) is called.
    clock: ServerClock,
    connection: Option<Box<dyn WsConnection>>,
    sub_messages: Vec<String>,
    subscriptions: Vec<(String, Symbol)>,
    /// Which derivatives channels were subscribed, per wire symbol. Bybit
    /// carries funding, mark and index on the same `tickers` stream that
    /// feeds the ordinary ticker, so the frame alone cannot say which prints
    /// the caller wants out of it.
    derivatives_channels: Vec<(String, DerivativesChannel)>,
    /// The private user-data connection, opened by
    /// [`subscribe_user_data`](Self::subscribe_user_data) and drained by
    /// [`poll_events`](Self::poll_events) alongside the public stream.
    private_connection: Option<Box<dyn WsConnection>>,
    /// Set once the private stream is subscribed, so [`poll_events`](Self::poll_events)
    /// re-subscribes it after a drop.
    user_data_active: bool,
    /// A dedicated connection to the WebSocket trade API (`/v5/trade`), opened and
    /// authenticated lazily on the first [`place_order_ws`](Self::place_order_ws)
    /// / [`cancel_order_ws`](Self::cancel_order_ws) call.
    ws_api_connection: Option<Box<dyn WsConnection>>,
}

/// Hand-written: the client holds `Box<dyn HttpTransport>`, `Box<dyn WsTransport>`
/// and a `Box<dyn Fn() -> i64>` clock, none of which a derive can reach. The
/// transports are shown by whether a connection is open rather than by value, and
/// the credentials only by whether they are set -- printing them would put secret
/// material into every log line that formats a client.
impl fmt::Debug for Bybit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bybit")
            .field("ws", &self.ws.is_some())
            .field("rest_base", &self.rest_base)
            .field("category", &self.category)
            .field("testnet", &self.testnet)
            .field("authenticated", &self.credentials.is_some())
            .field("clock_offset_ms", &self.clock.offset_ms())
            .field("recv_window_ms", &self.recv_window_ms)
            .field("connection", &self.connection.is_some())
            .field("sub_messages", &self.sub_messages.len())
            .field("subscriptions", &self.subscriptions.len())
            .field("private_connection", &self.private_connection.is_some())
            .field("user_data_active", &self.user_data_active)
            .field("ws_api_connection", &self.ws_api_connection.is_some())
            .finish_non_exhaustive()
    }
}

impl Bybit {
    /// Refuse a market this client does not route.
    ///
    /// Called at every seam that reaches the venue -- the HTTP helpers and each
    /// WebSocket connect -- so an unrouted market is refused before a request
    /// is built rather than answered by whichever market the URL happened to
    /// name. See [`ensure_market_is_routed`](super::ensure_market_is_routed)
    /// for what each client used to answer instead.
    fn ensure_market_is_routed(&self) -> Result<()> {
        super::ensure_market_is_routed("Bybit", self.market_type, super::SPOT_AND_LINEAR)
    }

    fn build(
        http: Box<dyn HttpTransport>,
        options: &ExchangeOptions,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            http,
            ws: None,
            rest_base: if options.testnet {
                "https://api-testnet.bybit.com".to_string()
            } else {
                "https://api.bybit.com".to_string()
            },
            category: category(options.market_type),
            market_type: options.market_type,
            position_mode: options.position_mode,
            testnet: options.testnet,
            credentials,
            recv_window_ms: options.recv_window_ms,
            now_ms: Box::new(system_now_ms),
            clock: ServerClock::new(),
            connection: None,
            sub_messages: Vec::new(),
            subscriptions: Vec::new(),
            derivatives_channels: Vec::new(),
            private_connection: None,
            user_data_active: false,
            ws_api_connection: None,
        }
    }

    /// The timestamp a signed request must carry: this machine's clock plus the
    /// offset learned from the venue.
    ///
    /// A venue rejects a signed request whose timestamp falls outside its own
    /// receive window, so a machine a few seconds off has every order refused --
    /// with a message about the window rather than about the clock. Until
    /// [`sync_time`](Self::sync_time) is called the offset is zero and this is
    /// the local time, which is the previous behaviour.
    fn signed_now_ms(&self) -> i64 {
        self.clock.server_time_ms((self.now_ms)())
    }

    /// Learn the offset between this machine's clock and the venue's, from
    /// `GET /v5/market/time`.
    ///
    /// Explicit rather than automatic: it costs a request, and a client should
    /// not make one the caller did not ask for. Call it once after connecting,
    /// and again if the process runs long enough for drift to matter.
    ///
    /// Returns the new offset in milliseconds (`server - local`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the response cannot be parsed.
    pub fn sync_time(&mut self) -> Result<i64> {
        // `get` unwraps the envelope to its `result`, so the top-level `time`
        // is not reachable here. `timeNano` is inside `result` and is a
        // nanosecond string; `timeSecond` beside it would drop sub-second
        // precision, which is the precision this offset exists to recover.
        let local_before = (self.now_ms)();
        let value = self.get("/v5/market/time", "")?;
        let nanos: i64 = value
            .get("timeNano")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Deserialization("market/time: no timeNano".into()))?
            .parse()
            .map_err(|_| Error::Deserialization("market/time: timeNano not an integer".into()))?;
        self.clock.sync(local_before, nanos / 1_000_000);
        Ok(self.clock.offset_ms())
    }

    /// Attach a WebSocket transport, enabling the streaming subscriptions.
    #[must_use]
    pub fn with_ws(mut self, ws: Box<dyn WsTransport>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Build a public Bybit client over the given transport and options.
    #[must_use]
    pub fn with_http(http: Box<dyn HttpTransport>, options: &ExchangeOptions) -> Self {
        Self::build(http, options, None)
    }

    /// Build an authenticated Bybit client for signed endpoints.
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

    /// Whether this client targets a futures category. Funding, mark/index and
    /// liquidations exist only there.
    fn is_futures(&self) -> bool {
        self.category != "spot"
    }

    /// Refuse `reduce_only` on a spot client, on every path that takes an order.
    ///
    /// A spot account holds balances, not positions, so there is nothing for a
    /// reduce-only order to reduce. Bybit's `reduceOnly` belongs to the `linear`
    /// and `inverse` categories; the spot endpoint takes the field and does not
    /// act on it, which is worse than rejecting it -- the caller is told the
    /// order can only close, and it can open.
    fn ensure_reduce_only_is_reducible(&self, request: &OrderRequest) -> Result<()> {
        if request.reduce_only && !self.is_futures() {
            return Err(Error::unsupported_field(
                "Bybit",
                "reduce_only on a spot order",
                "spot holds balances, not positions, and has none to reduce",
            ));
        }
        Ok(())
    }

    /// The Bybit product category this client targets (`spot`/`linear`/`inverse`).
    #[must_use]
    pub fn category(&self) -> &'static str {
        self.category
    }

    /// The Bybit wire symbol for a canonical [`Symbol`] (`BTC/USDT` -> `BTCUSDT`).
    #[must_use]
    pub fn wire_symbol(symbol: &Symbol) -> String {
        symbol.to_concatenated()
    }

    /// A ticker for `symbol`.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or the symbol is unknown.
    pub fn ticker(&self, symbol: &Symbol) -> Result<Ticker> {
        let query = format!(
            "category={}&symbol={}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.get("/v5/market/tickers", &query)?;
        let raw: TickerList =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        let entry = raw
            .list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no ticker for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: parse_decimal(&entry.last_price)?,
            bid: parse_decimal(&entry.bid1_price)?,
            ask: parse_decimal(&entry.ask1_price)?,
            volume: parse_decimal(&entry.volume24h)?,
            // Bybit's ticker entries carry no timestamp; the envelope's `time`
            // is when the server replied, not when the quote was true.
            timestamp: 0,
        })
    }

    /// Up to `limit` candles for `symbol` at `interval` (unified, e.g. `"1m"`,
    /// `"1h"`, `"1d"`). Bybit returns newest-first; the result is chronological.
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails or a row cannot be parsed.
    pub fn klines(&self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let query = format!(
            "category={}&symbol={}&interval={}&limit={limit}",
            self.category,
            Self::wire_symbol(symbol),
            map_interval(interval),
        );
        let result = self.get("/v5/market/kline", &query)?;
        let raw: KlineList =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        let mut candles = raw
            .list
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
        let query = format!(
            "category={}&symbol={}&limit={depth}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.get("/v5/market/orderbook", &query)?;
        let raw: RawDepth =
            serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))?;
        Ok(OrderBookSnapshot {
            symbol: symbol.clone(),
            last_update_id: raw.update_id,
            bids: parse_levels(&raw.bids)?,
            asks: parse_levels(&raw.asks)?,
            timestamp: raw.ts,
        })
    }

    /// Subscribe to the public trade stream for `symbol`.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] if no WebSocket transport is configured,
    /// or a transport error if the connection or subscription fails.
    pub fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("publicTrade.{}", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    /// Subscribe to the order-book stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("orderbook.50.{}", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    /// Subscribe to the ticker stream for `symbol`.
    ///
    /// # Errors
    /// See [`subscribe_trades`](Self::subscribe_trades).
    pub fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        let topic = format!("tickers.{}", Self::wire_symbol(symbol));
        self.subscribe(symbol, &topic)
    }

    /// Subscribe to a pushed derivatives channel.
    ///
    /// Bybit publishes funding, mark and index on the **same `tickers` stream**
    /// that carries the ordinary ticker -- one topic answering four questions --
    /// so both channels resolve to it and the parser emits whichever the caller
    /// subscribed to. Liquidations are `allLiquidation`, which reports the taker
    /// side of the forced order rather than the side of the position that was
    /// closed; that is the side this crate's [`Liquidation`] carries, so it maps
    /// straight through.
    ///
    /// These streams exist only on the linear (futures) venue.
    ///
    /// # Errors
    /// Returns an [`Error`] on a spot client, or if the subscription fails.
    pub fn subscribe_derivatives(
        &mut self,
        symbol: &Symbol,
        channel: DerivativesChannel,
    ) -> Result<()> {
        if !self.is_futures() {
            return Err(Error::unsupported_field(
                "Bybit",
                "a derivatives channel on a spot client",
                "funding, mark/index and liquidations exist only on the linear venue",
            ));
        }
        let wire = Self::wire_symbol(symbol);
        let topic = match channel {
            DerivativesChannel::Funding | DerivativesChannel::MarkIndex => {
                format!("tickers.{wire}")
            }
            DerivativesChannel::Liquidations => format!("allLiquidation.{wire}"),
        };
        self.derivatives_channels.push((wire, channel));
        self.subscribe(symbol, &topic)
    }

    /// The current open interest (`GET /v5/market/open-interest`), most recent
    /// point.
    ///
    /// Bybit does push open interest, on the `tickers` stream -- but only as one
    /// field among many, and only when it changes. Reading it is what gives a
    /// caller a figure at a known moment rather than whenever the venue last
    /// happened to include it.
    ///
    /// # Errors
    /// Returns an [`Error`] on a spot client, if the request fails, or if the
    /// venue returns no data point.
    pub fn open_interest(&self, symbol: &Symbol) -> Result<OpenInterest> {
        if !self.is_futures() {
            return Err(Error::unsupported_field(
                "Bybit",
                "open interest on a spot client",
                "open interest is a futures figure",
            ));
        }
        let query = format!(
            "category=linear&symbol={}&intervalTime=5min&limit=1",
            Self::wire_symbol(symbol)
        );
        let value = self.get("/v5/market/open-interest", &query)?;
        let result = parse_result::<serde_json::Value>(value)?;
        let point = result
            .get("list")
            .and_then(serde_json::Value::as_array)
            .and_then(|list| list.first())
            .ok_or_else(|| Error::NotFound("no open-interest data point".to_string()))?;
        Ok(OpenInterest {
            symbol: symbol.clone(),
            open_interest: parse_decimal(field_str(point, "openInterest")?)?,
            timestamp: field_str(point, "timestamp")
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0),
        })
    }

    /// The current long/short account ratio (`GET /v5/market/account-ratio`),
    /// most recent point.
    ///
    /// Reported as account proportions summing to one, carried through as given.
    ///
    /// # Errors
    /// Returns an [`Error`] on a spot client, if the request fails, or if the
    /// venue returns no data point.
    pub fn long_short_ratio(&self, symbol: &Symbol) -> Result<LongShortRatio> {
        if !self.is_futures() {
            return Err(Error::unsupported_field(
                "Bybit",
                "long/short positioning on a spot client",
                "positioning is a futures figure",
            ));
        }
        let query = format!(
            "category=linear&symbol={}&period=5min&limit=1",
            Self::wire_symbol(symbol)
        );
        let value = self.get("/v5/market/account-ratio", &query)?;
        let result = parse_result::<serde_json::Value>(value)?;
        let point = result
            .get("list")
            .and_then(serde_json::Value::as_array)
            .and_then(|list| list.first())
            .ok_or_else(|| Error::NotFound("no long/short data point".to_string()))?;
        Ok(LongShortRatio {
            symbol: symbol.clone(),
            long_size: parse_decimal(field_str(point, "buyRatio")?)?,
            short_size: parse_decimal(field_str(point, "sellRatio")?)?,
            timestamp: field_str(point, "timestamp")
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0),
        })
    }

    /// Open the connection if needed, send an `op:subscribe` for `topic`, and
    /// register the symbol for wire-name resolution.
    fn subscribe(&mut self, symbol: &Symbol, topic: &str) -> Result<()> {
        self.ensure_market_is_routed()?;
        let wire = Self::wire_symbol(symbol);
        if self.connection.is_none() {
            let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
            let connection = ws.connect(&ws_base_url(self.category, self.testnet))?;
            self.connection = Some(connection);
        }
        let message = format!(r#"{{"op":"subscribe","args":["{topic}"]}}"#);
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

    /// Drain all stream events available since the last call. Non-blocking;
    /// frames that fail to parse are skipped.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let subscriptions: HashMap<String, Symbol> = self.subscriptions.iter().cloned().collect();
        let resolve = |wire: &str| {
            subscriptions
                .get(wire)
                .cloned()
                .unwrap_or_else(|| Symbol::new(wire, ""))
        };
        let channels = self.derivatives_channels.clone();
        let mut events = Vec::new();
        if let Some(connection) = self.connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
                // Parsed separately: a `tickers` frame is both an ordinary
                // ticker and, on the linear venue, a funding and mark/index
                // print. One frame, up to three subscriptions.
                events.extend(parse_derivatives_frame(&frame, &resolve, &channels));
            }
        }
        // Drain the private user-data stream (order/wallet topics), if open.
        if let Some(connection) = self.private_connection.as_mut() {
            while let Ok(Some(frame)) = connection.recv() {
                if let Ok(mut parsed) = parse_ws_message(&frame, &resolve) {
                    events.append(&mut parsed);
                }
            }
        }
        // A dropped private stream is re-subscribed with a fresh op:auth (the
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
        let url = ws_base_url(self.category, self.testnet);
        crate::wsutil::reconnect_if_dropped(
            self.ws.as_deref(),
            &url,
            &mut self.connection,
            &self.sub_messages,
            &mut events,
        );
        events
    }

    /// Open the private user-data stream (`wss://.../v5/private`). Authenticates
    /// with an `op:auth` frame (signature = HMAC-SHA256 over `GET/realtime<expires>`),
    /// then subscribes to the `order` and `wallet` topics. Afterwards
    /// [`poll_events`](Self::poll_events) also surfaces the account's own
    /// [`Event::OrderUpdate`] and [`Event::BalanceUpdate`].
    ///
    /// A dropped private stream is re-subscribed automatically on the next
    /// [`poll_events`](Self::poll_events); call
    /// [`keepalive_user_data`](Self::keepalive_user_data) periodically to keep it
    /// from being dropped for inactivity.
    ///
    /// # Errors
    /// Returns [`Error::InvalidCredentials`] without credentials, [`Error::NotConnected`]
    /// without a WebSocket transport, or another [`Error`] if the request fails.
    pub fn subscribe_user_data(&mut self) -> Result<()> {
        self.ensure_market_is_routed()?;
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "user-data stream requires credentials",
        ))?;
        // Bybit signs `GET/realtime<expires>`; the auth is valid until `expires`.
        let expires = self.signed_now_ms() + i64::try_from(self.recv_window_ms).unwrap_or(5000);
        let signature = hmac_sha256_hex(
            creds.api_secret.as_bytes(),
            format!("GET/realtime{expires}").as_bytes(),
        );
        let auth = format!(
            r#"{{"op":"auth","args":["{}",{expires},"{signature}"]}}"#,
            creds.api_key
        );
        let subscribe = r#"{"op":"subscribe","args":["order","wallet"]}"#.to_string();
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect(&ws_private_url(self.testnet))?;
        connection.send(&auth)?;
        connection.send(&subscribe)?;
        self.private_connection = Some(connection);
        self.user_data_active = true;
        Ok(())
    }

    /// Send an application-level heartbeat (`op:ping`) on the private stream so it
    /// is not dropped for inactivity. A no-op before
    /// [`subscribe_user_data`](Self::subscribe_user_data).
    ///
    /// # Errors
    /// Returns an [`Error`] if the ping cannot be sent.
    pub fn keepalive_user_data(&mut self) -> Result<()> {
        if let Some(connection) = self.private_connection.as_mut() {
            connection.send(r#"{"op":"ping"}"#)?;
        }
        Ok(())
    }

    /// Place an order. Validated locally, then sent signed. Bybit's create
    /// endpoint returns only the ids, so the resulting [`Order`] carries the
    /// request's own fields with the venue order id and a `New` status.
    ///
    /// # Errors
    /// Returns an [`Error`] if the order is invalid, credentials are missing, or
    /// the venue rejects it.
    pub fn place_order(&self, request: &OrderRequest) -> Result<Order> {
        self.ensure_reduce_only_is_reducible(request)?;
        request.validate()?;
        let time_in_force = tif_for(request)?;
        let mut body = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "orderType": order_type_str(request.order_type),
            "qty": format_decimal(request.quantity),
            "timeInForce": time_in_force,
        });
        if let Some(price) = request.price {
            body["price"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            body["orderLinkId"] = serde_json::json!(id.clone());
        }
        if request.reduce_only {
            body["reduceOnly"] = serde_json::json!(true);
        }
        if let Some(idx) = self.position_idx(request) {
            body["positionIdx"] = serde_json::json!(idx);
        }
        if let Some(smp) = smp_str(request.stp) {
            body["smpType"] = serde_json::json!(smp);
        }
        if request.order_type.is_trigger() {
            for (key, value) in trigger_fields(request, self.category)? {
                body[key] = value;
            }
        }
        let result =
            self.signed_request(HttpMethod::Post, "/v5/order/create", "", &body.to_string())?;
        let created: CreateResult = parse_result(result)?;
        Ok(Order {
            id: created.order_id,
            client_order_id: (!created.order_link_id.is_empty()).then_some(created.order_link_id),
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
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "orderId": order_id,
        });
        self.signed_request(HttpMethod::Post, "/v5/order/cancel", "", &body.to_string())?;
        Ok(())
    }

    /// Place an order over the Bybit WebSocket trade API (`order.create`). Builds
    /// the same args as the REST path and exchanges them on the lazily-opened,
    /// authenticated `/v5/trade` connection. Bybit returns only the ids, so the
    /// resulting [`Order`] carries the request's fields with a `New` status.
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the order is invalid or rejected.
    pub fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        if request.order_type.is_trigger() {
            return Err(Error::unsupported_trigger("Bybit"));
        }
        self.ensure_reduce_only_is_reducible(request)?;
        request.validate()?;
        let time_in_force = tif_for(request)?;
        let mut arg = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(&request.symbol),
            "side": side_str(request.side),
            "orderType": order_type_str(request.order_type),
            "qty": format_decimal(request.quantity),
            "timeInForce": time_in_force,
        });
        if let Some(price) = request.price {
            arg["price"] = serde_json::json!(format_decimal(price));
        }
        if let Some(id) = &request.client_order_id {
            arg["orderLinkId"] = serde_json::json!(id.clone());
        }
        if request.reduce_only {
            arg["reduceOnly"] = serde_json::json!(true);
        }
        if let Some(smp) = smp_str(request.stp) {
            arg["smpType"] = serde_json::json!(smp);
        }
        if let Some(idx) = self.position_idx(request) {
            arg["positionIdx"] = serde_json::json!(idx);
        }
        let data = self.ws_trade_request("order.create", &arg)?;
        let created: CreateResult =
            serde_json::from_value(data).map_err(|e| Error::Deserialization(e.to_string()))?;
        Ok(Order {
            id: created.order_id,
            client_order_id: (!created.order_link_id.is_empty()).then_some(created.order_link_id),
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

    /// Cancel an order over the Bybit WebSocket trade API (`order.cancel`).
    ///
    /// # Errors
    /// Returns [`Error::NotConnected`] without a WebSocket transport, or another
    /// [`Error`] if the order is unknown or the request fails.
    pub fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        let arg = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "orderId": order_id,
        });
        self.ws_trade_request("order.cancel", &arg)?;
        Ok(())
    }

    /// Open and authenticate the `/v5/trade` connection if needed. Sends the
    /// `op:auth` frame (same signature as the private stream) and consumes the
    /// auth acknowledgement so later requests read their own responses.
    fn ensure_ws_trade(&mut self) -> Result<()> {
        self.ensure_market_is_routed()?;
        if self.ws_api_connection.is_some() {
            return Ok(());
        }
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "WebSocket trade requires credentials",
        ))?;
        let expires = self.signed_now_ms() + i64::try_from(self.recv_window_ms).unwrap_or(5000);
        let signature = hmac_sha256_hex(
            creds.api_secret.as_bytes(),
            format!("GET/realtime{expires}").as_bytes(),
        );
        let auth = format!(
            r#"{{"op":"auth","args":["{}",{expires},"{signature}"]}}"#,
            creds.api_key
        );
        let ws = self.ws.as_ref().ok_or(Error::NotConnected)?;
        let mut connection = ws.connect(&ws_trade_url(self.testnet))?;
        connection.send(&auth)?;
        // Consume the auth acknowledgement.
        connection.recv()?;
        self.ws_api_connection = Some(connection);
        Ok(())
    }

    /// Send a signed trade request frame and return its `data`, mapping a non-zero
    /// `retCode` onto the error taxonomy.
    fn ws_trade_request(&mut self, op: &str, arg: &serde_json::Value) -> Result<serde_json::Value> {
        self.ensure_ws_trade()?;
        let now = (self.now_ms)();
        let frame = serde_json::json!({
            "reqId": now.to_string(),
            "header": {
                "X-BAPI-TIMESTAMP": now.to_string(),
                "X-BAPI-RECV-WINDOW": self.recv_window_ms.to_string(),
            },
            "op": op,
            "args": [arg],
        })
        .to_string();
        let connection = self
            .ws_api_connection
            .as_mut()
            .expect("ws-trade connection just ensured");
        connection.send(&frame)?;
        let Some(response) = connection.recv()? else {
            return Err(Error::NotConnected);
        };
        let value: serde_json::Value =
            serde_json::from_str(&response).map_err(|e| Error::Deserialization(e.to_string()))?;
        let ret_code = value
            .get("retCode")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        if ret_code == 0 {
            Ok(value
                .get("data")
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        } else {
            let message = value
                .get("retMsg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            Err(Error::OrderRejected {
                code: ret_code.to_string(),
                message,
            })
        }
    }

    /// Query a single order by venue id.
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the order is unknown.
    pub fn query_order(&self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        let query = format!(
            "category={}&symbol={}&orderId={order_id}",
            self.category,
            Self::wire_symbol(symbol)
        );
        let result = self.signed_request(HttpMethod::Get, "/v5/order/realtime", &query, "")?;
        let list: OrderList = parse_result(result)?;
        let raw = list
            .list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
        order_from_raw(symbol.clone(), &raw)
    }

    /// All open orders, optionally filtered to one `symbol`. When unfiltered, the
    /// venue's wire symbol is mapped back to a canonical [`Symbol`].
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn open_orders(&self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        let mut query = format!("category={}", self.category);
        if let Some(s) = symbol {
            query.push_str("&symbol=");
            query.push_str(&Self::wire_symbol(s));
        }
        let result = self.signed_request(HttpMethod::Get, "/v5/order/realtime", &query, "")?;
        let list: OrderList = parse_result(result)?;
        list.list
            .iter()
            .map(|raw| {
                let sym = symbol
                    .cloned()
                    .unwrap_or_else(|| split_wire_symbol(&raw.symbol));
                order_from_raw(sym, raw)
            })
            .collect()
    }

    /// Unified-account balances (assets are reported as the venue lists them).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn balances(&self) -> Result<Vec<Balance>> {
        let result = self.signed_request(
            HttpMethod::Get,
            "/v5/account/wallet-balance",
            "accountType=UNIFIED",
            "",
        )?;
        let wallet: WalletBalance = parse_result(result)?;
        let account = wallet
            .list
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound("no wallet account".to_string()))?;
        Ok(account
            .coin
            .iter()
            .map(|c| Balance {
                asset: c.coin.clone(),
                free: dec_or_zero(&c.available_to_withdraw),
                locked: dec_or_zero(&c.locked),
            })
            .collect())
    }

    /// GET a public endpoint and unwrap the `{retCode, retMsg, result}` envelope.
    fn get(&self, path: &str, query: &str) -> Result<serde_json::Value> {
        self.ensure_market_is_routed()?;
        let url = format!("{}{path}?{query}", self.rest_base);
        let response = self.http.execute(&HttpRequest::get(url))?;
        unwrap_envelope(&response.body).map_err(|e| e.with_retry_after(response.retry_after()))
    }

    /// Sign a request with the Bybit `X-BAPI-*` header scheme: HMAC-SHA256 over
    /// `timestamp + apiKey + recvWindow + (query for GET, body for POST)`.
    fn signed_request(
        &self,
        method: HttpMethod,
        path: &str,
        query: &str,
        body: &str,
    ) -> Result<serde_json::Value> {
        self.ensure_market_is_routed()?;
        let creds = self.credentials.as_ref().ok_or(Error::InvalidCredentials(
            "signed endpoint requires credentials",
        ))?;
        let timestamp = self.signed_now_ms().to_string();
        let recv_window = self.recv_window_ms.to_string();
        let payload = if body.is_empty() { query } else { body };
        let sign_input = format!("{timestamp}{}{recv_window}{payload}", creds.api_key);
        let signature = hmac_sha256_hex(creds.api_secret.as_bytes(), sign_input.as_bytes());
        let url = if query.is_empty() {
            format!("{}{path}", self.rest_base)
        } else {
            format!("{}{path}?{query}", self.rest_base)
        };
        let mut request = HttpRequest::new(method, url)
            .with_header("X-BAPI-API-KEY", creds.api_key.clone())
            .with_header("X-BAPI-TIMESTAMP", timestamp)
            .with_header("X-BAPI-RECV-WINDOW", recv_window)
            .with_header("X-BAPI-SIGN", signature);
        if !body.is_empty() {
            request = request
                .with_header("Content-Type", "application/json")
                .with_body(body.to_string());
        }
        let response = self.http.execute(&request)?;
        unwrap_envelope(&response.body).map_err(|e| e.with_retry_after(response.retry_after()))
    }
}

/// The Bybit product category for a market type.
impl Bybit {
    /// Bybit's `positionIdx`, or `None` when the order does not need one.
    ///
    /// `0` is one-way and is also the venue default, so it is left off rather
    /// than sent. In hedge mode the buy side of the account is `1` and the sell
    /// side is `2`, and an order has to say which it acts on. Spot has no
    /// position index at all.
    fn position_idx(&self, request: &OrderRequest) -> Option<u8> {
        if self.category == "spot" || self.position_mode == PositionMode::OneWay {
            return None;
        }
        Some(
            match PositionSide::for_order(request.side, request.reduce_only) {
                PositionSide::Long => 1,
                PositionSide::Short => 2,
            },
        )
    }
}

fn category(market_type: MarketType) -> &'static str {
    match market_type {
        MarketType::Spot | MarketType::Margin => "spot",
        MarketType::UsdMFutures => "linear",
        MarketType::CoinMFutures => "inverse",
    }
}

/// Map a unified interval (`1m`/`1h`/`1d`) to Bybit's format (`1`/`60`/`D`).
fn map_interval(interval: &str) -> String {
    match interval {
        "1m" => "1",
        "3m" => "3",
        "5m" => "5",
        "15m" => "15",
        "30m" => "30",
        "1h" => "60",
        "2h" => "120",
        "4h" => "240",
        "6h" => "360",
        "12h" => "720",
        "1d" => "D",
        "1w" => "W",
        "1M" => "M",
        other => other,
    }
    .to_string()
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "retCode")]
    ret_code: i64,
    #[serde(rename = "retMsg", default)]
    ret_msg: String,
    #[serde(default)]
    result: serde_json::Value,
}

#[derive(Deserialize)]
struct TickerList {
    list: Vec<RawTicker>,
}

#[derive(Deserialize)]
struct RawTicker {
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "bid1Price")]
    bid1_price: String,
    #[serde(rename = "ask1Price")]
    ask1_price: String,
    #[serde(rename = "volume24h")]
    volume24h: String,
}

#[derive(Deserialize)]
struct KlineList {
    list: Vec<Vec<String>>,
}

#[derive(Deserialize)]
struct RawDepth {
    #[serde(rename = "u")]
    update_id: u64,
    /// Verified against the live endpoint: the book result is stamped.
    #[serde(default)]
    ts: i64,
    #[serde(rename = "b")]
    bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    asks: Vec<[String; 2]>,
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
    // Bybit kline: [startTime, open, high, low, close, volume, turnover].
    if row.len() < 6 {
        return Err(Error::Deserialization("kline row too short".to_string()));
    }
    let start = row[0]
        .parse::<i64>()
        .map_err(|e| Error::Deserialization(format!("kline start not an integer: {e}")))?;
    let f = |i: usize| -> Result<f64> {
        row[i]
            .parse::<f64>()
            .map_err(|e| Error::Deserialization(format!("kline field not a number: {e}")))
    };
    Candle::new(f(1)?, f(2)?, f(3)?, f(4)?, f(5)?, start)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

/// Map a Bybit `retCode` onto the unified error taxonomy.
fn map_error(ret_code: i64, ret_msg: &str) -> Error {
    match ret_code {
        10001 | 10004 | 10005 | 33004 => Error::Auth(ret_msg.to_string()),
        10006 | 10018 => Error::RateLimited { retry_after: None },
        110004 | 110007 | 170131 => Error::InsufficientBalance,
        110001 | 170213 => Error::NotFound(ret_msg.to_string()),
        _ => Error::Exchange {
            code: ret_code.to_string(),
            message: ret_msg.to_string(),
        },
    }
}

fn unwrap_envelope(body: &str) -> Result<serde_json::Value> {
    // Bybit returns HTTP 200 even for API errors; the retCode carries the real
    // status, so parse the envelope regardless of the HTTP code.
    let envelope: Envelope =
        serde_json::from_str(body).map_err(|e| Error::Deserialization(e.to_string()))?;
    if envelope.ret_code != 0 {
        return Err(map_error(envelope.ret_code, &envelope.ret_msg));
    }
    Ok(envelope.result)
}

/// Pull `topic` and `data` out of a raw frame and hand them to
/// [`parse_derivatives_message`]. A frame that is not JSON, or carries no topic,
/// yields nothing.
fn parse_derivatives_frame(
    text: &str,
    resolve: &impl Fn(&str) -> Symbol,
    channels: &[(String, DerivativesChannel)],
) -> Vec<Event> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(topic) = value.get("topic").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    let Some(data) = value.get("data") else {
        return Vec::new();
    };
    parse_derivatives_message(topic, data, resolve, channels)
}

/// Parse one Bybit WebSocket frame into the derivatives events the caller
/// subscribed to.
///
/// Bybit's `tickers` frames are *deltas*: after the first snapshot only the
/// fields that changed are present. So a funding print is emitted only when the
/// frame actually carries a rate, and a mark/index print only when it carries
/// both prices. Filling a missing field with the last seen value would report a
/// figure as current that the venue did not just publish, and filling it with
/// zero would report a funding rate of zero, which is a number a strategy acts
/// on.
fn parse_derivatives_message(
    topic: &str,
    data: &serde_json::Value,
    resolve: &impl Fn(&str) -> Symbol,
    channels: &[(String, DerivativesChannel)],
) -> Vec<Event> {
    let subscribed = |wire: &str, channel: DerivativesChannel| {
        channels.iter().any(|(w, c)| w == wire && *c == channel)
    };
    let mut out = Vec::new();
    if topic.starts_with("tickers.") {
        let Ok(wire) = field_str(data, "symbol") else {
            return out;
        };
        let symbol = resolve(wire);
        let timestamp = data
            .get("ts")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        // A missing field is an empty string here, which does not parse -- so an
        // absent price is absent rather than zero.
        let mark = parse_decimal(opt_str(data, "markPrice")).ok();
        if subscribed(wire, DerivativesChannel::Funding) {
            if let (Some(rate), Some(mark_price)) =
                (parse_decimal(opt_str(data, "fundingRate")).ok(), mark)
            {
                out.push(Event::Derivatives(DerivativesFeed::Funding(FundingRate {
                    symbol: symbol.clone(),
                    rate,
                    mark_price,
                    timestamp,
                })));
            }
        }
        if subscribed(wire, DerivativesChannel::MarkIndex) {
            if let (Some(mark_price), Some(index_price)) =
                (mark, parse_decimal(opt_str(data, "indexPrice")).ok())
            {
                out.push(Event::Derivatives(DerivativesFeed::MarkIndex(MarkIndex {
                    symbol,
                    mark_price,
                    index_price,
                    timestamp,
                })));
            }
        }
    } else if topic.starts_with("allLiquidation.") {
        let Some(prints) = data.as_array() else {
            return out;
        };
        for print in prints {
            let Ok(wire) = field_str(print, "s") else {
                continue;
            };
            if !subscribed(wire, DerivativesChannel::Liquidations) {
                continue;
            }
            // `S` on `allLiquidation` is the taker side of the forced order --
            // the side hitting the book -- which is what `Liquidation` carries.
            // The older `liquidation` topic reported the *position* side, the
            // opposite; reading one as the other inverts every liquidation-flow
            // figure computed downstream.
            let Ok(side) = parse_side(field_str(print, "S").unwrap_or("")) else {
                continue;
            };
            let (Ok(price), Ok(quantity)) = (
                field_str(print, "p").and_then(parse_decimal),
                field_str(print, "v").and_then(parse_decimal),
            ) else {
                continue;
            };
            out.push(Event::Derivatives(DerivativesFeed::Liquidation(
                Liquidation {
                    symbol: resolve(wire),
                    side,
                    price,
                    quantity,
                    timestamp: print
                        .get("T")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                },
            )));
        }
    }
    out
}

fn parse_result<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "Buy",
        OrderSide::Sell => "Sell",
    }
}

/// The fields that turn a Bybit order into a conditional one.
///
/// `triggerDirection` says which way the market has to cross the trigger, and
/// Bybit will not infer it: 1 is "rises to", 2 is "falls to". A sell stop
/// protects a long and fires on the way *down*; a buy stop covers a short and
/// fires on the way *up*. Sending the wrong direction arms the order on the
/// side that never comes, so the stop simply never fires.
///
/// Spot conditional orders also need `orderFilter`, because Bybit's spot
/// endpoint serves plain and conditional orders through the same call and
/// defaults to the plain one -- a trigger sent without it is accepted and
/// placed immediately.
fn trigger_fields(
    request: &OrderRequest,
    category: &str,
) -> Result<Vec<(&'static str, serde_json::Value)>> {
    let stop = request
        .stop_price
        .ok_or(Error::InvalidOrder("a trigger order requires a stop price"))?;
    let direction = match request.side {
        // Selling to protect a long: the market has to fall to the trigger.
        OrderSide::Sell => 2,
        // Buying to cover a short: it has to rise to it.
        OrderSide::Buy => 1,
    };
    let mut fields = vec![
        ("triggerPrice", serde_json::json!(format_decimal(stop))),
        ("triggerDirection", serde_json::json!(direction)),
    ];
    if category == "spot" {
        fields.push(("orderFilter", serde_json::json!("StopOrder")));
    }
    Ok(fields)
}

fn order_type_str(order_type: OrderType) -> &'static str {
    match order_type {
        OrderType::Market | OrderType::StopMarket => "Market",
        OrderType::Limit | OrderType::StopLimit => "Limit",
    }
}

/// The Bybit `timeInForce` value for a request. `PostOnly` is one of the values
/// that field takes, alongside `GTC`/`IOC`/`FOK`, so setting both asks for two
/// things in one slot: that is refused rather than resolved by dropping the
/// time-in-force, which is what these builders used to do silently.
fn tif_for(request: &OrderRequest) -> Result<&'static str> {
    match (request.post_only, request.time_in_force) {
        (true, TimeInForce::Gtc) => Ok("PostOnly"),
        (true, _) => Err(Error::unsupported_field(
            "Bybit",
            "post_only together with a non-GTC time-in-force",
            "`PostOnly` is one of the values Bybit's `timeInForce` field takes",
        )),
        (false, tif) => Ok(tif_str(tif)),
    }
}

fn tif_str(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
    }
}

/// The Bybit `smpType` value for a self-trade-prevention policy, or `None` to
/// omit it. Bybit expresses the maker/taker/both choice as `Cancel*`.
fn smp_str(stp: SelfTradePrevention) -> Option<&'static str> {
    match stp {
        SelfTradePrevention::None => None,
        SelfTradePrevention::ExpireMaker => Some("CancelMaker"),
        SelfTradePrevention::ExpireTaker => Some("CancelTaker"),
        SelfTradePrevention::ExpireBoth => Some("CancelBoth"),
    }
}

fn parse_side(raw: &str) -> Result<OrderSide> {
    match raw {
        "Buy" => Ok(OrderSide::Buy),
        "Sell" => Ok(OrderSide::Sell),
        other => Err(Error::Deserialization(format!("unknown side {other:?}"))),
    }
}

fn parse_order_type(raw: &str) -> Result<OrderType> {
    match raw {
        "Market" => Ok(OrderType::Market),
        "Limit" => Ok(OrderType::Limit),
        other => Err(Error::Deserialization(format!(
            "unknown order type {other:?}"
        ))),
    }
}

fn parse_status(raw: &str) -> Result<OrderStatus> {
    match raw {
        "New" | "Untriggered" | "Triggered" => Ok(OrderStatus::New),
        "PartiallyFilled" => Ok(OrderStatus::PartiallyFilled),
        "Filled" => Ok(OrderStatus::Filled),
        "Cancelled" | "PartiallyFilledCanceled" | "Deactivated" => Ok(OrderStatus::Canceled),
        "Rejected" => Ok(OrderStatus::Rejected),
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

fn order_from_raw(symbol: Symbol, raw: &RawOrder) -> Result<Order> {
    Ok(Order {
        id: raw.order_id.clone(),
        client_order_id: (!raw.order_link_id.is_empty()).then(|| raw.order_link_id.clone()),
        symbol,
        side: parse_side(&raw.side)?,
        order_type: parse_order_type(&raw.order_type)?,
        status: parse_status(&raw.order_status)?,
        quantity: parse_decimal(&raw.qty)?,
        filled_quantity: dec_or_zero(&raw.cum_exec_qty),
        price: nonzero_decimal(&raw.price),
        average_price: nonzero_decimal(&raw.avg_price),
    })
}

#[derive(Deserialize)]
struct CreateResult {
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "orderLinkId", default)]
    order_link_id: String,
}

#[derive(Deserialize)]
struct OrderList {
    list: Vec<RawOrder>,
}

#[derive(Deserialize)]
struct RawOrder {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "orderLinkId", default)]
    order_link_id: String,
    side: String,
    #[serde(rename = "orderType")]
    order_type: String,
    #[serde(rename = "orderStatus")]
    order_status: String,
    qty: String,
    #[serde(rename = "cumExecQty", default)]
    cum_exec_qty: String,
    #[serde(default)]
    price: String,
    #[serde(rename = "avgPrice", default)]
    avg_price: String,
}

#[derive(Deserialize)]
struct WalletBalance {
    list: Vec<WalletAccount>,
}

#[derive(Deserialize)]
struct WalletAccount {
    coin: Vec<CoinBalance>,
}

#[derive(Deserialize)]
struct CoinBalance {
    coin: String,
    #[serde(rename = "availableToWithdraw", default)]
    available_to_withdraw: String,
    #[serde(default)]
    locked: String,
}

/// Quote assets used to split a concatenated wire symbol (`BTCUSDT` -> `BTC/USDT`).
const KNOWN_QUOTES: &[&str] = &["USDT", "USDC", "EUR", "BTC", "ETH", "USD"];

/// Map a concatenated Bybit wire symbol back to a canonical [`Symbol`].
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

/// The public WebSocket base URL for a category and network.
fn ws_base_url(category: &str, testnet: bool) -> String {
    let host = if testnet {
        "wss://stream-testnet.bybit.com"
    } else {
        "wss://stream.bybit.com"
    };
    format!("{host}/v5/public/{category}")
}

/// The private (user-data) WebSocket URL for a network.
fn ws_private_url(testnet: bool) -> String {
    let host = if testnet {
        "wss://stream-testnet.bybit.com"
    } else {
        "wss://stream.bybit.com"
    };
    format!("{host}/v5/private")
}

/// The WebSocket trade-API URL for a network.
fn ws_trade_url(testnet: bool) -> String {
    let host = if testnet {
        "wss://stream-testnet.bybit.com"
    } else {
        "wss://stream.bybit.com"
    };
    format!("{host}/v5/trade")
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

/// Parse one Bybit WebSocket frame into zero or more [`Event`]s, routing by the
/// `topic` prefix. `op` responses and unhandled topics yield an empty vector.
fn parse_ws_message(text: &str, resolve: &impl Fn(&str) -> Symbol) -> Result<Vec<Event>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::Deserialization(e.to_string()))?;
    let Some(topic) = value.get("topic").and_then(serde_json::Value::as_str) else {
        return Ok(Vec::new());
    };
    let null = serde_json::Value::Null;
    let data = value.get("data").unwrap_or(&null);

    if topic.starts_with("publicTrade.") {
        let trades = data
            .as_array()
            .ok_or_else(|| Error::Deserialization("trade data not an array".to_string()))?;
        trades
            .iter()
            .map(|t| {
                Ok(Event::Trade(TradePrint {
                    symbol: resolve(field_str(t, "s")?),
                    price: parse_decimal(field_str(t, "p")?)?,
                    quantity: parse_decimal(field_str(t, "v")?)?,
                    aggressor: parse_side(field_str(t, "S")?)?,
                    timestamp: t.get("T").and_then(serde_json::Value::as_i64).unwrap_or(0),
                }))
            })
            .collect()
    } else if topic.starts_with("tickers.") {
        Ok(vec![Event::Ticker(Ticker {
            symbol: resolve(field_str(data, "symbol")?),
            last: parse_decimal(field_str(data, "lastPrice")?)?,
            bid: dec_or_zero(opt_str(data, "bid1Price")),
            ask: dec_or_zero(opt_str(data, "ask1Price")),
            volume: dec_or_zero(opt_str(data, "volume24h")),
            timestamp: value
                .get("ts")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        })])
    } else if topic.starts_with("orderbook.") {
        let symbol = resolve(field_str(data, "s")?);
        let update_id = data
            .get("u")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let bids = parse_ws_levels(data.get("b"))?;
        let asks = parse_ws_levels(data.get("a"))?;
        if value.get("type").and_then(serde_json::Value::as_str) == Some("snapshot") {
            Ok(vec![Event::BookSnapshot(OrderBookSnapshot {
                symbol,
                last_update_id: update_id,
                bids,
                asks,
                timestamp: value
                    .get("ts")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })])
        } else {
            Ok(vec![Event::BookDelta(BookDelta {
                symbol,
                first_update_id: update_id,
                final_update_id: update_id,
                bids,
                asks,
                timestamp: value
                    .get("ts")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })])
        }
    } else if topic == "order" {
        // Private order stream: `data` is an array of order objects sharing the
        // REST order shape, so each deserializes into `RawOrder`.
        let orders = data
            .as_array()
            .ok_or_else(|| Error::Deserialization("order data not an array".to_string()))?;
        orders
            .iter()
            .map(|raw| {
                let order: RawOrder = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                let symbol = split_wire_symbol(&order.symbol);
                Ok(Event::OrderUpdate(order_from_raw(symbol, &order)?))
            })
            .collect()
    } else if topic == "wallet" {
        // Private wallet stream: `data` is an array of accounts, each with a
        // `coin` array; every account emits a balance-update snapshot.
        let accounts = data
            .as_array()
            .ok_or_else(|| Error::Deserialization("wallet data not an array".to_string()))?;
        accounts
            .iter()
            .map(|raw| {
                let account: WalletAccount = serde_json::from_value(raw.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()))?;
                Ok(Event::BalanceUpdate(
                    account
                        .coin
                        .iter()
                        .map(|c| Balance {
                            asset: c.coin.clone(),
                            free: dec_or_zero(&c.available_to_withdraw),
                            locked: dec_or_zero(&c.locked),
                        })
                        .collect(),
                ))
            })
            .collect()
    } else {
        Ok(Vec::new())
    }
}

impl MarketData for Bybit {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        Bybit::ticker(self, symbol)
    }
    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        Bybit::klines(self, symbol, interval, limit)
    }
    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        Bybit::order_book(self, symbol, depth)
    }
    fn subscribe_trades(&mut self, symbol: &Symbol) -> Result<()> {
        Bybit::subscribe_trades(self, symbol)
    }
    fn subscribe_book(&mut self, symbol: &Symbol) -> Result<()> {
        Bybit::subscribe_book(self, symbol)
    }
    fn subscribe_ticker(&mut self, symbol: &Symbol) -> Result<()> {
        Bybit::subscribe_ticker(self, symbol)
    }
    fn poll_events(&mut self) -> Vec<Event> {
        Bybit::poll_events(self)
    }
}

impl Execution for Bybit {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        Bybit::place_order(self, request)
    }
    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Bybit::cancel_order(self, symbol, order_id)
    }
    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        Bybit::query_order(self, symbol, order_id)
    }
    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Bybit::open_orders(self, symbol)
    }
    fn balances(&mut self) -> Result<Vec<Balance>> {
        Bybit::balances(self)
    }
}

impl Bybit {
    /// Open positions (`/v5/position/list`). Without a symbol filter, linear
    /// positions are queried by settle coin (USDT).
    ///
    /// # Errors
    /// Returns an [`Error`] if credentials are missing or the request fails.
    pub fn positions(&self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        let query = match symbol {
            Some(s) => format!("category={}&symbol={}", self.category, Self::wire_symbol(s)),
            None => format!("category={}&settleCoin=USDT", self.category),
        };
        let result = self.signed_request(HttpMethod::Get, "/v5/position/list", &query, "")?;
        let list: PositionList = parse_result(result)?;
        list.list.iter().filter_map(parse_bybit_position).collect()
    }

    /// Set the leverage for `symbol` (`/v5/position/set-leverage`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the leverage is rejected or the request fails.
    pub fn set_leverage(&self, symbol: &Symbol, leverage: u32) -> Result<()> {
        let lev = leverage.to_string();
        let body = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "buyLeverage": lev,
            "sellLeverage": lev,
        });
        self.signed_request(
            HttpMethod::Post,
            "/v5/position/set-leverage",
            "",
            &body.to_string(),
        )?;
        Ok(())
    }

    /// Set the margin mode for `symbol` (`/v5/position/switch-isolated`). Bybit
    /// couples the mode with the leverage, so the current leverage is preserved.
    ///
    /// # Errors
    /// Returns an [`Error`] if the change is rejected or the request fails.
    pub fn set_margin_mode(&self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        let leverage = self
            .positions(Some(symbol))?
            .first()
            .map_or_else(|| "10".to_string(), |p| p.leverage.normalize().to_string());
        let trade_mode = match mode {
            MarginMode::Cross => 0,
            MarginMode::Isolated => 1,
        };
        let body = serde_json::json!({
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "tradeMode": trade_mode,
            "buyLeverage": leverage,
            "sellLeverage": leverage,
        });
        self.signed_request(
            HttpMethod::Post,
            "/v5/position/switch-isolated",
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
    /// (`/v5/order/amend`), then return the refreshed order.
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
            "category": self.category,
            "symbol": Self::wire_symbol(symbol),
            "orderId": order_id,
        });
        if let Some(q) = new_quantity {
            body["qty"] = serde_json::json!(format_decimal(q));
        }
        if let Some(p) = new_price {
            body["price"] = serde_json::json!(format_decimal(p));
        }
        self.signed_request(HttpMethod::Post, "/v5/order/amend", "", &body.to_string())?;
        self.query_order(symbol, order_id)
    }

    /// Place several orders in one request (`/v5/order/create-batch`). Each
    /// element's outcome is preserved: an empty returned `orderId` marks a
    /// rejected leg.
    ///
    /// # Errors
    /// Returns an [`Error`] if the batch request itself fails.
    pub fn place_batch(&self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        if requests.iter().any(|r| r.order_type.is_trigger()) {
            return Err(Error::unsupported_trigger("Bybit"));
        }
        for request in requests {
            self.ensure_reduce_only_is_reducible(request)?;
        }
        // Resolved before the batch is built, so a request the venue cannot
        // express refuses the whole call rather than being sent weakened.
        let tifs = requests.iter().map(tif_for).collect::<Result<Vec<_>>>()?;
        let items: Vec<serde_json::Value> = requests
            .iter()
            .zip(&tifs)
            .map(|(r, tif)| {
                let mut o = serde_json::json!({
                    "symbol": Self::wire_symbol(&r.symbol),
                    "side": side_str(r.side),
                    "orderType": order_type_str(r.order_type),
                    "qty": format_decimal(r.quantity),
                    "timeInForce": tif,
                });
                if let Some(smp) = smp_str(r.stp) {
                    o["smpType"] = serde_json::json!(smp);
                }
                if let Some(price) = r.price {
                    o["price"] = serde_json::json!(format_decimal(price));
                }
                if let Some(id) = &r.client_order_id {
                    o["orderLinkId"] = serde_json::json!(id.clone());
                }
                if r.reduce_only {
                    o["reduceOnly"] = serde_json::json!(true);
                }
                if let Some(idx) = self.position_idx(r) {
                    o["positionIdx"] = serde_json::json!(idx);
                }
                o
            })
            .collect();
        let body = serde_json::json!({ "category": self.category, "request": items });
        let result = self.signed_request(
            HttpMethod::Post,
            "/v5/order/create-batch",
            "",
            &body.to_string(),
        )?;
        let created: BatchCreateResult = parse_result(result)?;
        Ok(requests
            .iter()
            .zip(created.list)
            .map(|(req, out)| {
                if out.order_id.is_empty() {
                    return Err(Error::OrderRejected {
                        code: "batch".to_string(),
                        message: "order rejected in batch".to_string(),
                    });
                }
                Ok(Order {
                    id: out.order_id,
                    client_order_id: (!out.order_link_id.is_empty()).then_some(out.order_link_id),
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

    /// Cancel several orders on one `symbol` in a single request
    /// (`/v5/order/cancel-batch`).
    ///
    /// # Errors
    /// Returns an [`Error`] if the request fails.
    pub fn cancel_batch(&self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        let wire = Self::wire_symbol(symbol);
        let items: Vec<serde_json::Value> = order_ids
            .iter()
            .map(|id| serde_json::json!({ "symbol": wire, "orderId": id }))
            .collect();
        let body = serde_json::json!({ "category": self.category, "request": items });
        self.signed_request(
            HttpMethod::Post,
            "/v5/order/cancel-batch",
            "",
            &body.to_string(),
        )?;
        Ok(())
    }
}

impl AdvancedOrders for Bybit {
    fn amend_order(
        &mut self,
        symbol: &Symbol,
        order_id: &str,
        new_price: Option<Decimal>,
        new_quantity: Option<Decimal>,
    ) -> Result<Order> {
        Bybit::amend_order(self, symbol, order_id, new_price, new_quantity)
    }
    fn place_batch(&mut self, requests: &[OrderRequest]) -> Result<Vec<Result<Order>>> {
        Bybit::place_batch(self, requests)
    }
    fn cancel_batch(&mut self, symbol: &Symbol, order_ids: &[String]) -> Result<()> {
        Bybit::cancel_batch(self, symbol, order_ids)
    }
    /// Bybit has no standalone one-cancels-other order-list (take-profit/stop are
    /// attached to a position/order via `takeProfit`/`stopLoss`), so this returns
    /// an [`Error::Exchange`].
    fn place_oco(&mut self, _request: &OcoRequest) -> Result<Vec<Order>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "Bybit has no OCO order-list; attach takeProfit/stopLoss to the order"
                .to_string(),
        })
    }
}

impl Exchange for Bybit {
    fn name(&self) -> &'static str {
        "bybit"
    }
}

impl WsUserData for Bybit {
    fn subscribe_user_data(&mut self) -> Result<()> {
        Bybit::subscribe_user_data(self)
    }
    fn keepalive_user_data(&mut self) -> Result<()> {
        Bybit::keepalive_user_data(self)
    }
}

impl DerivativesStream for Bybit {
    fn subscribe_derivatives(
        &mut self,
        symbol: &Symbol,
        channel: DerivativesChannel,
    ) -> Result<()> {
        Bybit::subscribe_derivatives(self, symbol, channel)
    }
    fn open_interest(&mut self, symbol: &Symbol) -> Result<OpenInterest> {
        Bybit::open_interest(self, symbol)
    }
    fn long_short_ratio(&mut self, symbol: &Symbol) -> Result<LongShortRatio> {
        Bybit::long_short_ratio(self, symbol)
    }
}

impl WsExecution for Bybit {
    fn place_order_ws(&mut self, request: &OrderRequest) -> Result<Order> {
        Bybit::place_order_ws(self, request)
    }
    fn cancel_order_ws(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        Bybit::cancel_order_ws(self, symbol, order_id)
    }
}

#[derive(Deserialize)]
struct BatchCreateResult {
    list: Vec<CreateResult>,
}

impl Derivatives for Bybit {
    fn positions(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Position>> {
        Bybit::positions(self, symbol)
    }
    fn set_leverage(&mut self, symbol: &Symbol, leverage: u32) -> Result<()> {
        Bybit::set_leverage(self, symbol, leverage)
    }
    fn set_margin_mode(&mut self, symbol: &Symbol, mode: MarginMode) -> Result<()> {
        Bybit::set_margin_mode(self, symbol, mode)
    }
    fn close_position(&mut self, symbol: &Symbol) -> Result<Order> {
        Bybit::close_position(self, symbol)
    }
}

#[derive(Deserialize)]
struct PositionList {
    list: Vec<RawBybitPosition>,
}

#[derive(Deserialize)]
struct RawBybitPosition {
    symbol: String,
    side: String,
    size: String,
    #[serde(rename = "avgPrice")]
    avg_price: String,
    #[serde(rename = "markPrice")]
    mark_price: String,
    leverage: String,
    #[serde(rename = "unrealisedPnl")]
    unrealised_pnl: String,
    #[serde(rename = "tradeMode", default)]
    trade_mode: i64,
}

fn parse_bybit_position(raw: &RawBybitPosition) -> Option<Result<Position>> {
    let size = match parse_decimal(&raw.size) {
        Ok(size) if !size.is_zero() => size,
        Ok(_) => return None, // flat position
        Err(e) => return Some(Err(e)),
    };
    let side = match raw.side.as_str() {
        "Sell" => PositionSide::Short,
        _ => PositionSide::Long,
    };
    let build = || -> Result<Position> {
        Ok(Position {
            symbol: split_wire_symbol(&raw.symbol),
            side,
            quantity: size,
            entry_price: parse_decimal(&raw.avg_price)?,
            mark_price: parse_decimal(&raw.mark_price)?,
            leverage: parse_decimal(&raw.leverage)?,
            unrealized_pnl: parse_decimal(&raw.unrealised_pnl)?,
            margin_mode: if raw.trade_mode == 1 {
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

    fn client(market_type: MarketType, testnet: bool) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = if testnet {
            ExchangeOptions::testnet(market_type)
        } else {
            ExchangeOptions::mainnet(market_type)
        };
        let bybit = Bybit::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &opts);
        (bybit, mock)
    }

    #[test]
    fn category_by_market_type() {
        assert_eq!(category(MarketType::Spot), "spot");
        assert_eq!(category(MarketType::UsdMFutures), "linear");
        assert_eq!(category(MarketType::CoinMFutures), "inverse");
        assert_eq!(category(MarketType::Margin), "spot");
    }

    #[test]
    fn interval_mapping() {
        assert_eq!(map_interval("1m"), "1");
        assert_eq!(map_interval("1h"), "60");
        assert_eq!(map_interval("4h"), "240");
        assert_eq!(map_interval("1d"), "D");
        assert_eq!(map_interval("weird"), "weird");
    }

    #[test]
    fn ticker_unwraps_envelope_and_targets_url() {
        let (bybit, mock) = client(MarketType::Spot, false);
        assert_eq!(bybit.category(), "spot");
        mock.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"symbol":"BTCUSDT",
            "lastPrice":"20000.5","bid1Price":"20000.0","ask1Price":"20001.0","volume24h":"1234.5"}]}}"#,
        );
        let ticker = bybit.ticker(&symbol()).unwrap();
        assert_eq!(ticker.last, dec!(20000.5));
        assert_eq!(ticker.bid, dec!(20000.0));
        assert_eq!(ticker.ask, dec!(20001.0));
        let req = &mock.recorded_requests()[0];
        assert_eq!(
            req.url,
            "https://api.bybit.com/v5/market/tickers?category=spot&symbol=BTCUSDT"
        );
    }

    #[test]
    fn klines_are_reversed_to_chronological() {
        let (bybit, mock) = client(MarketType::Spot, false);
        // Bybit returns newest-first.
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[
            ["1700000060000","105","106","104","105.5","2","0"],
            ["1700000000000","100","110","95","105","12.5","0"]]}}"#,
        );
        let candles = bybit.klines(&symbol(), "1m", 2).unwrap();
        assert_eq!(candles.len(), 2);
        // Oldest first after reversing.
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert_eq!(candles[1].timestamp, 1_700_000_060_000);
    }

    #[test]
    fn order_book_parses_levels() {
        let (bybit, mock) = client(MarketType::UsdMFutures, true);
        assert_eq!(bybit.category(), "linear");
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"s":"BTCUSDT","u":77,
            "b":[["100.0","1.5"]],"a":[["101.0","2.0"]]}}"#,
        );
        let book = bybit.order_book(&symbol(), 5).unwrap();
        assert_eq!(book.last_update_id, 77);
        assert_eq!(book.bids[0], BookLevel::new(dec!(100.0), dec!(1.5)));
        assert_eq!(book.asks[0], BookLevel::new(dec!(101.0), dec!(2.0)));
        assert!(mock.recorded_requests()[0]
            .url
            .starts_with("https://api-testnet.bybit.com/v5/market/orderbook"));
    }

    #[test]
    fn error_envelope_maps_to_taxonomy() {
        let cases = [
            (10004, "sign"),
            (10006, "rate"),
            (170131, "balance"),
            (110001, "notfound"),
            (99999, "exchange"),
        ];
        for (code, kind) in cases {
            let (bybit, mock) = client(MarketType::Spot, false);
            mock.push_json(
                200,
                format!(r#"{{"retCode":{code},"retMsg":"x","result":{{}}}}"#),
            );
            let err = bybit.ticker(&symbol()).unwrap_err();
            match kind {
                "sign" => assert!(matches!(err, Error::Auth(_))),
                "rate" => assert!(matches!(err, Error::RateLimited { .. })),
                "balance" => assert!(matches!(err, Error::InsufficientBalance)),
                "notfound" => assert!(matches!(err, Error::NotFound(_))),
                _ => assert!(matches!(err, Error::Exchange { .. })),
            }
        }
    }

    #[test]
    fn empty_ticker_list_is_not_found() {
        let (bybit, mock) = client(MarketType::Spot, false);
        mock.push_json(200, r#"{"retCode":0,"result":{"list":[]}}"#);
        assert!(matches!(
            bybit.ticker(&symbol()).unwrap_err(),
            Error::NotFound(_)
        ));
    }

    fn signed_client(now_ms: i64) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let bybit = Bybit::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (bybit, mock)
    }

    fn signed_futures_client(now_ms: i64) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
        let bybit = Bybit::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (bybit, mock)
    }

    /// Bybit will not infer which way the market must cross the trigger.
    ///
    /// `triggerDirection` is 1 for "rises to" and 2 for "falls to". A sell stop
    /// protects a long and fires on the way *down*; a buy stop covers a short
    /// and fires on the way *up*. The wrong direction arms the order on the
    /// side that never comes, so the stop simply never fires -- and nothing
    /// reports an error, because the order was accepted.
    #[test]
    fn a_trigger_carries_the_direction_the_market_must_cross() {
        for (side, expected) in [(OrderSide::Sell, 2), (OrderSide::Buy, 1)] {
            let (bybit, mock) = signed_futures_client(1000);
            mock.push_json(
                200,
                r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
            );
            let request = OrderRequest {
                order_type: OrderType::StopMarket,
                stop_price: Some(dec!(19000)),
                side,
                ..OrderRequest::market_sell(symbol(), dec!(1))
            };
            Bybit::place_order(&bybit, &request).unwrap();

            let body = mock.recorded_requests()[0].body.clone().unwrap();
            assert!(body.contains(r#""triggerPrice":"19000""#), "{body}");
            assert!(
                body.contains(&format!(r#""triggerDirection":{expected}"#)),
                "a {side:?} stop must fire on the other side: {body}"
            );
        }
    }

    /// Bybit's spot endpoint serves plain and conditional orders through one
    /// call and defaults to the plain one, so a trigger without `orderFilter`
    /// is accepted and placed **immediately** -- the stop that executes at once.
    #[test]
    fn a_spot_trigger_names_the_order_filter() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        let request = OrderRequest {
            order_type: OrderType::StopMarket,
            stop_price: Some(dec!(19000)),
            ..OrderRequest::market_sell(symbol(), dec!(1))
        };
        Bybit::place_order(&bybit, &request).unwrap();

        let body = mock.recorded_requests()[0].body.clone().unwrap();
        assert!(body.contains(r#""orderFilter":"StopOrder""#), "{body}");
    }

    /// A futures trigger needs no order filter: the category already says which
    /// endpoint behaviour applies.
    #[test]
    fn a_futures_trigger_needs_no_order_filter() {
        let (bybit, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        let request = OrderRequest {
            order_type: OrderType::StopLimit,
            stop_price: Some(dec!(19000)),
            ..OrderRequest::limit_sell(symbol(), dec!(1), dec!(18900))
        };
        Bybit::place_order(&bybit, &request).unwrap();

        let body = mock.recorded_requests()[0].body.clone().unwrap();
        assert!(!body.contains("orderFilter"), "{body}");
        // The limit price keeps its own field; the trigger has another.
        assert!(body.contains(r#""price":"18900""#), "{body}");
        assert!(body.contains(r#""triggerPrice":"19000""#), "{body}");
    }

    #[test]
    fn a_bybit_trigger_without_a_stop_price_is_refused() {
        let (bybit, mock) = signed_client(1000);
        let request = OrderRequest {
            order_type: OrderType::StopMarket,
            stop_price: None,
            ..OrderRequest::market_sell(symbol(), dec!(1))
        };
        assert!(Bybit::place_order(&bybit, &request).is_err());
        assert!(mock.recorded_requests().is_empty());
    }

    #[test]
    fn stp_maps_to_smp_type() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        bybit
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .with_stp(SelfTradePrevention::ExpireBoth),
            )
            .unwrap();
        let reqs = mock.recorded_requests();
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""smpType":"CancelBoth""#));
    }

    #[test]
    fn amend_order_amends_then_reads_back() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"symbol":"BTCUSDT","orderId":"a","side":"Buy",
            "orderType":"Limit","orderStatus":"New","qty":"2","price":"101"}]}}"#,
        );
        let order = bybit
            .amend_order(&symbol(), "a", Some(dec!(101)), Some(dec!(2)))
            .unwrap();
        assert_eq!(order.quantity, dec!(2));
        assert_eq!(order.price, Some(dec!(101)));
        let reqs = mock.recorded_requests();
        assert!(reqs[0].url.contains("/v5/order/amend"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""qty":"2""#));
        assert!(body.contains(r#""price":"101""#));
        assert!(reqs[1].url.contains("/v5/order/realtime"));
    }

    #[test]
    fn place_batch_returns_per_order_results() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[
            {"orderId":"o1","orderLinkId":""},
            {"orderId":"","orderLinkId":""}]}}"#,
        );
        let results = bybit
            .place_batch(&[
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)),
                OrderRequest::limit_buy(symbol(), dec!(1), dec!(101)),
            ])
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().id, "o1");
        assert!(matches!(
            results[1].as_ref().unwrap_err(),
            Error::OrderRejected { .. }
        ));
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/v5/order/create-batch"));
    }

    #[test]
    fn cancel_batch_is_one_call() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(200, r#"{"retCode":0,"result":{"list":[]}}"#);
        bybit
            .cancel_batch(&symbol(), &["1".to_string(), "2".to_string()])
            .unwrap();
        let reqs = mock.recorded_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/v5/order/cancel-batch"));
        let body = reqs[0].body.as_ref().unwrap();
        assert!(body.contains(r#""orderId":"1""#));
        assert!(body.contains(r#""orderId":"2""#));
    }

    /// Bybit's `tickers` frames are deltas: after the first snapshot only the
    /// changed fields are present. A funding print is emitted only when the
    /// frame actually carries a rate. Carrying the last seen value forward would
    /// report a stale figure as current, and defaulting to zero would report a
    /// funding rate of zero -- a number a strategy acts on.
    #[test]
    fn a_delta_ticker_frame_prints_only_what_it_carries() {
        let resolve = |_: &str| symbol();
        let both = [
            ("BTCUSDT".to_string(), DerivativesChannel::Funding),
            ("BTCUSDT".to_string(), DerivativesChannel::MarkIndex),
        ];

        let full: serde_json::Value = serde_json::from_str(
            r#"{"symbol":"BTCUSDT","markPrice":"20000.5","indexPrice":"19998.25","fundingRate":"0.0001","ts":1700000000000}"#,
        )
        .unwrap();
        let events = parse_derivatives_message("tickers.BTCUSDT", &full, &resolve, &both);
        assert_eq!(events.len(), 2);

        // A delta with only the mark price: no funding print, because there is
        // no rate in it.
        let delta: serde_json::Value =
            serde_json::from_str(r#"{"symbol":"BTCUSDT","markPrice":"20001.0"}"#).unwrap();
        let events = parse_derivatives_message("tickers.BTCUSDT", &delta, &resolve, &both);
        assert!(events.is_empty());

        // A delta with a rate and a mark, but no index: funding only.
        let delta: serde_json::Value = serde_json::from_str(
            r#"{"symbol":"BTCUSDT","markPrice":"20001.0","fundingRate":"0.0002"}"#,
        )
        .unwrap();
        let events = parse_derivatives_message("tickers.BTCUSDT", &delta, &resolve, &both);
        assert_eq!(events.len(), 1);
        let Event::Derivatives(DerivativesFeed::Funding(funding)) = &events[0] else {
            panic!("expected a funding print");
        };
        assert_eq!(funding.rate, dec!(0.0002));
    }

    /// The same frame prints nothing for a client that subscribed to neither
    /// channel: the `tickers` topic also feeds the ordinary ticker, so a client
    /// watching prices must not start receiving funding prints it never asked
    /// for.
    #[test]
    fn a_ticker_subscription_alone_yields_no_derivatives_prints() {
        let resolve = |_: &str| symbol();
        let full: serde_json::Value = serde_json::from_str(
            r#"{"symbol":"BTCUSDT","markPrice":"20000.5","indexPrice":"19998.25","fundingRate":"0.0001"}"#,
        )
        .unwrap();
        assert!(parse_derivatives_message("tickers.BTCUSDT", &full, &resolve, &[]).is_empty());
    }

    /// `allLiquidation` reports the taker side of the forced order -- the side
    /// hitting the book -- which is what `Liquidation` carries. The older
    /// `liquidation` topic reported the position side, the opposite one.
    #[test]
    fn all_liquidation_carries_the_side_hitting_the_book() {
        let resolve = |_: &str| symbol();
        let data: serde_json::Value = serde_json::from_str(
            r#"[{"T":1700000000123,"s":"BTCUSDT","S":"Sell","v":"2.5","p":"19000"}]"#,
        )
        .unwrap();
        let events = parse_derivatives_message(
            "allLiquidation.BTCUSDT",
            &data,
            &resolve,
            &[("BTCUSDT".to_string(), DerivativesChannel::Liquidations)],
        );
        assert_eq!(events.len(), 1);
        let Event::Derivatives(DerivativesFeed::Liquidation(liq)) = &events[0] else {
            panic!("expected a liquidation print");
        };
        assert_eq!(liq.side, OrderSide::Sell);
        assert_eq!(liq.price, dec!(19000));
        assert_eq!(liq.quantity, dec!(2.5));
        assert_eq!(liq.timestamp, 1_700_000_000_123);

        assert!(
            parse_derivatives_message("allLiquidation.BTCUSDT", &data, &resolve, &[]).is_empty()
        );
    }

    /// A spot client refuses the derivatives channels rather than subscribing to
    /// a topic that will never carry a frame.
    #[test]
    fn a_spot_client_refuses_the_derivatives_channels() {
        let (mut spot, _mock) = signed_client(1000);
        let err = DerivativesStream::subscribe_derivatives(
            &mut spot,
            &symbol(),
            DerivativesChannel::Funding,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Exchange { ref code, .. } if code == "unsupported"));
        let err = DerivativesStream::open_interest(&mut spot, &symbol()).unwrap_err();
        assert!(matches!(err, Error::Exchange { ref code, .. } if code == "unsupported"));
        let err = DerivativesStream::long_short_ratio(&mut spot, &symbol()).unwrap_err();
        assert!(matches!(err, Error::Exchange { ref code, .. } if code == "unsupported"));
    }

    /// The polled figures are read over REST, and an empty series is reported
    /// rather than read as a zero.
    #[test]
    fn the_polled_figures_are_read_over_rest() {
        let (futures, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"openInterest":"12345.67","timestamp":"1700000000000"}]}}"#,
        );
        let oi = Bybit::open_interest(&futures, &symbol()).unwrap();
        assert_eq!(oi.open_interest, dec!(12345.67));
        assert_eq!(oi.timestamp, 1_700_000_000_000);

        let (futures, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"buyRatio":"0.6","sellRatio":"0.4","timestamp":"1700000000000"}]}}"#,
        );
        let ratio = Bybit::long_short_ratio(&futures, &symbol()).unwrap();
        assert_eq!(ratio.long_size, dec!(0.6));
        assert_eq!(ratio.short_size, dec!(0.4));

        let (futures, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"retCode":0,"retMsg":"OK","result":{"list":[]}}"#);
        assert!(Bybit::open_interest(&futures, &symbol()).is_err());
    }

    #[test]
    fn place_oco_is_unsupported() {
        let (mut bybit, _mock) = signed_client(1000);
        assert!(matches!(
            AdvancedOrders::place_oco(
                &mut bybit,
                &OcoRequest::new(symbol(), OrderSide::Sell, dec!(1), dec!(110), dec!(95))
            )
            .unwrap_err(),
            Error::Exchange { .. }
        ));
    }

    const POSITION_ENVELOPE: &str = r#"{"retCode":0,"retMsg":"OK","result":{"list":[
        {"symbol":"BTCUSDT","side":"Buy","size":"0.5","avgPrice":"20000","markPrice":"20100","leverage":"10","unrealisedPnl":"50","tradeMode":1}
    ],"category":"linear"}}"#;

    #[test]
    fn derivatives_positions_parse() {
        let (mut bybit, mock) = signed_futures_client(1000);
        mock.push_json(200, POSITION_ENVELOPE);
        let positions = Derivatives::positions(&mut bybit, Some(&symbol())).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, Symbol::new("BTC", "USDT"));
        assert_eq!(positions[0].side, PositionSide::Long);
        assert_eq!(positions[0].quantity, dec!(0.5));
        assert_eq!(positions[0].leverage, dec!(10));
        assert_eq!(positions[0].margin_mode, MarginMode::Isolated);
        assert!(mock.recorded_requests()[0]
            .url
            .contains("/v5/position/list"));
        assert!(mock.recorded_requests()[0].url.contains("category=linear"));
    }

    #[test]
    fn derivatives_set_leverage_hits_endpoint() {
        let (mut bybit, mock) = signed_futures_client(1000);
        mock.push_json(200, r#"{"retCode":0,"retMsg":"OK","result":{}}"#);
        Derivatives::set_leverage(&mut bybit, &symbol(), 20).unwrap();
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("/v5/position/set-leverage"));
        assert!(req
            .body
            .as_deref()
            .unwrap()
            .contains(r#""buyLeverage":"20""#));
    }

    #[test]
    fn derivatives_set_margin_mode_switches_isolated() {
        let (mut bybit, mock) = signed_futures_client(1000);
        mock.push_json(200, POSITION_ENVELOPE); // leverage lookup
        mock.push_json(200, r#"{"retCode":0,"retMsg":"OK","result":{}}"#);
        Derivatives::set_margin_mode(&mut bybit, &symbol(), MarginMode::Isolated).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/v5/position/switch-isolated"));
        assert!(reqs[1]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""tradeMode":1"#));
    }

    #[test]
    fn derivatives_close_position_reduce_only() {
        let (mut bybit, mock) = signed_futures_client(1000);
        mock.push_json(200, POSITION_ENVELOPE);
        mock.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"9","orderLinkId":""}}"#,
        );
        Derivatives::close_position(&mut bybit, &symbol()).unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[1].url.contains("/v5/order/create"));
        let body = reqs[1].body.as_deref().unwrap();
        assert!(body.contains(r#""side":"Sell""#));
        assert!(body.contains(r#""reduceOnly":true"#));
    }

    fn header<'a>(req: &'a HttpRequest, name: &str) -> &'a str {
        req.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    fn hedged_futures_client(now_ms: i64) -> (Bybit, Arc<MockHttpTransport>) {
        let mock = Arc::new(MockHttpTransport::new());
        let mut opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
        opts.position_mode = PositionMode::Hedge;
        let bybit = Bybit::with_credentials(
            Box::new(ArcTransport(Arc::clone(&mock))),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_clock(Box::new(move || now_ms));
        (bybit, mock)
    }

    #[test]
    fn post_only_and_reduce_only_reach_every_order_path() {
        // Three paths build an order body; each spells post-only as the
        // `PostOnly` time-in-force and reduce-only as its own flag. A futures
        // client, because `reduce_only` names a position: on spot there is none
        // to reduce and every path refuses it.
        let (bybit, mock) = signed_futures_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        bybit
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .post_only()
                    .reduce_only(),
            )
            .unwrap();
        let body = mock.recorded_requests()[0].body.clone().unwrap();
        assert!(body.contains(r#""timeInForce":"PostOnly""#));
        assert!(body.contains(r#""reduceOnly":true"#));

        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"orderId":"o1","orderLinkId":""}]}}"#,
        );
        bybit
            .place_batch(&[OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).reduce_only()])
            .unwrap();
        assert!(mock.recorded_requests()[1]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""reduceOnly":true"#));
    }

    #[test]
    fn hedge_mode_sends_the_position_index() {
        // positionIdx 1 is the buy side of a hedged account, 2 the sell side.
        // 0 is one-way and is the venue default, so it is left off entirely.
        let (hedged, mock) = hedged_futures_client(1000);
        for _ in 0..2 {
            mock.push_json(
                200,
                r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
            );
        }
        hedged
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        hedged
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).reduce_only())
            .unwrap();
        let reqs = mock.recorded_requests();
        assert!(reqs[0]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""positionIdx":1"#));
        // A buy that reduces is closing the short side.
        assert!(reqs[1]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""positionIdx":2"#));

        // Spot never carries one, whatever the mode says.
        let (spot, spot_mock) = signed_client(1000);
        spot_mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"a","orderLinkId":""}}"#,
        );
        spot.place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert!(!spot_mock.recorded_requests()[0]
            .body
            .as_deref()
            .unwrap()
            .contains("positionIdx"));
    }

    #[test]
    fn place_order_signs_with_bapi_headers() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"1739","orderLinkId":"abc"}}"#,
        );
        let order = bybit
            .place_order(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)).with_client_order_id("abc"),
            )
            .unwrap();
        assert_eq!(order.id, "1739");
        assert_eq!(order.client_order_id.as_deref(), Some("abc"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.symbol, symbol());

        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        let body = req.body.as_ref().unwrap();
        let ts = header(req, "X-BAPI-TIMESTAMP");
        let recv = header(req, "X-BAPI-RECV-WINDOW");
        assert_eq!(ts, "1000");
        let expected = hmac_sha256_hex(b"SECRET", format!("{ts}APIKEY{recv}{body}").as_bytes());
        assert_eq!(header(req, "X-BAPI-SIGN"), expected);
        assert_eq!(header(req, "X-BAPI-API-KEY"), "APIKEY");
        assert!(body.contains(r#""side":"Buy""#));
        assert!(body.contains(r#""orderLinkId":"abc""#));
    }

    #[test]
    fn cancel_order_posts_signed() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(200, r#"{"retCode":0,"result":{"orderId":"1739"}}"#);
        bybit.cancel_order(&symbol(), "1739").unwrap();
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Post);
        assert!(req.url.ends_with("/v5/order/cancel"));
        assert!(req.body.as_ref().unwrap().contains(r#""orderId":"1739""#));
    }

    #[test]
    fn query_order_parses_realtime_list() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"orderId":"1739","orderLinkId":"",
            "symbol":"BTCUSDT","side":"Sell","orderType":"Market","orderStatus":"Filled",
            "qty":"2","cumExecQty":"2","price":"0","avgPrice":"100"}]}}"#,
        );
        let order = bybit.query_order(&symbol(), "1739").unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.filled_quantity, dec!(2));
        assert_eq!(order.average_price, Some(dec!(100)));
        assert_eq!(order.price, None);
        assert_eq!(order.client_order_id, None);
        let req = &mock.recorded_requests()[0];
        assert_eq!(req.method, HttpMethod::Get);
        assert!(req.url.contains("orderId=1739"));
        assert!(req.headers.iter().any(|(k, _)| k == "X-BAPI-SIGN"));
    }

    #[test]
    fn query_missing_order_is_not_found() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(200, r#"{"retCode":0,"result":{"list":[]}}"#);
        assert!(matches!(
            bybit.query_order(&symbol(), "x").unwrap_err(),
            Error::NotFound(_)
        ));
    }

    #[test]
    fn signed_without_credentials_errors() {
        let (bybit, _) = client(MarketType::Spot, false);
        assert!(matches!(
            bybit
                .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }

    #[test]
    fn split_wire_symbol_uses_known_quotes() {
        assert_eq!(split_wire_symbol("BTCUSDT"), Symbol::new("BTC", "USDT"));
        assert_eq!(split_wire_symbol("ETHBTC"), Symbol::new("ETH", "BTC"));
        assert_eq!(split_wire_symbol("WEIRD"), Symbol::new("WEIRD", ""));
    }

    #[test]
    fn open_orders_filtered_and_unfiltered() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"symbol":"BTCUSDT","orderId":"1","orderLinkId":"a",
            "side":"Buy","orderType":"Limit","orderStatus":"New","qty":"1","cumExecQty":"0",
            "price":"100","avgPrice":"0"}]}}"#,
        );
        let orders = bybit.open_orders(Some(&symbol())).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].symbol, symbol());

        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"symbol":"ETHUSDT","orderId":"2","orderLinkId":"",
            "side":"Sell","orderType":"Market","orderStatus":"New","qty":"2","cumExecQty":"0",
            "price":"0","avgPrice":"0"}]}}"#,
        );
        let orders = bybit.open_orders(None).unwrap();
        assert_eq!(orders[0].symbol, Symbol::new("ETH", "USDT"));
        assert!(!mock.recorded_requests()[1].url.contains("symbol="));
    }

    #[test]
    fn balances_parse_unified_wallet() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"list":[{"coin":[
            {"coin":"USDT","availableToWithdraw":"100.5","locked":"25.5"},
            {"coin":"BTC","availableToWithdraw":"0.1","locked":"0"}]}]}}"#,
        );
        let bals = bybit.balances().unwrap();
        assert_eq!(bals.len(), 2);
        assert_eq!(bals[0].asset, "USDT");
        assert_eq!(bals[0].free, dec!(100.5));
        assert_eq!(bals[0].locked, dec!(25.5));
        assert_eq!(bals[0].total(), dec!(126));
        let req = &mock.recorded_requests()[0];
        assert!(req.url.contains("accountType=UNIFIED"));
        assert!(req.headers.iter().any(|(k, _)| k == "X-BAPI-SIGN"));
    }

    fn streaming_client(ws: &Arc<MockWsTransport>) -> Bybit {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        Bybit::with_http(Box::new(ArcTransport(http)), &opts)
            .with_ws(Box::new(ArcWs(Arc::clone(ws))))
    }

    fn signed_ws_client(now_ms: i64) -> (Bybit, Arc<MockWsTransport>) {
        let http = Arc::new(MockHttpTransport::new());
        let ws = Arc::new(MockWsTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let bybit = Bybit::with_credentials(
            Box::new(ArcTransport(http)),
            &opts,
            Credentials::new("APIKEY", "SECRET"),
        )
        .with_ws(Box::new(ArcWs(Arc::clone(&ws))))
        .with_clock(Box::new(move || now_ms));
        (bybit, ws)
    }

    #[test]
    fn subscribe_user_data_authenticates_and_streams_orders_and_wallet() {
        let (mut bybit, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(r#"{"success":true,"op":"auth"}"#.to_string())),
            Ok(Some(r#"{"success":true,"op":"subscribe"}"#.to_string())),
            Ok(Some(
                r#"{"topic":"order","data":[{"symbol":"BTCUSDT","orderId":"55","orderLinkId":"my",
                "side":"Buy","orderType":"Limit","orderStatus":"Filled","qty":"1","cumExecQty":"1",
                "price":"100","avgPrice":"100"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"topic":"wallet","data":[{"coin":[{"coin":"USDT",
                "availableToWithdraw":"900","locked":"50"}]}]}"#
                    .to_string(),
            )),
        ]);
        bybit.subscribe_user_data().unwrap();
        assert_eq!(ws.connected_urls()[0], "wss://stream.bybit.com/v5/private");
        assert!(ws.sent()[0].contains(r#""op":"auth""#));
        assert!(ws.sent()[0].contains(r#""APIKEY""#));
        assert!(ws.sent()[1].contains(r#""op":"subscribe""#));
        assert!(ws.sent()[1].contains("order"));
        assert!(ws.sent()[1].contains("wallet"));

        let events = bybit.poll_events();
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
        assert_eq!(balances[0].asset, "USDT");
        assert_eq!(balances[0].free, dec!(900));
        assert_eq!(balances[0].locked, dec!(50));
    }

    #[test]
    fn subscribe_user_data_requires_credentials() {
        let ws = Arc::new(MockWsTransport::new());
        let mut bybit = streaming_client(&ws);
        assert!(matches!(
            bybit.subscribe_user_data().unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn keepalive_user_data_pings_the_private_stream() {
        let (mut bybit, ws) = signed_ws_client(1000);
        ws.push_connection(vec![]);
        bybit.subscribe_user_data().unwrap();
        bybit.keepalive_user_data().unwrap();
        assert!(ws.sent().iter().any(|f| f == r#"{"op":"ping"}"#));
    }

    #[test]
    fn keepalive_user_data_is_a_noop_before_subscribe() {
        let (mut bybit, ws) = signed_ws_client(1000);
        bybit.keepalive_user_data().unwrap();
        assert!(ws.sent().is_empty());
    }

    #[test]
    fn dropped_user_data_stream_reconnects_with_a_fresh_auth() {
        let (mut bybit, ws) = signed_ws_client(1000);
        // The first private connection closes on the first recv; the reconnect
        // target is a fresh open connection.
        ws.push_connection(vec![Ok(None)]);
        ws.push_connection(vec![]);
        bybit.subscribe_user_data().unwrap();

        let events = bybit.poll_events();
        assert!(events.contains(&Event::Disconnected));
        assert!(events.contains(&Event::Reconnected));
        // Two private connections (initial + reconnect), each re-signing op:auth.
        let auth_frames = ws
            .sent()
            .into_iter()
            .filter(|f| f.contains(r#""op":"auth""#))
            .count();
        assert_eq!(auth_frames, 2);
        assert_eq!(ws.connected_urls().len(), 2);
        assert_eq!(ws.connected_urls()[1], "wss://stream.bybit.com/v5/private");
    }

    #[test]
    fn the_websocket_frame_carries_the_stp_policy() {
        // Self-trade prevention was set on the request, honoured on the REST
        // path, and dropped on this one -- a policy that is not sent is not
        // applied.
        let (mut bybit, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"op":"auth","retCode":0,"retMsg":"OK"}"#.to_string(),
            )),
            Ok(Some(
                r#"{"reqId":"1000","retCode":0,"retMsg":"OK","op":"order.create",
                "data":{"orderId":"55","orderLinkId":""}}"#
                    .to_string(),
            )),
        ]);
        bybit
            .place_order_ws(
                &OrderRequest::limit_buy(symbol(), dec!(1), dec!(100))
                    .with_stp(SelfTradePrevention::ExpireTaker),
            )
            .unwrap();
        assert!(ws.sent()[1].contains(r#""smpType":"CancelTaker""#));
    }

    #[test]
    fn place_and_cancel_order_over_ws_trade() {
        let (mut bybit, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(
                r#"{"op":"auth","retCode":0,"retMsg":"OK"}"#.to_string(),
            )),
            Ok(Some(
                r#"{"reqId":"1000","retCode":0,"retMsg":"OK","op":"order.create",
                "data":{"orderId":"55","orderLinkId":"my"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"reqId":"1000","retCode":0,"retMsg":"OK","op":"order.cancel",
                "data":{"orderId":"55"}}"#
                    .to_string(),
            )),
        ]);
        let order = bybit
            .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "55");
        assert_eq!(order.client_order_id.as_deref(), Some("my"));
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(ws.connected_urls()[0], "wss://stream.bybit.com/v5/trade");
        // The auth frame is sent first, then the order request.
        assert!(ws.sent()[0].contains(r#""op":"auth""#));
        assert!(ws.sent()[1].contains(r#""op":"order.create""#));
        assert!(ws.sent()[1].contains(r#""symbol":"BTCUSDT""#));

        bybit.cancel_order_ws(&symbol(), "55").unwrap();
        assert!(ws.sent()[2].contains(r#""op":"order.cancel""#));
        assert!(ws.sent()[2].contains(r#""orderId":"55""#));
    }

    #[test]
    fn ws_trade_surfaces_rejection() {
        let (mut bybit, ws) = signed_ws_client(1000);
        ws.push_connection(vec![
            Ok(Some(r#"{"op":"auth","retCode":0}"#.to_string())),
            Ok(Some(
                r#"{"reqId":"1000","retCode":10001,"retMsg":"insufficient balance"}"#.to_string(),
            )),
        ]);
        assert!(matches!(
            bybit
                .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::OrderRejected { .. }
        ));
    }

    #[test]
    fn ws_trade_requires_credentials() {
        let ws = Arc::new(MockWsTransport::new());
        let mut bybit = streaming_client(&ws);
        assert!(matches!(
            bybit
                .place_order_ws(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
                .unwrap_err(),
            Error::InvalidCredentials(_)
        ));
    }

    #[test]
    fn subscribe_sends_op_and_poll_parses_trades_and_book() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"topic":"publicTrade.BTCUSDT","type":"snapshot","data":[
                {"T":1,"s":"BTCUSDT","S":"Buy","v":"0.5","p":"100"},
                {"T":2,"s":"BTCUSDT","S":"Sell","v":"1","p":"101"}]}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"topic":"orderbook.50.BTCUSDT","type":"snapshot","data":{"s":"BTCUSDT","u":10,
                "b":[["100","1"]],"a":[["101","2"]]}}"#
                    .to_string(),
            )),
            Ok(Some(r#"{"success":true,"op":"subscribe"}"#.to_string())),
        ]);
        let mut bybit = streaming_client(&ws);
        bybit.subscribe_trades(&symbol()).unwrap();
        assert_eq!(
            ws.connected_urls(),
            vec!["wss://stream.bybit.com/v5/public/spot".to_string()]
        );
        assert!(ws.sent()[0].contains(r#""op":"subscribe""#));
        assert!(ws.sent()[0].contains("publicTrade.BTCUSDT"));

        let events = bybit.poll_events();
        // 2 trades + 1 book snapshot (the op response is ignored).
        assert_eq!(events.len(), 3);
        let Event::Trade(t) = &events[0] else {
            panic!("expected trade")
        };
        assert_eq!(t.aggressor, OrderSide::Buy);
        assert_eq!(t.price, dec!(100));
        assert!(matches!(events[1], Event::Trade(_)));
        assert!(matches!(events[2], Event::BookSnapshot(_)));
    }

    #[test]
    fn ws_ticker_and_book_delta_parse() {
        let ws = Arc::new(MockWsTransport::new());
        ws.push_connection(vec![
            Ok(Some(
                r#"{"topic":"tickers.BTCUSDT","data":{"symbol":"BTCUSDT","lastPrice":"100",
                "bid1Price":"99","ask1Price":"101","volume24h":"5"}}"#
                    .to_string(),
            )),
            Ok(Some(
                r#"{"topic":"orderbook.50.BTCUSDT","type":"delta","data":{"s":"BTCUSDT","u":11,
                "b":[["100","0"]],"a":[]}}"#
                    .to_string(),
            )),
        ]);
        let mut bybit = streaming_client(&ws);
        bybit.subscribe_ticker(&symbol()).unwrap();
        bybit.subscribe_book(&symbol()).unwrap();
        assert_eq!(ws.connected_urls().len(), 1); // one connection reused

        let events = bybit.poll_events();
        assert_eq!(events.len(), 2);
        let Event::Ticker(ticker) = &events[0] else {
            panic!("expected ticker")
        };
        assert_eq!(ticker.last, dec!(100));
        assert_eq!(ticker.bid, dec!(99));
        let Event::BookDelta(delta) = &events[1] else {
            panic!("expected book delta")
        };
        assert_eq!(delta.final_update_id, 11);
        assert_eq!(delta.bids[0].quantity, dec!(0));
    }

    #[test]
    fn subscribe_without_ws_errors_and_poll_empty() {
        let http = Arc::new(MockHttpTransport::new());
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let mut bybit = Bybit::with_http(Box::new(ArcTransport(http)), &opts);
        assert!(matches!(
            bybit.subscribe_trades(&symbol()).unwrap_err(),
            Error::NotConnected
        ));
        assert!(bybit.poll_events().is_empty());
    }

    #[test]
    fn works_as_a_boxed_exchange() {
        let (bybit, mock) = signed_client(1000);
        mock.push_json(
            200,
            r#"{"retCode":0,"result":{"orderId":"1","orderLinkId":""}}"#,
        );
        let mut exchange: Box<dyn Exchange> = Box::new(bybit);
        assert_eq!(exchange.name(), "bybit");
        let order = exchange
            .place_order(&OrderRequest::limit_buy(symbol(), dec!(1), dec!(100)))
            .unwrap();
        assert_eq!(order.id, "1");
    }

    /// `Debug` reports connection state, never secret material. A client is
    /// formatted into logs and error messages, so anything it prints is
    /// somewhere a credential must not be.
    #[test]
    fn debug_reports_state_without_credentials() {
        let (client, _http) = signed_client(1_700_000_000_000);
        let rendered = format!("{client:?}");

        assert!(rendered.starts_with("Bybit {"));
        assert!(rendered.contains("authenticated: true"));
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("APIKEY"));
    }

    /// The clock offset must reach the wire, not just the struct.
    ///
    /// A venue refuses a signed request whose timestamp is outside its own
    /// receive window, so a machine a few seconds off has every order rejected
    /// -- with a message about the window, not about the clock. This asserts the
    /// whole path: sync, then a signed request carrying the adjusted time.
    #[test]
    fn sync_time_shifts_signed_timestamps() {
        let (mut bybit, http) = signed_client(1_000_000);

        // Shape verified against the live public endpoint on 2026-09-01.
        http.push_json(200, r#"{"retCode":0,"retMsg":"OK","result":{"timeSecond":"1004","timeNano":"1004500000000"}}"#);
        let offset = bybit.sync_time().expect("sync must succeed");
        assert_eq!(offset, 4_500, "the venue is 4.5 s ahead of this machine");

        http.push_json(
            200,
            r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"coin":[]}]}}"#,
        );
        bybit.balances().expect("balances must succeed");

        let requests = http.recorded_requests();
        let signed = requests.last().expect("a signed request was recorded");
        let header = signed
            .headers
            .iter()
            .find(|(name, _)| name == "X-BAPI-TIMESTAMP")
            .map(|(_, value)| value.as_str())
            .expect("signed request carries X-BAPI-TIMESTAMP");
        assert_eq!(header, "1004500", "the venue's time, not ours");
    }

    /// A rate-limited response carries the venue's advised wait.
    ///
    /// The limit itself is recognised from the body -- a code in the error
    /// envelope -- while the wait arrives in the `Retry-After` header, so the
    /// two are read in different places. Until they were joined, every
    /// `RateLimited` this client raised carried `retry_after: None`: a field
    /// the error type documents, and that nothing in the crate ever filled.
    #[test]
    fn rate_limit_carries_the_venues_advised_wait() {
        let (bybit, mock) = client(MarketType::Spot, false);
        mock.push_response(
            crate::transport::HttpResponse::new(
                200,
                r#"{"retCode":10006,"retMsg":"rate","result":{}}"#,
            )
            .with_header("Retry-After", "2.5"),
        );
        let err = bybit.ticker(&symbol()).unwrap_err();
        let wait = std::time::Duration::from_millis(2500);
        assert!(matches!(err, Error::RateLimited { retry_after: Some(d) } if d == wait));
    }
}
