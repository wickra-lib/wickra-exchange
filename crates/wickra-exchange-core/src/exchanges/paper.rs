//! A deterministic paper-trading exchange.
//!
//! [`PaperExchange`] implements the very same [`Exchange`] trait as every live
//! venue, but fills orders against an internal book and portfolio instead of a
//! network. This is the library's headline differentiator: a strategy written
//! against [`Exchange`] runs **paper ↔ live by swapping the implementation** —
//! not a line of strategy code changes.
//!
//! ## Fill model
//!
//! The fill model mirrors the cost semantics of the Wickra backtest engine
//! (`wickra-backtest-core`, `spec::Costs` / `engine::slippage_rate`) so that a
//! paper run and a backtest agree on execution:
//!
//! - **Fixed-bps slippage.** A market (or marketable-limit) fill executes at
//!   `mark * (1 + dir * slippage)`, where `dir` is `+1` for a buy and `-1` for a
//!   sell — a buy pays up, a sell gives up, exactly as the engine models it.
//! - **Maker / taker fees in basis points.** A fill pays `notional * rate`,
//!   charged in the quote asset; immediate fills pay the taker rate.
//!
//! The engine's own fill routine is a bar-loop (orders decided on a close, filled
//! on the next open) with no public single-order entry point, so the arithmetic
//! is reimplemented here against the order-driven [`Execution`] API rather than
//! taken as a dependency.
//!
//! ## Market data
//!
//! A paper exchange has no feed of its own: prices are injected with
//! [`set_price`](PaperExchange::set_price). [`ticker`](MarketData::ticker) reports
//! the current mark; historical [`klines`](MarketData::klines) and depth
//! [`order_book`](MarketData::order_book) are unsupported — pair the paper
//! exchange with a live or replay market-data feed. Execution events (order and
//! balance updates) flow through [`poll_events`](MarketData::poll_events).

use std::collections::{BTreeMap, HashMap, VecDeque};

use rust_decimal::Decimal;
use wickra_core::Candle;

use crate::error::{Error, Result};
use crate::events::{Event, OrderBookSnapshot};
use crate::options::MarketType;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::types::{Balance, Order, OrderRequest, OrderSide, OrderStatus, OrderType, Ticker};

/// Basis points as a fraction: `bps / 10_000`.
fn bps_fraction(bps: Decimal) -> Decimal {
    bps / Decimal::from(10_000)
}

/// A deterministic, network-free exchange that simulates fills through an
/// internal portfolio. See the [module docs](self) for the fill model.
pub struct PaperExchange {
    market_type: MarketType,
    maker_bps: Decimal,
    taker_bps: Decimal,
    slippage_bps: Decimal,
    /// Current mark price per symbol, injected via [`set_price`](Self::set_price).
    marks: HashMap<Symbol, Decimal>,
    /// Free/locked balance per asset.
    balances: BTreeMap<String, Balance>,
    /// Every order ever placed, keyed by venue id; open orders are those whose
    /// status [`is_open`](OrderStatus::is_open).
    orders: BTreeMap<String, Order>,
    /// Pending execution events drained by [`poll_events`](Self::poll_events).
    events: VecDeque<Event>,
    next_id: u64,
}

impl Default for PaperExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl PaperExchange {
    /// A frictionless spot paper exchange (zero fees, zero slippage). Add costs
    /// with [`with_fees`](Self::with_fees) / [`with_slippage_bps`](Self::with_slippage_bps)
    /// and seed the account with [`with_balance`](Self::with_balance).
    #[must_use]
    pub fn new() -> Self {
        Self {
            market_type: MarketType::Spot,
            maker_bps: Decimal::ZERO,
            taker_bps: Decimal::ZERO,
            slippage_bps: Decimal::ZERO,
            marks: HashMap::new(),
            balances: BTreeMap::new(),
            orders: BTreeMap::new(),
            events: VecDeque::new(),
            next_id: 0,
        }
    }

    /// Set the market type this paper account trades (default [`MarketType::Spot`]).
    #[must_use]
    pub fn with_market_type(mut self, market_type: MarketType) -> Self {
        self.market_type = market_type;
        self
    }

    /// Set the maker and taker fees, in basis points.
    #[must_use]
    pub fn with_fees(mut self, maker_bps: Decimal, taker_bps: Decimal) -> Self {
        self.maker_bps = maker_bps;
        self.taker_bps = taker_bps;
        self
    }

