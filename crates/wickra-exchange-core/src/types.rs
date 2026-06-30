//! Order-layer value types.
//!
//! Every price and quantity here is a [`Decimal`], never an `f64`: exchanges
//! reject mis-rounded values, and float drift (scientific notation, `1e-8`
//! error) loses money. Indicator inputs stay `f64` — the boundary is the order
//! layer, which this module defines.

use crate::symbol::Symbol;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The side of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    /// Buy / long.
    Buy,
    /// Sell / short.
    Sell,
}

impl OrderSide {
    /// The opposite side.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            OrderSide::Buy => OrderSide::Sell,
            OrderSide::Sell => OrderSide::Buy,
        }
    }
}

/// The type of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Execute immediately at the best available price.
    Market,
    /// Rest at a limit price.
    Limit,
    /// Trigger a market order when the stop price is reached.
    StopMarket,
    /// Trigger a limit order when the stop price is reached.
    StopLimit,
}

impl OrderType {
    /// Whether this order type requires a limit price.
    #[must_use]
    pub fn requires_price(self) -> bool {
        matches!(self, OrderType::Limit | OrderType::StopLimit)
    }
}

/// How long an order remains active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    /// Good-til-cancelled.
    Gtc,
    /// Immediate-or-cancel: fill what is possible now, cancel the rest.
    Ioc,
    /// Fill-or-kill: fill entirely now or cancel entirely.
    Fok,
}

/// The lifecycle state of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Accepted, not yet filled.
    New,
    /// Partially filled, still resting.
    PartiallyFilled,
    /// Fully filled.
    Filled,
    /// Cancelled before fully filling.
    Canceled,
    /// Rejected by the venue.
    Rejected,
    /// Expired (e.g. an IOC remainder).
    Expired,
}

impl OrderStatus {
    /// Whether the order is still working on the book (`New` or `PartiallyFilled`).
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, OrderStatus::New | OrderStatus::PartiallyFilled)
    }
}

/// A request to place an order. Built with the convenience constructors and
/// refined with the builder methods; quantities and prices are exact `Decimal`s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRequest {
    /// The market to trade.
    pub symbol: Symbol,
    /// Buy or sell.
    pub side: OrderSide,
    /// Order type.
    pub order_type: OrderType,
    /// Order quantity in base units.
    pub quantity: Decimal,
    /// Limit price (required for limit / stop-limit orders).
    pub price: Option<Decimal>,
    /// Trigger price (required for stop orders).
    pub stop_price: Option<Decimal>,
    /// Time-in-force policy.
    pub time_in_force: TimeInForce,
    /// Optional client-supplied id for idempotent placement and reconciliation.
    pub client_order_id: Option<String>,
    /// Only reduce an existing position; never open or increase one.
    pub reduce_only: bool,
    /// Only rest as a maker; reject if it would take liquidity.
    pub post_only: bool,
}

impl OrderRequest {
    fn bare(symbol: Symbol, side: OrderSide, order_type: OrderType, quantity: Decimal) -> Self {
        Self {
            symbol,
            side,
            order_type,
            quantity,
            price: None,
            stop_price: None,
            time_in_force: TimeInForce::Gtc,
            client_order_id: None,
            reduce_only: false,
            post_only: false,
        }
    }

    /// A market buy of `quantity` base units.
    #[must_use]
    pub fn market_buy(symbol: Symbol, quantity: Decimal) -> Self {
        Self::bare(symbol, OrderSide::Buy, OrderType::Market, quantity)
    }

    /// A market sell of `quantity` base units.
    #[must_use]
    pub fn market_sell(symbol: Symbol, quantity: Decimal) -> Self {
        Self::bare(symbol, OrderSide::Sell, OrderType::Market, quantity)
    }

    /// A limit buy of `quantity` base units at `price`.
    #[must_use]
    pub fn limit_buy(symbol: Symbol, quantity: Decimal, price: Decimal) -> Self {
        Self {
            price: Some(price),
            ..Self::bare(symbol, OrderSide::Buy, OrderType::Limit, quantity)
        }
    }

