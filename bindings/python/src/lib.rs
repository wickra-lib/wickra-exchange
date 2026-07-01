//! Python bindings for `wickra-exchange`, exposed under the `wickra_exchange`
//! package.
//!
//! This is thin glue over the crate's synchronous, pull-based [`Exchange`] API:
//! build [`Credentials`] and [`OrderRequest`], open a client (live venue via
//! [`connect`], or the offline [`PaperExchange`] / [`ReplayExchange`]
//! simulators), then place orders and drain events with the same loop you would
//! run in Rust. The same strategy therefore runs paper, replay and live by
//! swapping the constructor — from Python.

// Python protocol methods take `self` by value/ref regardless of use, and PyO3
// extractor signatures pass owned values; both trip these pedantic lints.
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::unused_self)]

use std::collections::HashMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use wickra_exchange::{
    connect, Credentials, Event, Exchange, ExchangeOptions, MarketType, Order, OrderRequest,
    OrderSide, OrderStatus, PaperExchange, ReplayExchange, Symbol, Ticker, TradePrint,
};

/// Parse a `"BASE/QUOTE"` market string into a [`Symbol`].
fn parse_symbol(market: &str) -> PyResult<Symbol> {
    match market.split_once('/') {
        Some((base, quote)) if !base.is_empty() && !quote.is_empty() => {
            Ok(Symbol::new(base, quote))
        }
        _ => Err(PyValueError::new_err(format!(
            "market must be 'BASE/QUOTE', got {market:?}"
        ))),
    }
}

/// Convert a Python float to an exact [`Decimal`].
fn to_decimal(value: f64) -> PyResult<Decimal> {
    Decimal::from_f64_retain(value)
        .ok_or_else(|| PyValueError::new_err(format!("{value} is not a finite number")))
}

/// Convert a [`Decimal`] to a Python-facing float.
fn to_float(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

fn side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

fn status_str(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::New => "new",
        OrderStatus::PartiallyFilled => "partially_filled",
        OrderStatus::Filled => "filled",
        OrderStatus::Canceled => "canceled",
        OrderStatus::Rejected => "rejected",
        OrderStatus::Expired => "expired",
    }
}

/// Build a Python dict describing an [`Order`].
fn order_dict<'py>(py: Python<'py>, order: &Order) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", &order.id)?;
    dict.set_item("client_order_id", order.client_order_id.clone())?;
    dict.set_item("symbol", order.symbol.to_string())?;
    dict.set_item("side", side_str(order.side))?;
    dict.set_item("status", status_str(order.status))?;
    dict.set_item("quantity", to_float(order.quantity))?;
    dict.set_item("filled_quantity", to_float(order.filled_quantity))?;
    dict.set_item("price", order.price.map(to_float))?;
    dict.set_item("average_price", order.average_price.map(to_float))?;
    Ok(dict)
}

/// Build a Python dict describing a stream [`Event`].
fn event_dict<'py>(py: Python<'py>, event: &Event) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    match event {
        Event::Trade(trade) => {
            dict.set_item("type", "trade")?;
            dict.set_item("symbol", trade.symbol.to_string())?;
            dict.set_item("price", to_float(trade.price))?;
            dict.set_item("quantity", to_float(trade.quantity))?;
            dict.set_item("side", side_str(trade.aggressor))?;
            dict.set_item("timestamp", trade.timestamp)?;
        }
        Event::Ticker(ticker) => {
            dict.set_item("type", "ticker")?;
            dict.set_item("symbol", ticker.symbol.to_string())?;
            dict.set_item("last", to_float(ticker.last))?;
        }
        Event::OrderUpdate(order) => {
            dict.set_item("type", "order_update")?;
            dict.set_item("order", order_dict(py, order)?)?;
        }
        Event::BalanceUpdate(balances) => {
            dict.set_item("type", "balance_update")?;
            let map: HashMap<String, f64> = balances
                .iter()
                .map(|b| (b.asset.clone(), to_float(b.free)))
                .collect();
            dict.set_item("balances", map)?;
        }
        Event::Subscribed { channel } => {
            dict.set_item("type", "subscribed")?;
            dict.set_item("channel", channel)?;
        }
        other => {
            // BookSnapshot / BookDelta / Disconnected / Reconnected: expose the
            // discriminant so a Python loop can still branch on it.
            dict.set_item("type", format!("{other:?}"))?;
        }
    }
    Ok(dict)
}