    /// Set the fixed slippage applied to every fill, in basis points.
    #[must_use]
    pub fn with_slippage_bps(mut self, slippage_bps: Decimal) -> Self {
        self.slippage_bps = slippage_bps;
        self
    }

    /// Seed (or top up) the free balance of `asset`.
    #[must_use]
    pub fn with_balance(mut self, asset: impl Into<String>, amount: Decimal) -> Self {
        let asset = asset.into();
        let entry = self.balances.entry(asset.clone()).or_insert(Balance {
            asset,
            free: Decimal::ZERO,
            locked: Decimal::ZERO,
        });
        entry.free += amount;
        self
    }

    /// The market type this account trades.
    #[must_use]
    pub fn market_type(&self) -> MarketType {
        self.market_type
    }

    /// Set the current mark price for `symbol`, used for fills and the ticker.
    pub fn set_price(&mut self, symbol: &Symbol, price: Decimal) {
        self.marks.insert(symbol.clone(), price);
    }

    /// Cancel every open order, releasing the funds they locked, and return the
    /// number cancelled. This is the actuator a
    /// [`DeadMansSwitch`](crate::DeadMansSwitch) fires on disconnect.
    pub fn cancel_all(&mut self) -> u32 {
        let open: Vec<(Symbol, String)> = self
            .orders
            .values()
            .filter(|order| order.is_open())
            .map(|order| (order.symbol.clone(), order.id.clone()))
            .collect();
        let mut cancelled = 0;
        for (symbol, id) in open {
            if self.cancel_order(&symbol, &id).is_ok() {
                cancelled += 1;
            }
        }
        cancelled
    }

    /// The configured maker fee, in basis points.
    #[must_use]
    pub fn maker_bps(&self) -> Decimal {
        self.maker_bps
    }

    /// The configured taker fee, in basis points.
    #[must_use]
    pub fn taker_bps(&self) -> Decimal {
        self.taker_bps
    }

    fn next_order_id(&mut self) -> String {
        self.next_id += 1;
        format!("paper-{}", self.next_id)
    }

    fn balance_mut(&mut self, asset: &str) -> &mut Balance {
        self.balances
            .entry(asset.to_string())
            .or_insert_with(|| Balance {
                asset: asset.to_string(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            })
    }

    fn free_of(&self, asset: &str) -> Decimal {
        self.balances.get(asset).map_or(Decimal::ZERO, |b| b.free)
    }

    fn balance_snapshot(&self) -> Vec<Balance> {
        self.balances.values().cloned().collect()
    }

    /// The fill price for a `side` order against `mark`, after fixed-bps slippage:
    /// a buy fills higher, a sell lower.
    fn fill_price(&self, side: OrderSide, mark: Decimal) -> Decimal {
        let slip = bps_fraction(self.slippage_bps);
        match side {
            OrderSide::Buy => mark * (Decimal::ONE + slip),
            OrderSide::Sell => mark * (Decimal::ONE - slip),
        }
    }

    /// Settle a fill against the portfolio and record the order. Assumes the
    /// pre-flight balance check has already passed.
    fn settle_fill(&mut self, request: &OrderRequest, fill_price: Decimal) -> Order {
        let base = request.symbol.base().to_string();
        let quote = request.symbol.quote().to_string();
        let qty = request.quantity;
        let notional = qty * fill_price;
        let fee = notional * bps_fraction(self.taker_bps);

        match request.side {
            OrderSide::Buy => {
                self.balance_mut(&quote).free -= notional + fee;
                self.balance_mut(&base).free += qty;
            }
            OrderSide::Sell => {
                self.balance_mut(&base).free -= qty;
                self.balance_mut(&quote).free += notional - fee;
            }
        }

        let id = self.next_order_id();
        let order = Order {
            id: id.clone(),
            client_order_id: request.client_order_id.clone(),
            symbol: request.symbol.clone(),
            side: request.side,
            order_type: request.order_type,
            status: OrderStatus::Filled,
            quantity: qty,
            filled_quantity: qty,
            price: request.price,
            average_price: Some(fill_price),
        };
        self.orders.insert(id, order.clone());
        self.events.push_back(Event::OrderUpdate(order.clone()));
        self.events
            .push_back(Event::BalanceUpdate(self.balance_snapshot()));
        order
    }

    /// Rest a non-marketable limit order, locking the funds it reserves.
    fn rest_order(&mut self, request: &OrderRequest, price: Decimal) -> Order {
        let (asset, amount) = match request.side {
            OrderSide::Buy => (request.symbol.quote().to_string(), request.quantity * price),
            OrderSide::Sell => (request.symbol.base().to_string(), request.quantity),
        };
        let balance = self.balance_mut(&asset);
        balance.free -= amount;
        balance.locked += amount;

        let id = self.next_order_id();
        let order = Order {
            id: id.clone(),
            client_order_id: request.client_order_id.clone(),
            symbol: request.symbol.clone(),
            side: request.side,
            order_type: request.order_type,
            status: OrderStatus::New,
            quantity: request.quantity,
            filled_quantity: Decimal::ZERO,
            price: Some(price),
            average_price: None,
        };
        self.orders.insert(id, order.clone());
        self.events.push_back(Event::OrderUpdate(order.clone()));
        order
    }
}

impl MarketData for PaperExchange {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        let mark = self
            .marks
            .get(symbol)
            .copied()
            .ok_or_else(|| Error::NotFound(format!("no paper price set for {symbol}")))?;
        Ok(Ticker {
            symbol: symbol.clone(),
            last: mark,
            bid: mark,
            ask: mark,
            volume: Decimal::ZERO,
        })
    }

