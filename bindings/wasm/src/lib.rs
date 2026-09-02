//! WebAssembly bindings for `wickra-exchange`.
//!
//! Build with:
//! ```text
//! wasm-pack build bindings/wasm --target web --release --features panic-hook
//! ```
//!
//! # What this binding exposes, and why it is not the whole API
//!
//! The Node, Python, C and JVM bindings expose live venue clients. This one
//! cannot, and the reason is not effort: a live client needs TCP sockets and a
//! TLS stack, and `wasm32-unknown-unknown` has neither. The transport crate
//! (`wickra-exchange`) is built on tokio, reqwest and tokio-tungstenite, none of
//! which target the browser. A `connect()` here would be a function that
//! compiles and then fails at the first request.
//!
//! So this binding is deliberately scoped to the part of the library that is
//! *pure computation* and therefore genuinely runs in a browser:
//!
//! - [`Exchange::paper`] — an offline account with fees and slippage,
//! - [`Exchange::replay_trades`] — a recorded price tape filled against that
//!   account,
//! - order placement, cancellation, balances, ticker and event draining against
//!   either.
//!
//! That is the same surface a backtest uses, which makes browser-side strategy
//! demos and replay UIs possible without a server. What is absent —
//! `connect`, user-data streams, WebSocket execution, derivatives, `klines` —
//! is absent because it needs a network, not because it was skipped.
//!
//! Depth (`order_book`) is absent for a different reason: the paper account has
//! no depth feed and returns `unsupported`, and the replay backend delegates
//! straight to it, so on both backends reachable from here the call cannot
//! succeed. Exposing it would add a method that compiles, type-checks and always
//! throws.

#![allow(clippy::needless_pass_by_value)]
// wasm-bindgen generates the JS-facing type machinery on these types.
#![allow(missing_debug_implementations)]

use std::collections::HashMap;

use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use wasm_bindgen::prelude::*;

use wickra_exchange_core::{
    Event, Exchange as CoreExchange, Order, OrderRequest as CoreOrderRequest, OrderSide,
    OrderStatus, PaperExchange, ReplayExchange, Symbol, TradePrint,
};

/// Route Rust panics to `console.error` with a readable stack.
///
/// Enabled by the `panic-hook` feature; without it a panic surfaces in JS as
/// "unreachable executed" with nothing pointing at the cause.
#[cfg(feature = "panic-hook")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

