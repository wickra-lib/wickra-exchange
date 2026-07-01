//! Node.js bindings for `wickra-exchange` via napi-rs.
//!
//! Build with:
//! ```text
//! cd bindings/node && npm install && npm run build
//! ```
//!
//! Then `require("wickra-exchange")` from Node. This is thin glue over the
//! crate's synchronous, pull-based [`Exchange`] API: build credentials and order
//! requests, open a client (live venue, or the offline paper / replay
//! simulators), then place orders and drain events — the same strategy runs
//! paper, replay and live by swapping the constructor.

#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::unused_self)]
// napi-derive generates the Node-facing debug/type machinery.
#![allow(missing_debug_implementations)]

use std::collections::HashMap;

use napi::bindgen_prelude::{Error as NapiError, Status};
use napi_derive::napi;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use wickra_exchange::{
    connect, connect_advanced, connect_derivatives, AdvancedOrders as CoreAdvancedOrders,
    Credentials as CoreCredentials, Derivatives as CoreDerivatives, Event,
    Exchange as CoreExchange, ExchangeOptions, MarginMode, MarketType, OcoRequest, Order,
    OrderRequest as CoreOrderRequest, OrderSide, OrderStatus, PaperExchange, Position,
    PositionSide, ReplayExchange, Symbol, TradePrint,
};

fn err(message: impl Into<String>) -> NapiError {
    NapiError::new(Status::InvalidArg, message.into())
}

fn map_err<E: std::fmt::Display>(e: E) -> NapiError {
    err(e.to_string())
}

fn parse_symbol(market: &str) -> napi::Result<Symbol> {
    match market.split_once('/') {
        Some((base, quote)) if !base.is_empty() && !quote.is_empty() => {
            Ok(Symbol::new(base, quote))
        }
        _ => Err(err(format!("market must be 'BASE/QUOTE', got {market:?}"))),
    }
}

fn to_decimal(value: f64) -> napi::Result<Decimal> {
    Decimal::from_f64_retain(value).ok_or_else(|| err(format!("{value} is not a finite number")))
}