/// API credentials for a venue.
#[pyclass(name = "Credentials")]
pub struct PyCredentials {
    inner: Credentials,
}

#[pymethods]
impl PyCredentials {
    #[new]
    #[pyo3(signature = (api_key, api_secret, passphrase=None, private_key=None))]
    fn new(
        api_key: String,
        api_secret: String,
        passphrase: Option<String>,
        private_key: Option<String>,
    ) -> Self {
        let mut inner = Credentials::new(api_key, api_secret);
        if let Some(passphrase) = passphrase {
            inner = inner.with_passphrase(passphrase);
        }
        if let Some(private_key) = private_key {
            inner = inner.with_private_key(private_key);
        }
        Self { inner }
    }
}

/// An order request, built with the market/limit constructors.
#[pyclass(name = "OrderRequest")]
pub struct PyOrderRequest {
    inner: OrderRequest,
}

#[pymethods]
impl PyOrderRequest {
    #[staticmethod]
    fn market_buy(market: &str, quantity: f64) -> PyResult<Self> {
        Ok(Self {
            inner: OrderRequest::market_buy(parse_symbol(market)?, to_decimal(quantity)?),
        })
    }

    #[staticmethod]
    fn market_sell(market: &str, quantity: f64) -> PyResult<Self> {
        Ok(Self {
            inner: OrderRequest::market_sell(parse_symbol(market)?, to_decimal(quantity)?),
        })
    }