fn err(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

fn map_err<E: std::fmt::Display>(e: E) -> JsValue {
    err(e.to_string())
}

fn parse_symbol(market: &str) -> Result<Symbol, JsValue> {
    match market.split_once('/') {
        Some((base, quote)) if !base.is_empty() && !quote.is_empty() => {
            Ok(Symbol::new(base, quote))
        }
        _ => Err(err(format!("market must be 'BASE/QUOTE', got {market:?}"))),
    }
}

fn to_decimal(value: f64) -> Result<Decimal, JsValue> {
    Decimal::from_f64(value).ok_or_else(|| err(format!("{value} is not a finite number")))
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

/// Serialise a plain payload struct into a JS object.
///
/// `serialize_maps_as_objects` is not the default and has to be asked for: out
/// of the box `serde_wasm_bindgen` turns a `HashMap` into a JS `Map`, so
/// `balances().BTC` reads back `undefined` and only an explicit `.get("BTC")`
/// works. The Node binding hands back a `Record<string, number>`, and the two
/// JavaScript bindings returning different container types for the same call
/// would be a trap that no type signature warns about.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    value.serialize(&serializer).map_err(map_err)
}

/// Library version (matches the Rust crate version).
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// An order as reported by the exchange.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderInfo {
    id: String,
    client_order_id: Option<String>,
    symbol: String,
    side: String,
    status: String,
    quantity: f64,
    filled_quantity: f64,
    price: Option<f64>,
    average_price: Option<f64>,
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

/// The current best prices and rolling volume for one market.
#[derive(Serialize)]
struct TickerInfo {
    symbol: String,
    last: f64,
    bid: f64,
    ask: f64,
    volume: f64,
}

/// A single stream event. `kind` discriminates the payload.
#[derive(Serialize)]
struct StreamEvent {
    kind: String,
    symbol: Option<String>,
    price: Option<f64>,
    quantity: Option<f64>,
    side: Option<String>,
    timestamp: Option<i64>,
    order: Option<OrderInfo>,
    balances: Option<HashMap<String, f64>>,
    channel: Option<String>,
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

/// An order request, built with the market/limit factory methods.
#[wasm_bindgen]
pub struct OrderRequest {
    inner: CoreOrderRequest,
}

#[wasm_bindgen]
impl OrderRequest {
    #[wasm_bindgen(js_name = marketBuy)]
    pub fn market_buy(market: &str, quantity: f64) -> Result<OrderRequest, JsValue> {
        Ok(Self {
            inner: CoreOrderRequest::market_buy(parse_symbol(market)?, to_decimal(quantity)?),
        })
    }

    #[wasm_bindgen(js_name = marketSell)]
    pub fn market_sell(market: &str, quantity: f64) -> Result<OrderRequest, JsValue> {
        Ok(Self {
            inner: CoreOrderRequest::market_sell(parse_symbol(market)?, to_decimal(quantity)?),
        })
    }

    #[wasm_bindgen(js_name = limitBuy)]
    pub fn limit_buy(market: &str, quantity: f64, price: f64) -> Result<OrderRequest, JsValue> {
        Ok(Self {
            inner: CoreOrderRequest::limit_buy(
                parse_symbol(market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }

    #[wasm_bindgen(js_name = limitSell)]
    pub fn limit_sell(market: &str, quantity: f64, price: f64) -> Result<OrderRequest, JsValue> {
        Ok(Self {
            inner: CoreOrderRequest::limit_sell(
                parse_symbol(market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }
}

enum Inner {
    Paper(PaperExchange),
    Replay(ReplayExchange),
}

impl Inner {
    fn as_exchange(&mut self) -> &mut dyn CoreExchange {
        match self {
            Inner::Paper(paper) => paper,
            Inner::Replay(replay) => replay,
        }
    }
}

/// An offline exchange: a paper account, or a replay tape filled against one.
///
/// Both implement the same `Exchange` API the live clients do in the other
/// bindings, so a strategy written against this runs unchanged on a live venue
/// once it moves off the browser.
#[wasm_bindgen]
pub struct Exchange {
    inner: Inner,
}

/// Build a paper account from a `Record<string, number>` of balances.
fn paper_from(
    balances: &JsValue,
    maker_bps: Option<f64>,
    taker_bps: Option<f64>,
    slippage_bps: Option<f64>,
) -> Result<PaperExchange, JsValue> {
    let balances: HashMap<String, f64> =
        serde_wasm_bindgen::from_value(balances.clone()).map_err(map_err)?;
    let mut paper = PaperExchange::new()
        .with_fees(
            to_decimal(maker_bps.unwrap_or(0.0))?,
            to_decimal(taker_bps.unwrap_or(0.0))?,
        )
        .with_slippage_bps(to_decimal(slippage_bps.unwrap_or(0.0))?);
    for (asset, amount) in balances {
        paper = paper.with_balance(asset, to_decimal(amount)?);
    }
    Ok(paper)
}

#[wasm_bindgen]
impl Exchange {
    /// An offline paper account seeded from `balances` (asset -> amount), with
    /// optional maker/taker fees and slippage in basis points.
    pub fn paper(
        balances: &JsValue,
        maker_bps: Option<f64>,
        taker_bps: Option<f64>,
        slippage_bps: Option<f64>,
    ) -> Result<Exchange, JsValue> {
        Ok(Self {
            inner: Inner::Paper(paper_from(balances, maker_bps, taker_bps, slippage_bps)?),
        })
    }

    /// A replay account driven by a recorded price `tape` of `market` trades,
    /// filling against a paper book seeded from `balances`.
    #[wasm_bindgen(js_name = replayTrades)]
    pub fn replay_trades(
        market: &str,
        tape: Vec<f64>,
        balances: &JsValue,
        maker_bps: Option<f64>,
        taker_bps: Option<f64>,
        slippage_bps: Option<f64>,
    ) -> Result<Exchange, JsValue> {
        let symbol = parse_symbol(market)?;
        let paper = paper_from(balances, maker_bps, taker_bps, slippage_bps)?;
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

    /// The backend's lowercase identifier (`"paper"` or `"replay"`).
    #[must_use]
    pub fn name(&self) -> String {
        match &self.inner {
            Inner::Paper(paper) => paper.name().to_string(),
            Inner::Replay(replay) => replay.name().to_string(),
        }
    }

    /// Set the mark price a paper account fills against (paper backend only).
    #[wasm_bindgen(js_name = setPrice)]
    pub fn set_price(&mut self, market: &str, price: f64) -> Result<(), JsValue> {
        match &mut self.inner {
            Inner::Paper(paper) => {
                paper.set_price(&parse_symbol(market)?, to_decimal(price)?);
                Ok(())
            }
            Inner::Replay(_) => Err(err("set_price is only supported on a paper exchange")),
        }
    }

    /// Place an order; returns the resulting order.
    #[wasm_bindgen(js_name = placeOrder)]
    pub fn place_order(&mut self, request: &OrderRequest) -> Result<JsValue, JsValue> {
        let order = self
            .inner
            .as_exchange()
            .place_order(&request.inner)
            .map_err(map_err)?;
        to_js(&OrderInfo::from(&order))
    }

    /// Cancel an open order by id.
    #[wasm_bindgen(js_name = cancelOrder)]
    pub fn cancel_order(&mut self, market: &str, order_id: &str) -> Result<(), JsValue> {
        self.inner
            .as_exchange()
            .cancel_order(&parse_symbol(market)?, order_id)
            .map_err(map_err)
    }

    /// The current ticker for `market`.
    pub fn ticker(&mut self, market: &str) -> Result<JsValue, JsValue> {
        let ticker = self
            .inner
            .as_exchange()
            .ticker(&parse_symbol(market)?)
            .map_err(map_err)?;
        to_js(&TickerInfo {
            symbol: ticker.symbol.to_string(),
            last: to_float(ticker.last),
            bid: to_float(ticker.bid),
            ask: to_float(ticker.ask),
            volume: to_float(ticker.volume),
        })
    }

    /// Account balances as an `asset -> free amount` object.
    pub fn balances(&mut self) -> Result<JsValue, JsValue> {
        let balances = self.inner.as_exchange().balances().map_err(map_err)?;
        let map: HashMap<String, f64> = balances
            .into_iter()
            .map(|b| (b.asset, to_float(b.free)))
            .collect();
        to_js(&map)
    }

    /// Look up a single order by id.
    #[wasm_bindgen(js_name = queryOrder)]
    pub fn query_order(&mut self, market: &str, order_id: &str) -> Result<JsValue, JsValue> {
        let order = self
            .inner
            .as_exchange()
            .query_order(&parse_symbol(market)?, order_id)
            .map_err(map_err)?;
        to_js(&OrderInfo::from(&order))
    }

    /// Open orders, optionally filtered to one `market`.
    #[wasm_bindgen(js_name = openOrders)]
    pub fn open_orders(&mut self, market: Option<String>) -> Result<JsValue, JsValue> {
        let symbol = match market {
            Some(market) => Some(parse_symbol(&market)?),
            None => None,
        };
        let orders = self
            .inner
            .as_exchange()
            .open_orders(symbol.as_ref())
            .map_err(map_err)?;
        let infos: Vec<OrderInfo> = orders.iter().map(OrderInfo::from).collect();
        to_js(&infos)
    }

    /// Drain all events buffered since the last call.
    #[wasm_bindgen(js_name = pollEvents)]
    pub fn poll_events(&mut self) -> Result<JsValue, JsValue> {
        let events: Vec<StreamEvent> = self
            .inner
            .as_exchange()
            .poll_events()
            .iter()
            .map(StreamEvent::from_event)
            .collect();
        to_js(&events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_price_arrives_as_the_number_the_caller_typed() {
        // This binding takes prices and quantities as floats, so the
        // conversion back is the whole of the fidelity. It used to be
        // `from_f64_retain`, which keeps the *binary* expansion of the double:
        // a caller asking for 20000.15 sent "20000.150000000001455191522832"
        // to the venue, which a price filter rejects and a tick check rounds
        // away from.
        for (typed, expected) in [
            (20000.15_f64, "20000.15"),
            (0.1, "0.1"),
            (1.005, "1.005"),
            (0.000_000_01, "0.00000001"),
        ] {
            assert_eq!(
                wickra_exchange_core::format_decimal(to_decimal(typed).unwrap()),
                expected
            );
        }
        // The non-finite guard is not asserted here. This binding reports
        // errors as a `JsValue`, and constructing one outside a wasm runtime
        // aborts the process inside wasm-bindgen rather than failing a test.
        // The same guard is asserted natively in the C ABI, Python and Node
        // suites, which run on the host.
    }
}