    /// A limit sell of `quantity` base units at `price`.
    #[must_use]
    pub fn limit_sell(symbol: Symbol, quantity: Decimal, price: Decimal) -> Self {
        Self {
            price: Some(price),
            ..Self::bare(symbol, OrderSide::Sell, OrderType::Limit, quantity)
        }
    }

    /// Set the time-in-force policy.
    #[must_use]
    pub fn with_time_in_force(mut self, tif: TimeInForce) -> Self {
        self.time_in_force = tif;
        self
    }

    /// Attach a client order id.
    #[must_use]
    pub fn with_client_order_id(mut self, id: impl Into<String>) -> Self {
        self.client_order_id = Some(id.into());
        self
    }

    /// Mark the order reduce-only.
    #[must_use]
    pub fn reduce_only(mut self) -> Self {
        self.reduce_only = true;
        self
    }

    /// Mark the order post-only.
    #[must_use]
    pub fn post_only(mut self) -> Self {
        self.post_only = true;
        self
    }

    /// Validate the request's internal consistency, before any venue filter is
    /// applied: a strictly positive quantity, a price where the type needs one,
    /// and a stop price for stop orders.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOrder`](crate::Error::InvalidOrder) when an
    /// invariant is violated.
    pub fn validate(&self) -> crate::Result<()> {
        use crate::Error;
        if self.quantity <= Decimal::ZERO {
            return Err(Error::InvalidOrder("quantity must be positive"));
        }
        if self.order_type.requires_price() {
            match self.price {
                Some(p) if p > Decimal::ZERO => {}
                Some(_) => return Err(Error::InvalidOrder("price must be positive")),
                None => return Err(Error::InvalidOrder("limit order requires a price")),
            }
        }
        if matches!(
            self.order_type,
            OrderType::StopMarket | OrderType::StopLimit
        ) && self.stop_price.is_none()
        {
            return Err(Error::InvalidOrder("stop order requires a stop price"));
        }
        Ok(())
    }
}

/// An order as reported by the venue after placement or on query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// The venue's order id.
    pub id: String,
    /// The client order id, if one was supplied.
    pub client_order_id: Option<String>,
    /// The market.
    pub symbol: Symbol,
    /// Buy or sell.
    pub side: OrderSide,
    /// Order type.
    pub order_type: OrderType,
    /// Current lifecycle state.
    pub status: OrderStatus,
    /// Total ordered quantity.
    pub quantity: Decimal,
    /// Quantity filled so far.
    pub filled_quantity: Decimal,
    /// Limit price, if any.
    pub price: Option<Decimal>,
    /// Volume-weighted average fill price, if any fills occurred.
    pub average_price: Option<Decimal>,
}

impl Order {
    /// The quantity still to be filled.
    #[must_use]
    pub fn remaining_quantity(&self) -> Decimal {
        self.quantity - self.filled_quantity
    }

    /// Whether the order is still working on the book.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.status.is_open()
    }
}

/// A balance for a single asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    /// The asset (e.g. `USDT`).
    pub asset: String,
    /// Freely available quantity.
    pub free: Decimal,
    /// Quantity locked in open orders or positions.
    pub locked: Decimal,
}

impl Balance {
    /// Free plus locked.
    #[must_use]
    pub fn total(&self) -> Decimal {
        self.free + self.locked
    }
}