    fn klines(&mut self, _symbol: &Symbol, _interval: &str, _limit: u32) -> Result<Vec<Candle>> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "paper exchange has no historical data; pair it with a live or replay feed"
                .to_string(),
        })
    }

    fn order_book(&mut self, _symbol: &Symbol, _depth: u32) -> Result<OrderBookSnapshot> {
        Err(Error::Exchange {
            code: "unsupported".to_string(),
            message: "paper exchange has no depth feed; pair it with a live or replay feed"
                .to_string(),
        })
    }

    fn subscribe_trades(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn subscribe_book(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn subscribe_ticker(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }
}

impl Execution for PaperExchange {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        request.validate()?;
        if matches!(
            request.order_type,
            OrderType::StopMarket | OrderType::StopLimit
        ) {
            return Err(Error::InvalidOrder(
                "paper exchange supports only market and limit orders",
            ));
        }
        let mark =
            self.marks.get(&request.symbol).copied().ok_or_else(|| {
                Error::NotFound(format!("no paper price set for {}", request.symbol))
            })?;

        // A limit order fills now only if it crosses the mark; otherwise it rests.
        // Stop types are rejected above, so they never reach this match; they are
        // grouped with the live arms only to keep it exhaustive without a catch-all.
        let crosses = match request.order_type {
            OrderType::Market | OrderType::StopMarket => true,
            OrderType::Limit | OrderType::StopLimit => match request.side {
                OrderSide::Buy => request.price.unwrap_or(mark) >= mark,
                OrderSide::Sell => request.price.unwrap_or(mark) <= mark,
            },
        };

        if !crosses {
            // A resting limit: check the funds it must lock, then rest it.
            let price = request.price.unwrap_or(mark);
            let (asset, amount) = match request.side {
                OrderSide::Buy => (request.symbol.quote(), request.quantity * price),
                OrderSide::Sell => (request.symbol.base(), request.quantity),
            };
            if self.free_of(asset) < amount {
                return Err(Error::InsufficientBalance);
            }
            return Ok(self.rest_order(request, price));
        }

        if request.post_only {
            return Err(Error::OrderRejected {
                code: "post_only".to_string(),
                message: "post-only order would take liquidity".to_string(),
            });
        }

        let fill_price = self.fill_price(request.side, mark);
        let notional = request.quantity * fill_price;
        let fee = notional * bps_fraction(self.taker_bps);

        // Pre-flight balance check: buys spend quote (+ fee), sells spend base.
        match request.side {
            OrderSide::Buy => {
                if self.free_of(request.symbol.quote()) < notional + fee {
                    return Err(Error::InsufficientBalance);
                }
            }
            OrderSide::Sell => {
                if self.free_of(request.symbol.base()) < request.quantity {
                    return Err(Error::InsufficientBalance);
                }
            }
        }

        // A marketable order fills in full immediately, so fill-or-kill and
        // immediate-or-cancel are both satisfied by this single fill.
        Ok(self.settle_fill(request, fill_price))
    }

    fn cancel_order(&mut self, _symbol: &Symbol, order_id: &str) -> Result<()> {
        let order = self
            .orders
            .get_mut(order_id)
            .filter(|o| o.is_open())
            .ok_or_else(|| Error::NotFound(format!("no open paper order {order_id}")))?;
        order.status = OrderStatus::Canceled;
        let order = order.clone();

        // Release the funds the resting order had locked.
        let price = order.price.unwrap_or_default();
        let (asset, amount) = match order.side {
            OrderSide::Buy => (order.symbol.quote().to_string(), order.quantity * price),
            OrderSide::Sell => (order.symbol.base().to_string(), order.quantity),
        };
        let balance = self.balance_mut(&asset);
        balance.locked -= amount;
        balance.free += amount;

        self.events.push_back(Event::OrderUpdate(order));
        self.events
            .push_back(Event::BalanceUpdate(self.balance_snapshot()));
        Ok(())
    }

    fn query_order(&mut self, _symbol: &Symbol, order_id: &str) -> Result<Order> {
        self.orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("no paper order {order_id}")))
    }

    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        Ok(self
            .orders
            .values()
            .filter(|o| o.is_open())
            .filter(|o| symbol.is_none_or(|s| &o.symbol == s))
            .cloned()
            .collect())
    }

    fn balances(&mut self) -> Result<Vec<Balance>> {
        Ok(self.balance_snapshot())
    }
}