fn to_float(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

fn side_str(side: OrderSide) -> String {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
    .to_string()
}

fn status_str(status: OrderStatus) -> String {
    match status {
        OrderStatus::New => "new",
        OrderStatus::PartiallyFilled => "partially_filled",
        OrderStatus::Filled => "filled",
        OrderStatus::Canceled => "canceled",
        OrderStatus::Rejected => "rejected",
        OrderStatus::Expired => "expired",
    }
    .to_string()
}

/// Library version (matches the Rust crate version).
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// An order as reported by the exchange.
#[napi(object)]
pub struct OrderInfo {
    pub id: String,
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: String,
    pub status: String,
    pub quantity: f64,
    pub filled_quantity: f64,
    pub price: Option<f64>,
    pub average_price: Option<f64>,
}

impl From<&Order> for OrderInfo {
    fn from(order: &Order) -> Self {
        Self {
            id: order.id.clone(),
            client_order_id: order.client_order_id.clone(),
            symbol: order.symbol.to_string(),
            side: side_str(order.side),
            status: status_str(order.status),
            quantity: to_float(order.quantity),
            filled_quantity: to_float(order.filled_quantity),
            price: order.price.map(to_float),
            average_price: order.average_price.map(to_float),
        }
    }
}

fn position_side_str(side: PositionSide) -> String {
    match side {
        PositionSide::Long => "long",
        PositionSide::Short => "short",
    }
    .to_string()
}

fn margin_mode_from_str(mode: &str) -> napi::Result<MarginMode> {
    match mode {
        "cross" => Ok(MarginMode::Cross),
        "isolated" => Ok(MarginMode::Isolated),
        other => Err(err(format!(
            "margin_mode must be 'cross' or 'isolated', got {other:?}"
        ))),
    }
}

fn side_from_str(side: &str) -> napi::Result<OrderSide> {
    match side {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        other => Err(err(format!("side must be 'buy' or 'sell', got {other:?}"))),
    }
}

/// A derivatives position.
#[napi(object)]
pub struct PositionInfo {
    pub symbol: String,
    pub side: String,
    pub quantity: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub leverage: f64,
    pub unrealized_pnl: f64,
    pub margin_mode: String,
}

impl From<&Position> for PositionInfo {
    fn from(position: &Position) -> Self {
        Self {
            symbol: position.symbol.to_string(),
            side: position_side_str(position.side),
            quantity: to_float(position.quantity),
            entry_price: to_float(position.entry_price),
            mark_price: to_float(position.mark_price),
            leverage: to_float(position.leverage),
            unrealized_pnl: to_float(position.unrealized_pnl),
            margin_mode: match position.margin_mode {
                MarginMode::Cross => "cross",
                MarginMode::Isolated => "isolated",
            }
            .to_string(),
        }
    }
}

/// A point-in-time ticker.
#[napi(object)]
pub struct TickerInfo {
    pub symbol: String,
    pub last: f64,
    pub bid: f64,
    pub ask: f64,
    pub volume: f64,
}

/// A single stream event. `kind` discriminates the payload
/// (`"trade"`, `"ticker"`, `"order_update"`, `"balance_update"`, ...).
#[napi(object)]
pub struct StreamEvent {
    pub kind: String,
    pub symbol: Option<String>,
    pub price: Option<f64>,
    pub quantity: Option<f64>,
    pub side: Option<String>,
    pub timestamp: Option<i64>,
    pub order: Option<OrderInfo>,
    pub balances: Option<HashMap<String, f64>>,
    pub channel: Option<String>,
}

impl StreamEvent {
    fn empty(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            symbol: None,
            price: None,
            quantity: None,
            side: None,
            timestamp: None,
            order: None,
            balances: None,
            channel: None,
        }
    }

    fn from_event(event: &Event) -> Self {
        match event {
            Event::Trade(trade) => StreamEvent {
                symbol: Some(trade.symbol.to_string()),
                price: Some(to_float(trade.price)),
                quantity: Some(to_float(trade.quantity)),
                side: Some(side_str(trade.aggressor)),
                timestamp: Some(trade.timestamp),
                ..Self::empty("trade")
            },
            Event::Ticker(ticker) => StreamEvent {
                symbol: Some(ticker.symbol.to_string()),
                price: Some(to_float(ticker.last)),
                ..Self::empty("ticker")
            },
            Event::OrderUpdate(order) => StreamEvent {
                order: Some(OrderInfo::from(order)),
                ..Self::empty("order_update")
            },
            Event::BalanceUpdate(balances) => StreamEvent {
                balances: Some(
                    balances
                        .iter()
                        .map(|b| (b.asset.clone(), to_float(b.free)))
                        .collect(),
                ),
                ..Self::empty("balance_update")
            },
            Event::Subscribed { channel } => StreamEvent {
                channel: Some(channel.clone()),
                ..Self::empty("subscribed")
            },
            other => Self::empty(&format!("{other:?}")),
        }
    }
}

/// API credentials for a venue.
#[napi]
pub struct Credentials {
    inner: CoreCredentials,
}

#[napi]
impl Credentials {
    #[napi(constructor)]
    pub fn new(
        api_key: String,
        api_secret: String,
        passphrase: Option<String>,
        private_key: Option<String>,
    ) -> Self {
        let mut inner = CoreCredentials::new(api_key, api_secret);
        if let Some(passphrase) = passphrase {
            inner = inner.with_passphrase(passphrase);
        }
        if let Some(private_key) = private_key {
            inner = inner.with_private_key(private_key);
        }
        Self { inner }
    }
}

/// An order request, built with the market/limit factory methods.
#[napi]
pub struct OrderRequest {
    inner: CoreOrderRequest,
}

#[napi]
impl OrderRequest {
    #[napi(factory)]
    pub fn market_buy(market: String, quantity: f64) -> napi::Result<Self> {
        Ok(Self {
            inner: CoreOrderRequest::market_buy(parse_symbol(&market)?, to_decimal(quantity)?),
        })
    }