/// A point-in-time price summary for a market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticker {
    /// The market.
    pub symbol: Symbol,
    /// Last traded price.
    pub last: Decimal,
    /// Best bid price.
    pub bid: Decimal,
    /// Best ask price.
    pub ask: Decimal,
    /// Rolling base-asset volume.
    pub volume: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn btcusdt() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    #[test]
    fn side_and_type_helpers() {
        assert_eq!(OrderSide::Buy.opposite(), OrderSide::Sell);
        assert_eq!(OrderSide::Sell.opposite(), OrderSide::Buy);
        assert!(OrderType::Limit.requires_price());
        assert!(OrderType::StopLimit.requires_price());
        assert!(!OrderType::Market.requires_price());
        assert!(!OrderType::StopMarket.requires_price());
    }

    #[test]
    fn status_open_classification() {
        assert!(OrderStatus::New.is_open());
        assert!(OrderStatus::PartiallyFilled.is_open());
        assert!(!OrderStatus::Filled.is_open());
        assert!(!OrderStatus::Canceled.is_open());
        assert!(!OrderStatus::Rejected.is_open());
        assert!(!OrderStatus::Expired.is_open());
    }

    #[test]
    fn constructors_set_side_type_and_price() {
        let mb = OrderRequest::market_buy(btcusdt(), dec!(0.5));
        assert_eq!(mb.side, OrderSide::Buy);
        assert_eq!(mb.order_type, OrderType::Market);
        assert!(mb.price.is_none());

        let ms = OrderRequest::market_sell(btcusdt(), dec!(0.5));
        assert_eq!(ms.side, OrderSide::Sell);

        let lb = OrderRequest::limit_buy(btcusdt(), dec!(0.001), dec!(20000));
        assert_eq!(lb.side, OrderSide::Buy);
        assert_eq!(lb.order_type, OrderType::Limit);
        assert_eq!(lb.price, Some(dec!(20000)));

        let ls = OrderRequest::limit_sell(btcusdt(), dec!(0.001), dec!(30000));
        assert_eq!(ls.side, OrderSide::Sell);
        assert_eq!(ls.price, Some(dec!(30000)));
    }

    #[test]
    fn builder_flags_and_fields() {
        let req = OrderRequest::limit_buy(btcusdt(), dec!(1), dec!(100))
            .with_time_in_force(TimeInForce::Ioc)
            .with_client_order_id("abc-123")
            .reduce_only()
            .post_only();
        assert_eq!(req.time_in_force, TimeInForce::Ioc);
        assert_eq!(req.client_order_id.as_deref(), Some("abc-123"));
        assert!(req.reduce_only);
        assert!(req.post_only);
    }

    #[test]
    fn validate_catches_bad_requests() {
        use crate::Error;
        // Good.
        assert!(OrderRequest::limit_buy(btcusdt(), dec!(1), dec!(100))
            .validate()
            .is_ok());
        assert!(OrderRequest::market_buy(btcusdt(), dec!(1))
            .validate()
            .is_ok());

        // Non-positive quantity.
        assert_eq!(
            OrderRequest::market_buy(btcusdt(), dec!(0))
                .validate()
                .unwrap_err(),
            Error::InvalidOrder("quantity must be positive")
        );

        // Limit with no price.
        let mut req = OrderRequest::limit_buy(btcusdt(), dec!(1), dec!(100));
        req.price = None;
        assert_eq!(
            req.validate().unwrap_err(),
            Error::InvalidOrder("limit order requires a price")
        );

        // Limit with non-positive price.
        let mut req = OrderRequest::limit_buy(btcusdt(), dec!(1), dec!(100));
        req.price = Some(dec!(0));
        assert_eq!(
            req.validate().unwrap_err(),
            Error::InvalidOrder("price must be positive")
        );

        // Stop without a stop price.
        let mut req = OrderRequest::market_buy(btcusdt(), dec!(1));
        req.order_type = OrderType::StopMarket;
        assert_eq!(
            req.validate().unwrap_err(),
            Error::InvalidOrder("stop order requires a stop price")
        );
    }

    #[test]
    fn order_remaining_and_open() {
        let order = Order {
            id: "1".into(),
            client_order_id: None,
            symbol: btcusdt(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            status: OrderStatus::PartiallyFilled,
            quantity: dec!(2),
            filled_quantity: dec!(0.5),
            price: Some(dec!(100)),
            average_price: Some(dec!(100)),
        };
        assert_eq!(order.remaining_quantity(), dec!(1.5));
        assert!(order.is_open());
    }

    #[test]
    fn balance_total() {
        let bal = Balance {
            asset: "USDT".into(),
            free: dec!(100.5),
            locked: dec!(25.25),
        };
        assert_eq!(bal.total(), dec!(125.75));
    }

    #[test]
    fn order_request_round_trips_through_json() {
        let req =
            OrderRequest::limit_buy(btcusdt(), dec!(0.001), dec!(20000)).with_client_order_id("x");
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<OrderRequest>(&json).unwrap(), req);
    }
}