impl Exchange for PaperExchange {
    fn name(&self) -> &'static str {
        "paper"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn sym() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    fn seeded() -> PaperExchange {
        let mut ex = PaperExchange::new()
            .with_fees(dec!(1), dec!(5))
            .with_slippage_bps(dec!(10))
            .with_balance("USDT", dec!(100000))
            .with_balance("BTC", dec!(2));
        ex.set_price(&sym(), dec!(20000));
        ex
    }

    #[test]
    fn market_buy_fills_with_slippage_and_fee() {
        let mut ex = seeded();
        let order = ex
            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
        assert_eq!(order.average_price, Some(dec!(20020)));
        assert_eq!(order.filled_quantity, dec!(1));

        // Quote spent = 20020 notional + 5 bps fee (10.01) = 20030.01.
        let usdt = ex
            .balances()
            .unwrap()
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();
        assert_eq!(usdt.free, dec!(100000) - dec!(20020) - dec!(10.01));
        let btc = ex
            .balances()
            .unwrap()
            .into_iter()
            .find(|b| b.asset == "BTC")
            .unwrap();
        assert_eq!(btc.free, dec!(3));
    }

    #[test]
    fn market_sell_credits_quote_net_of_fee() {
        let mut ex = seeded();
        ex.place_order(&OrderRequest::market_sell(sym(), dec!(1)))
            .unwrap();
        // 10 bps slippage on a sell: 20000 * 0.999 = 19980; fee 5 bps = 9.99.
        let usdt = ex.free_of("USDT");
        assert_eq!(usdt, dec!(100000) + dec!(19980) - dec!(9.99));
        assert_eq!(ex.free_of("BTC"), dec!(1));
    }

    #[test]
    fn frictionless_fill_matches_the_mark_exactly() {
        let mut ex = PaperExchange::new().with_balance("USDT", dec!(50000));
        ex.set_price(&sym(), dec!(20000));
        let order = ex
            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap();
        assert_eq!(order.average_price, Some(dec!(20000)));
        assert_eq!(ex.free_of("USDT"), dec!(30000));
    }