    #[staticmethod]
    fn limit_buy(market: &str, quantity: f64, price: f64) -> PyResult<Self> {
        Ok(Self {
            inner: OrderRequest::limit_buy(
                parse_symbol(market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }

    #[staticmethod]
    fn limit_sell(market: &str, quantity: f64, price: f64) -> PyResult<Self> {
        Ok(Self {
            inner: OrderRequest::limit_sell(
                parse_symbol(market)?,
                to_decimal(quantity)?,
                to_decimal(price)?,
            ),
        })
    }
}

/// The concrete client behind a [`PyExchange`].
enum Inner {
    Paper(PaperExchange),
    Replay(ReplayExchange),
    Live(Box<dyn Exchange>),
}

impl Inner {
    fn as_exchange(&mut self) -> &mut dyn Exchange {
        match self {
            Inner::Paper(paper) => paper,
            Inner::Replay(replay) => replay,
            Inner::Live(live) => live.as_mut(),
        }
    }
}

/// A unified exchange client over the synchronous, pull-based API.
///
/// Construct one with [`paper`](PyExchange::paper),
/// [`replay_trades`](PyExchange::replay_trades) or [`connect`](PyExchange::connect);
/// the methods are identical whichever backend you chose.
#[pyclass(name = "Exchange", unsendable)]
pub struct PyExchange {
    inner: Inner,
}

#[pymethods]
impl PyExchange {
    /// An offline paper account seeded from `balances` (asset -> amount), with
    /// optional maker/taker fees and slippage in basis points.
    #[staticmethod]
    #[pyo3(signature = (balances, maker_bps=0.0, taker_bps=0.0, slippage_bps=0.0))]
    fn paper(
        balances: HashMap<String, f64>,
        maker_bps: f64,
        taker_bps: f64,
        slippage_bps: f64,
    ) -> PyResult<Self> {
        let mut paper = PaperExchange::new()
            .with_fees(to_decimal(maker_bps)?, to_decimal(taker_bps)?)
            .with_slippage_bps(to_decimal(slippage_bps)?);
        for (asset, amount) in balances {
            paper = paper.with_balance(asset, to_decimal(amount)?);
        }
        Ok(Self {
            inner: Inner::Paper(paper),
        })
    }

    /// A replay account driven by a recorded price `tape` of `market` trades,
    /// filling against a paper book seeded from `balances`.
    #[staticmethod]
    #[pyo3(signature = (market, tape, balances, maker_bps=0.0, taker_bps=0.0, slippage_bps=0.0))]
    fn replay_trades(
        market: &str,
        tape: Vec<f64>,
        balances: HashMap<String, f64>,
        maker_bps: f64,
        taker_bps: f64,
        slippage_bps: f64,
    ) -> PyResult<Self> {
        let symbol = parse_symbol(market)?;
        let mut paper = PaperExchange::new()
            .with_fees(to_decimal(maker_bps)?, to_decimal(taker_bps)?)
            .with_slippage_bps(to_decimal(slippage_bps)?);
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
    #[staticmethod]
    #[pyo3(signature = (name, credentials, testnet=false))]
    fn connect(name: &str, credentials: &PyCredentials, testnet: bool) -> PyResult<Self> {
        let options = if testnet {
            ExchangeOptions::testnet(MarketType::Spot)
        } else {
            ExchangeOptions::mainnet(MarketType::Spot)
        };
        let live = connect(name, credentials.inner.clone(), &options)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self {
            inner: Inner::Live(live),
        })
    }

    /// The venue's lowercase identifier (`"paper"`, `"replay"`, `"binance"`, ...).
    fn name(&self) -> &'static str {
        match &self.inner {
            Inner::Paper(paper) => paper.name(),
            Inner::Replay(replay) => replay.name(),
            Inner::Live(live) => live.name(),
        }
    }

    /// Set the mark price a paper account fills against (paper backend only).
    fn set_price(&mut self, market: &str, price: f64) -> PyResult<()> {
        match &mut self.inner {
            Inner::Paper(paper) => {
                paper.set_price(&parse_symbol(market)?, to_decimal(price)?);
                Ok(())
            }
            _ => Err(PyValueError::new_err(
                "set_price is only supported on a paper exchange",
            )),
        }
    }

    /// Place an order; returns the resulting order as a dict.
    fn place_order<'py>(
        &mut self,
        py: Python<'py>,
        request: &PyOrderRequest,
    ) -> PyResult<Bound<'py, PyDict>> {
        let order = self
            .inner
            .as_exchange()
            .place_order(&request.inner)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        order_dict(py, &order)
    }

    /// Cancel an open order by venue id.
    fn cancel_order(&mut self, market: &str, order_id: &str) -> PyResult<()> {
        self.inner
            .as_exchange()
            .cancel_order(&parse_symbol(market)?, order_id)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// The current ticker for `market` as a dict.
    fn ticker<'py>(&mut self, py: Python<'py>, market: &str) -> PyResult<Bound<'py, PyDict>> {
        let ticker: Ticker = self
            .inner
            .as_exchange()
            .ticker(&parse_symbol(market)?)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let dict = PyDict::new(py);
        dict.set_item("symbol", ticker.symbol.to_string())?;
        dict.set_item("last", to_float(ticker.last))?;
        dict.set_item("bid", to_float(ticker.bid))?;
        dict.set_item("ask", to_float(ticker.ask))?;
        dict.set_item("volume", to_float(ticker.volume))?;
        Ok(dict)
    }

    /// Account balances as an `asset -> free amount` dict.
    fn balances(&mut self) -> PyResult<HashMap<String, f64>> {
        let balances = self
            .inner
            .as_exchange()
            .balances()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(balances
            .into_iter()
            .map(|b| (b.asset, to_float(b.free)))
            .collect())
    }

    /// Subscribe to the public trade stream for `market`.
    fn subscribe_trades(&mut self, market: &str) -> PyResult<()> {
        self.inner
            .as_exchange()
            .subscribe_trades(&parse_symbol(market)?)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Drain all events buffered since the last call, each as a dict.
    fn poll_events<'py>(&mut self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.inner
            .as_exchange()
            .poll_events()
            .iter()
            .map(|event| event_dict(py, event))
            .collect()
    }
}

/// The `_wickra_exchange` extension module.
#[pymodule]
fn _wickra_exchange(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add_class::<PyCredentials>()?;
    module.add_class::<PyOrderRequest>()?;
    module.add_class::<PyExchange>()?;
    Ok(())
}