    #[napi(factory)]
    pub fn market_sell(market: String, quantity: f64) -> napi::Result<Self> {
        Ok(Self {
            inner: CoreOrderRequest::market_sell(parse_symbol(&market)?, to_decimal(quantity)?),
        })
    }

    #[napi(factory)]
    pub fn limit_buy(market: String, quantity: f64, price: f64) -> napi::Result<Self> {
        Ok(Self {
            inner: CoreOrderRequest::limit_buy(
                parse_symbol(&market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }

    #[napi(factory)]
    pub fn limit_sell(market: String, quantity: f64, price: f64) -> napi::Result<Self> {
        Ok(Self {
            inner: CoreOrderRequest::limit_sell(
                parse_symbol(&market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }
}

enum Inner {
    Paper(PaperExchange),
    Replay(ReplayExchange),
    Live(Box<dyn CoreExchange>),
}

impl Inner {
    fn as_exchange(&mut self) -> &mut dyn CoreExchange {
        match self {
            Inner::Paper(paper) => paper,
            Inner::Replay(replay) => replay,
            Inner::Live(live) => live.as_mut(),
        }
    }
}

/// A unified exchange client over the synchronous, pull-based API.
#[napi]
pub struct Exchange {
    inner: Inner,
}

#[napi]
impl Exchange {
    /// An offline paper account seeded from `balances` (asset -> amount), with
    /// optional maker/taker fees and slippage in basis points.
    #[napi(factory)]
    pub fn paper(
        balances: HashMap<String, f64>,
        maker_bps: Option<f64>,
        taker_bps: Option<f64>,
        slippage_bps: Option<f64>,
    ) -> napi::Result<Self> {
        let mut paper = PaperExchange::new()
            .with_fees(
                to_decimal(maker_bps.unwrap_or(0.0))?,
                to_decimal(taker_bps.unwrap_or(0.0))?,
            )
            .with_slippage_bps(to_decimal(slippage_bps.unwrap_or(0.0))?);
        for (asset, amount) in balances {
            paper = paper.with_balance(asset, to_decimal(amount)?);
        }
        Ok(Self {
            inner: Inner::Paper(paper),
        })
    }

    /// A replay account driven by a recorded price `tape` of `market` trades,
    /// filling against a paper book seeded from `balances`.
    #[napi(factory)]
    pub fn replay_trades(
        market: String,
        tape: Vec<f64>,
        balances: HashMap<String, f64>,
        maker_bps: Option<f64>,
        taker_bps: Option<f64>,
        slippage_bps: Option<f64>,
    ) -> napi::Result<Self> {
        let symbol = parse_symbol(&market)?;
        let mut paper = PaperExchange::new()
            .with_fees(
                to_decimal(maker_bps.unwrap_or(0.0))?,
                to_decimal(taker_bps.unwrap_or(0.0))?,
            )
            .with_slippage_bps(to_decimal(slippage_bps.unwrap_or(0.0))?);
        for (asset, amount) in balances {
            paper = paper.with_balance(asset, to_decimal(amount)?);
        }
        let mut frames = Vec::with_capacity(tape.len());
        for (index, price) in tape.into_iter().enumerate() {
            frames.push(Event::Trade(TradePrint {
                symbol: symbol.clone(),
                price: to_decimal(price)?,
                quantity: Decimal::ONE,
                aggressor: OrderSide::Buy,
                timestamp: i64::try_from(index).unwrap_or(i64::MAX),
            }));
        }
        Ok(Self {
            inner: Inner::Replay(ReplayExchange::with_paper(frames, paper)),
        })
    }

    /// A live client for `name` (see the crate README for the ten supported
    /// venues), authenticated with `credentials`.
    #[napi(factory)]
    pub fn connect(
        name: String,
        credentials: &Credentials,
        testnet: Option<bool>,
    ) -> napi::Result<Self> {
        let options = if testnet.unwrap_or(false) {
            ExchangeOptions::testnet(MarketType::Spot)
        } else {
            ExchangeOptions::mainnet(MarketType::Spot)
        };
        let live = connect(&name, credentials.inner.clone(), &options).map_err(map_err)?;
        Ok(Self {
            inner: Inner::Live(live),
        })
    }

    /// The venue's lowercase identifier (`"paper"`, `"replay"`, `"binance"`, ...).
    #[napi]
    pub fn name(&self) -> String {
        match &self.inner {
            Inner::Paper(paper) => paper.name().to_string(),
            Inner::Replay(replay) => replay.name().to_string(),
            Inner::Live(live) => live.name().to_string(),
        }
    }

    /// Set the mark price a paper account fills against (paper backend only).
    #[napi]
    pub fn set_price(&mut self, market: String, price: f64) -> napi::Result<()> {
        match &mut self.inner {
            Inner::Paper(paper) => {
                paper.set_price(&parse_symbol(&market)?, to_decimal(price)?);
                Ok(())
            }
            _ => Err(err("set_price is only supported on a paper exchange")),
        }
    }

    /// Place an order; returns the resulting order.
    #[napi]
    pub fn place_order(&mut self, request: &OrderRequest) -> napi::Result<OrderInfo> {
        let order = self
            .inner
            .as_exchange()
            .place_order(&request.inner)
            .map_err(map_err)?;
        Ok(OrderInfo::from(&order))
    }

    /// Cancel an open order by venue id.
    #[napi]
    pub fn cancel_order(&mut self, market: String, order_id: String) -> napi::Result<()> {
        self.inner
            .as_exchange()
            .cancel_order(&parse_symbol(&market)?, &order_id)
            .map_err(map_err)
    }

    /// The current ticker for `market`.
    #[napi]
    pub fn ticker(&mut self, market: String) -> napi::Result<TickerInfo> {
        let ticker = self
            .inner
            .as_exchange()
            .ticker(&parse_symbol(&market)?)
            .map_err(map_err)?;
        Ok(TickerInfo {
            symbol: ticker.symbol.to_string(),
            last: to_float(ticker.last),
            bid: to_float(ticker.bid),
            ask: to_float(ticker.ask),
            volume: to_float(ticker.volume),
        })
    }

    /// Account balances as an `asset -> free amount` map.
    #[napi]
    pub fn balances(&mut self) -> napi::Result<HashMap<String, f64>> {
        let balances = self.inner.as_exchange().balances().map_err(map_err)?;
        Ok(balances
            .into_iter()
            .map(|b| (b.asset, to_float(b.free)))
            .collect())
    }

    /// Subscribe to the public trade stream for `market`.
    #[napi]
    pub fn subscribe_trades(&mut self, market: String) -> napi::Result<()> {
        self.inner
            .as_exchange()
            .subscribe_trades(&parse_symbol(&market)?)
            .map_err(map_err)
    }

    /// Drain all events buffered since the last call.
    #[napi]
    pub fn poll_events(&mut self) -> Vec<StreamEvent> {
        self.inner
            .as_exchange()
            .poll_events()
            .iter()
            .map(StreamEvent::from_event)
            .collect()
    }
}

/// A live derivatives (futures/perpetual) client: positions, leverage, margin
/// mode and reduce-only close. Available on the eight venues with futures markets.
#[napi]
pub struct Derivatives {
    inner: Box<dyn CoreDerivatives>,
}

#[napi]
impl Derivatives {
    /// Connect a USDⓈ-M futures client for `name`. Throws for a spot-only venue.
    #[napi(factory)]
    pub fn connect(
        name: String,
        credentials: &Credentials,
        testnet: Option<bool>,
    ) -> napi::Result<Self> {
        let options = if testnet.unwrap_or(false) {
            ExchangeOptions::testnet(MarketType::UsdMFutures)
        } else {
            ExchangeOptions::mainnet(MarketType::UsdMFutures)
        };
        let inner =
            connect_derivatives(&name, credentials.inner.clone(), &options).map_err(map_err)?;
        Ok(Self { inner })
    }

    /// Open positions, optionally filtered to one `market`.
    #[napi]
    pub fn positions(&mut self, market: Option<String>) -> napi::Result<Vec<PositionInfo>> {
        let symbol = market.map(|m| parse_symbol(&m)).transpose()?;
        Ok(self
            .inner
            .positions(symbol.as_ref())
            .map_err(map_err)?
            .iter()
            .map(PositionInfo::from)
            .collect())
    }

    /// Set the leverage for `market`.
    #[napi]
    pub fn set_leverage(&mut self, market: String, leverage: u32) -> napi::Result<()> {
        self.inner
            .set_leverage(&parse_symbol(&market)?, leverage)
            .map_err(map_err)
    }

    /// Set the margin mode for `market` (`"cross"` or `"isolated"`).
    #[napi]
    pub fn set_margin_mode(&mut self, market: String, mode: String) -> napi::Result<()> {
        self.inner
            .set_margin_mode(&parse_symbol(&market)?, margin_mode_from_str(&mode)?)
            .map_err(map_err)
    }

    /// Flatten the open position in `market` with a reduce-only market order.
    #[napi]
    pub fn close_position(&mut self, market: String) -> napi::Result<OrderInfo> {
        let order = self
            .inner
            .close_position(&parse_symbol(&market)?)
            .map_err(map_err)?;
        Ok(OrderInfo::from(&order))
    }
}

/// A live advanced-orders client: amend, batch place/cancel and OCO. Available
/// on the eight trading venues (native where supported, else a thrown error).
#[napi]
pub struct AdvancedOrders {
    inner: Box<dyn CoreAdvancedOrders>,
}

#[napi]
impl AdvancedOrders {
    /// Connect an advanced-orders client for `name`. `futures` selects the
    /// USDⓈ-M futures market. Throws for a venue without an advanced-order surface.
    #[napi(factory)]
    pub fn connect(
        name: String,
        credentials: &Credentials,
        testnet: Option<bool>,
        futures: Option<bool>,
    ) -> napi::Result<Self> {
        let market_type = if futures.unwrap_or(false) {
            MarketType::UsdMFutures
        } else {
            MarketType::Spot
        };
        let options = if testnet.unwrap_or(false) {
            ExchangeOptions::testnet(market_type)
        } else {
            ExchangeOptions::mainnet(market_type)
        };
        let inner =
            connect_advanced(&name, credentials.inner.clone(), &options).map_err(map_err)?;
        Ok(Self { inner })
    }

    /// Amend a resting order's price and/or quantity in place; `null`/`undefined`
    /// leaves that field unchanged. Returns the refreshed order.
    #[napi]
    pub fn amend_order(
        &mut self,
        market: String,
        order_id: String,
        new_price: Option<f64>,
        new_quantity: Option<f64>,
    ) -> napi::Result<OrderInfo> {
        let price = new_price.map(to_decimal).transpose()?;
        let quantity = new_quantity.map(to_decimal).transpose()?;
        let order = self
            .inner
            .amend_order(&parse_symbol(&market)?, &order_id, price, quantity)
            .map_err(map_err)?;
        Ok(OrderInfo::from(&order))
    }

    /// Cancel several orders on `market` in one request.
    #[napi]
    pub fn cancel_batch(&mut self, market: String, order_ids: Vec<String>) -> napi::Result<()> {
        self.inner
            .cancel_batch(&parse_symbol(&market)?, &order_ids)
            .map_err(map_err)
    }

    /// Place a one-cancels-other bracket (take-profit `price` + `stopPrice`
    /// trigger, optional `stopLimitPrice`); returns the resulting order legs.
    #[napi]
    pub fn place_oco(
        &mut self,
        market: String,
        side: String,
        quantity: f64,
        price: f64,
        stop_price: f64,
        stop_limit_price: Option<f64>,
    ) -> napi::Result<Vec<OrderInfo>> {
        let mut request = OcoRequest::new(
            parse_symbol(&market)?,
            side_from_str(&side)?,
            to_decimal(quantity)?,
            to_decimal(price)?,
            to_decimal(stop_price)?,
        );
        if let Some(slp) = stop_limit_price {
            request = request.with_stop_limit_price(to_decimal(slp)?);
        }
        Ok(self
            .inner
            .place_oco(&request)
            .map_err(map_err)?
            .iter()
            .map(OrderInfo::from)
            .collect())
    }
}