    #[test]
    fn poll_events_emits_order_then_balance_update() {
        let mut ex = seeded();
        ex.place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap();
        let events = ex.poll_events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::OrderUpdate(_)));
        assert!(matches!(events[1], Event::BalanceUpdate(_)));
        // Draining clears the buffer.
        assert!(ex.poll_events().is_empty());
    }

    #[test]
    fn resting_limit_locks_funds_and_is_cancelable() {
        let mut ex = seeded();
        // Buy limit below the mark rests instead of filling.
        let order = ex
            .place_order(&OrderRequest::limit_buy(sym(), dec!(1), dec!(19000)))
            .unwrap();
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(ex.free_of("USDT"), dec!(100000) - dec!(19000));
        assert_eq!(ex.open_orders(None).unwrap().len(), 1);

        ex.cancel_order(&sym(), &order.id).unwrap();
        assert_eq!(ex.free_of("USDT"), dec!(100000));
        assert!(ex.open_orders(None).unwrap().is_empty());
        assert_eq!(
            ex.query_order(&sym(), &order.id).unwrap().status,
            OrderStatus::Canceled
        );
    }

    #[test]
    fn dead_mans_switch_fires_cancel_all_on_lost_heartbeat() {
        use crate::DeadMansSwitch;
        use std::time::Duration;

        let mut ex = seeded();
        // Two resting orders on opposite sides of the mark (20000).
        ex.place_order(&OrderRequest::limit_buy(sym(), dec!(1), dec!(19000)))
            .unwrap();
        ex.place_order(&OrderRequest::limit_sell(sym(), dec!(1), dec!(21000)))
            .unwrap();
        assert_eq!(ex.open_orders(None).unwrap().len(), 2);

        let mut switch = DeadMansSwitch::new(Duration::from_secs(5));
        switch.heartbeat(1_000);
        assert!(!switch.is_expired(3_000)); // still in contact

        // The heartbeat is lost: the switch trips and the actuator cancels all.
        assert!(switch.is_expired(6_000));
        let cancelled = ex.cancel_all();
        assert_eq!(cancelled, 2);
        assert!(ex.open_orders(None).unwrap().is_empty());
        // Locked funds are released back to free on both sides.
        assert_eq!(ex.free_of("USDT"), dec!(100000));
        assert_eq!(ex.free_of("BTC"), dec!(2));
    }

    #[test]
    fn marketable_limit_buy_fills_immediately() {
        let mut ex = seeded();
        // Buy limit above the mark crosses and fills.
        let order = ex
            .place_order(&OrderRequest::limit_buy(sym(), dec!(1), dec!(21000)))
            .unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.average_price, Some(dec!(20020)));
    }

    #[test]
    fn post_only_crossing_order_is_rejected() {
        let mut ex = seeded();
        let err = ex
            .place_order(&OrderRequest::limit_buy(sym(), dec!(1), dec!(21000)).post_only())
            .unwrap_err();
        assert!(matches!(err, Error::OrderRejected { .. }));
    }

    #[test]
    fn insufficient_balance_is_rejected() {
        let mut ex = PaperExchange::new().with_balance("USDT", dec!(100));
        ex.set_price(&sym(), dec!(20000));
        let err = ex
            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap_err();
        assert!(matches!(err, Error::InsufficientBalance));
    }

    #[test]
    fn insufficient_base_rejects_a_sell() {
        let mut ex = PaperExchange::new().with_balance("BTC", dec!(0.5));
        ex.set_price(&sym(), dec!(20000));
        let err = ex
            .place_order(&OrderRequest::market_sell(sym(), dec!(1)))
            .unwrap_err();
        assert!(matches!(err, Error::InsufficientBalance));
    }

    #[test]
    fn stop_orders_are_unsupported() {
        let mut ex = seeded();
        let request = OrderRequest {
            order_type: OrderType::StopMarket,
            stop_price: Some(dec!(19000)),
            ..OrderRequest::market_sell(sym(), dec!(1))
        };
        assert!(matches!(
            ex.place_order(&request).unwrap_err(),
            Error::InvalidOrder(_)
        ));
    }

    #[test]
    fn order_without_a_price_needs_a_mark() {
        let mut ex = PaperExchange::new().with_balance("USDT", dec!(50000));
        let err = ex
            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn ticker_reports_the_mark_and_klines_are_unsupported() {
        let mut ex = seeded();
        let ticker = ex.ticker(&sym()).unwrap();
        assert_eq!(ticker.last, dec!(20000));
        assert_eq!(ticker.bid, ticker.ask);
        assert!(ex.klines(&sym(), "1m", 10).is_err());
        assert!(ex.order_book(&sym(), 10).is_err());
        assert!(ex.ticker(&Symbol::new("ETH", "USDT")).is_err());
    }

    #[test]
    fn subscriptions_are_inert_and_name_is_paper() {
        let mut ex = seeded();
        assert!(ex.subscribe_trades(&sym()).is_ok());
        assert!(ex.subscribe_book(&sym()).is_ok());
        assert!(ex.subscribe_ticker(&sym()).is_ok());
        assert!(ex.poll_events().is_empty());
        assert_eq!(ex.name(), "paper");
        assert_eq!(ex.market_type(), MarketType::Spot);
    }

    #[test]
    fn cancel_unknown_order_is_not_found() {
        let mut ex = seeded();
        assert!(matches!(
            ex.cancel_order(&sym(), "paper-999").unwrap_err(),
            Error::NotFound(_)
        ));
        assert!(matches!(
            ex.query_order(&sym(), "paper-999").unwrap_err(),
            Error::NotFound(_)
        ));
    }

    #[test]
    fn builder_configuration_is_readable() {
        let ex = PaperExchange::default()
            .with_market_type(MarketType::UsdMFutures)
            .with_fees(dec!(2), dec!(7));
        assert_eq!(ex.market_type(), MarketType::UsdMFutures);
        assert_eq!(ex.maker_bps(), dec!(2));
        assert_eq!(ex.taker_bps(), dec!(7));
    }
}
