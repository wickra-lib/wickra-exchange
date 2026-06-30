//! Market-data and account streaming types.
//!
//! These are the items a pull-based stream yields from [`poll_events`]. Prices
//! and quantities are exact [`Decimal`]s; the order-book types carry a sequence
//! id so the local builder can apply diffs and detect gaps. They are all
//! `serde`-serializable, which is what the replay-parity corpus pins across the
//! language bindings.
//!
//! [`poll_events`]: crate::MarketData::poll_events

use crate::symbol::Symbol;
use crate::types::{Balance, Order, OrderSide, Ticker};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A single resting price level (exchange-precision `Decimal`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookLevel {
    /// Price of the level.
    pub price: Decimal,
    /// Resting quantity at this price. A quantity of zero in a [`BookDelta`]
    /// means the level was removed.
    pub quantity: Decimal,
}

impl BookLevel {
    /// Construct a level.
    #[must_use]
    pub fn new(price: Decimal, quantity: Decimal) -> Self {
        Self { price, quantity }
    }
}

/// A depth snapshot with exact levels, best-first on each side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderBookSnapshot {
    /// The market.
    pub symbol: Symbol,
    /// The venue's sequence id for this snapshot (for diff alignment).
    pub last_update_id: u64,
    /// Bid levels, best (highest price) first.
    pub bids: Vec<BookLevel>,
    /// Ask levels, best (lowest price) first.
    pub asks: Vec<BookLevel>,
}

impl OrderBookSnapshot {
    /// The best (highest-price) bid, or `None` if the bid side is empty.
    #[must_use]
    pub fn best_bid(&self) -> Option<&BookLevel> {
        self.bids.first()
    }

    /// The best (lowest-price) ask, or `None` if the ask side is empty.
    #[must_use]
    pub fn best_ask(&self) -> Option<&BookLevel> {
        self.asks.first()
    }

    /// The mid price `(best_bid + best_ask) / 2`, or `None` if either side is empty.
    #[must_use]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid.price + ask.price) / Decimal::from(2)),
            _ => None,
        }
    }

    /// The bid/ask spread `best_ask - best_bid`, or `None` if either side is empty.
    #[must_use]
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }
}

/// A depth diff to apply to a locally maintained order book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookDelta {
    /// The market.
    pub symbol: Symbol,
    /// The first sequence id covered by this diff.
    pub first_update_id: u64,
    /// The last sequence id covered by this diff.
    pub final_update_id: u64,
    /// Changed bid levels (quantity zero = remove).
    pub bids: Vec<BookLevel>,
    /// Changed ask levels (quantity zero = remove).
    pub asks: Vec<BookLevel>,
}

/// A single executed public trade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradePrint {
    /// The market.
    pub symbol: Symbol,
    /// Execution price.
    pub price: Decimal,
    /// Executed quantity.
    pub quantity: Decimal,
    /// The taker (aggressor) side.
    pub aggressor: OrderSide,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// An item yielded by a pull-based stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A public trade print.
    Trade(TradePrint),
    /// A ticker update.
    Ticker(Ticker),
    /// A full order-book snapshot (e.g. after subscribe or resync).
    BookSnapshot(OrderBookSnapshot),
    /// An incremental order-book diff.
    BookDelta(BookDelta),
    /// An update to one of the account's orders (user-data stream).
    OrderUpdate(Order),
    /// An account balance update (user-data stream).
    BalanceUpdate(Vec<Balance>),
    /// A subscription was acknowledged by the venue.
    Subscribed {
        /// The channel that was subscribed.
        channel: String,
    },
    /// The stream disconnected; the client will attempt to reconnect.
    Disconnected,
    /// The stream reconnected and resubscribed.
    Reconnected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn book() -> OrderBookSnapshot {
        OrderBookSnapshot {
            symbol: Symbol::new("BTC", "USDT"),
            last_update_id: 42,
            bids: vec![
                BookLevel::new(dec!(100), dec!(1)),
                BookLevel::new(dec!(99), dec!(2)),
            ],
            asks: vec![
                BookLevel::new(dec!(101), dec!(1)),
                BookLevel::new(dec!(102), dec!(3)),
            ],
        }
    }

    #[test]
    fn book_best_levels_mid_and_spread() {
        let b = book();
        assert_eq!(b.best_bid(), Some(&BookLevel::new(dec!(100), dec!(1))));
        assert_eq!(b.best_ask(), Some(&BookLevel::new(dec!(101), dec!(1))));
        assert_eq!(b.mid_price(), Some(dec!(100.5)));
        assert_eq!(b.spread(), Some(dec!(1)));
    }

    #[test]
    fn empty_sides_yield_no_mid_or_spread() {
        let empty = OrderBookSnapshot {
            symbol: Symbol::new("BTC", "USDT"),
            last_update_id: 0,
            bids: vec![],
            asks: vec![],
        };
        assert!(empty.best_bid().is_none());
        assert!(empty.best_ask().is_none());
        assert!(empty.mid_price().is_none());
        assert!(empty.spread().is_none());

        let bid_only = OrderBookSnapshot {
            asks: vec![],
            ..book()
        };
        assert!(bid_only.mid_price().is_none());
        assert!(bid_only.spread().is_none());
    }

    #[test]
    fn event_is_tagged_in_json() {
        let event = Event::Trade(TradePrint {
            symbol: Symbol::new("BTC", "USDT"),
            price: dec!(20000),
            quantity: dec!(0.5),
            aggressor: OrderSide::Buy,
            timestamp: 1_700_000_000_000,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"trade\""));
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn lifecycle_events_round_trip() {
        for event in [Event::Disconnected, Event::Reconnected] {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
        }
        let sub = Event::Subscribed {
            channel: "btcusdt@trade".into(),
        };
        let json = serde_json::to_string(&sub).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), sub);
    }
}
