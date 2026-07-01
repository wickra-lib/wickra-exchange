//! Typed derivatives feeds and zero-glue conversions into the wickra-core
//! indicator input types.
//!
//! An exchange publishes microstructure over several channels — trades, depth,
//! funding, open interest, liquidations, positioning. This module gives each a
//! typed representation and a **direct conversion into the exact value type the
//! wickra-core indicator families consume**, so a recorded or live frame feeds an
//! indicator with no hand-written glue:
//!
//! | Source (this crate)                     | Target (`wickra_core`) |
//! |-----------------------------------------|------------------------|
//! | [`TradePrint`]                          | [`Trade`]              |
//! | [`OrderBookSnapshot`]                   | [`OrderBook`]          |
//! | [`DerivativesFeed`] (via [`DerivativesTickBuilder`]) | [`DerivativesTick`] |
//! | [`BreadthMember`] slice                 | [`CrossSection`]       |
//!
//! The derivatives channels arrive independently, so [`DerivativesTickBuilder`]
//! folds each [`DerivativesFeed`] into a running tick and emits a validated
//! [`DerivativesTick`] on demand.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use wickra_core::{CrossSection, DerivativesTick, Level, Member, OrderBook, Side, Trade};

use crate::error::{Error, Result};
use crate::events::{OrderBookSnapshot, TradePrint};
use crate::symbol::Symbol;
use crate::types::OrderSide;

/// Convert an exact [`Decimal`] to the `f64` the indicator core consumes. A
/// value outside `f64`'s range becomes `NaN`, which the core's own validators
/// reject — so an out-of-range figure surfaces as a conversion error rather than
/// a silent zero.
fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

/// Map an exchange aggressor side to the core's trade side.
fn core_side(side: OrderSide) -> Side {
    match side {
        OrderSide::Buy => Side::Buy,
        OrderSide::Sell => Side::Sell,
    }
}

/// Convert a public [`TradePrint`] into the core [`Trade`] value consumed by the
/// microstructure indicator family.
///
/// # Errors
///
/// Returns [`Error::Deserialization`] if the price or size is out of the core's
/// valid range (non-finite, non-positive price, or negative size).
pub fn trade_from_print(print: &TradePrint) -> Result<Trade> {
    Trade::new(
        to_f64(print.price),
        to_f64(print.quantity),
        core_side(print.aggressor),
        print.timestamp,
    )
    .map_err(|e| Error::Deserialization(e.to_string()))
}

/// Convert a depth [`OrderBookSnapshot`] into the core [`OrderBook`] consumed by
/// the order-book imbalance / order-flow indicators.
///
/// # Errors
///
/// Returns [`Error::Deserialization`] if either side is empty, a level is
/// invalid, the sides are not strictly best-first, or the book is crossed.
pub fn order_book_from_snapshot(snapshot: &OrderBookSnapshot) -> Result<OrderBook> {
    let to_levels = |levels: &[crate::events::BookLevel]| -> Result<Vec<Level>> {
        levels
            .iter()
            .map(|level| {
                Level::new(to_f64(level.price), to_f64(level.quantity))
                    .map_err(|e| Error::Deserialization(e.to_string()))
            })
            .collect()
    };
    OrderBook::new(to_levels(&snapshot.bids)?, to_levels(&snapshot.asks)?)
        .map_err(|e| Error::Deserialization(e.to_string()))
}

/// One symbol's contribution to a market-breadth cross-section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreadthMember {
    /// Price change versus the previous close (sign classifies advance/decline).
    pub change: Decimal,
    /// Period volume for the symbol (non-negative).
    pub volume: Decimal,
    /// Whether the symbol printed a new period high.
    pub new_high: bool,
    /// Whether the symbol printed a new period low.
    pub new_low: bool,
}

/// Convert a universe of [`BreadthMember`]s into the core [`CrossSection`]
/// consumed by the breadth indicator family.
///
/// # Errors
///
/// Returns [`Error::Deserialization`] if the universe is empty or a member's
/// change/volume is out of the core's valid range.
pub fn cross_section(members: &[BreadthMember], timestamp: i64) -> Result<CrossSection> {
    let members = members
        .iter()
        .map(|member| {
            Member::new(
                to_f64(member.change),
                to_f64(member.volume),
                member.new_high,
                member.new_low,
            )
        })
        .collect();
    CrossSection::new(members, timestamp).map_err(|e| Error::Deserialization(e.to_string()))
}

/// A funding-rate update for a perpetual market.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundingRate {
    /// The market.
    pub symbol: Symbol,
    /// Funding rate for the interval (may be negative).
    pub rate: Decimal,
    /// The perpetual mark price at the funding print.
    pub mark_price: Decimal,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// An open-interest update: outstanding contracts / notional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInterest {
    /// The market.
    pub symbol: Symbol,
    /// Open interest (non-negative).
    pub open_interest: Decimal,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// A forced liquidation print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Liquidation {
    /// The market.
    pub symbol: Symbol,
    /// The side of the forced order hitting the market: a liquidated **long**
    /// sells ([`OrderSide::Sell`]), a liquidated **short** buys
    /// ([`OrderSide::Buy`]).
    pub side: OrderSide,
    /// Liquidation price.
    pub price: Decimal,
    /// Liquidated quantity.
    pub quantity: Decimal,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// A long/short positioning ratio update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongShortRatio {
    /// The market.
    pub symbol: Symbol,
    /// Aggregate long size / long account count (non-negative).
    pub long_size: Decimal,
    /// Aggregate short size / short account count (non-negative).
    pub short_size: Decimal,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// A mark/index price update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkIndex {
    /// The market.
    pub symbol: Symbol,
    /// Perpetual mark price (strictly positive).
    pub mark_price: Decimal,
    /// Spot / index price the perpetual tracks (strictly positive).
    pub index_price: Decimal,
    /// Venue timestamp (milliseconds since the Unix epoch).
    pub timestamp: i64,
}

/// One channel of the derivatives microstructure feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivativesFeed {
    /// A funding-rate print.
    Funding(FundingRate),
    /// An open-interest print.
    OpenInterest(OpenInterest),
    /// A forced-liquidation print.
    Liquidation(Liquidation),
    /// A long/short positioning print.
    LongShortRatio(LongShortRatio),
    /// A mark/index price print.
    MarkIndex(MarkIndex),
}

/// Folds independent [`DerivativesFeed`] channels into a running
/// [`DerivativesTick`].
///
/// The derivatives channels (funding, open interest, liquidations, positioning,
/// mark/index) arrive on their own cadences; apply each as it lands, then
/// [`build`](Self::build) a validated tick for the perpetual-futures indicator
/// family. Liquidation notionals accumulate until [`reset_flows`](Self::reset_flows)
/// clears them for the next interval.
#[derive(Debug, Clone, Default)]
pub struct DerivativesTickBuilder {
    funding_rate: f64,
    mark_price: f64,
    index_price: f64,
    futures_price: f64,
    open_interest: f64,
    long_size: f64,
    short_size: f64,
    long_liquidation: f64,
    short_liquidation: f64,
    timestamp: i64,
}

impl DerivativesTickBuilder {
    /// A fresh builder with every field zeroed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one feed channel into the running tick.
    pub fn apply(&mut self, feed: &DerivativesFeed) {
        match feed {
            DerivativesFeed::Funding(funding) => {
                self.funding_rate = to_f64(funding.rate);
                self.mark_price = to_f64(funding.mark_price);
                self.timestamp = funding.timestamp;
            }
            DerivativesFeed::OpenInterest(oi) => {
                self.open_interest = to_f64(oi.open_interest);
                self.timestamp = oi.timestamp;
            }
            DerivativesFeed::Liquidation(liq) => {
                let notional = to_f64(liq.price * liq.quantity);
                match liq.side {
                    OrderSide::Sell => self.long_liquidation += notional,
                    OrderSide::Buy => self.short_liquidation += notional,
                }
                self.timestamp = liq.timestamp;
            }
            DerivativesFeed::LongShortRatio(ratio) => {
                self.long_size = to_f64(ratio.long_size);
                self.short_size = to_f64(ratio.short_size);
                self.timestamp = ratio.timestamp;
            }
            DerivativesFeed::MarkIndex(mark) => {
                self.mark_price = to_f64(mark.mark_price);
                self.index_price = to_f64(mark.index_price);
                // Absent a dated-futures channel, the perpetual tracks its own mark.
                self.futures_price = to_f64(mark.mark_price);
                self.timestamp = mark.timestamp;
            }
        }
    }

    /// Clear the interval-accumulated liquidation notionals.
    pub fn reset_flows(&mut self) {
        self.long_liquidation = 0.0;
        self.short_liquidation = 0.0;
    }

    /// Build a validated [`DerivativesTick`] from the folded state.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Deserialization`] if a required price channel
    /// (mark/index/futures) has not yet been applied, so the tick would violate
    /// the core's positivity invariants.
    pub fn build(&self) -> Result<DerivativesTick> {
        DerivativesTick::new(
            self.funding_rate,
            self.mark_price,
            self.index_price,
            self.futures_price,
            self.open_interest,
            self.long_size,
            self.short_size,
            // Taker buy/sell volume is carried by the trade channel, not these
            // derivatives feeds; it stays zero until a trade-derived source sets it.
            0.0,
            0.0,
            self.long_liquidation,
            self.short_liquidation,
            self.timestamp,
        )
        .map_err(|e| Error::Deserialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::BookLevel;
    use rust_decimal_macros::dec;

    fn sym() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    #[test]
    fn trade_print_converts_to_core_trade() {
        let print = TradePrint {
            symbol: sym(),
            price: dec!(20000.5),
            quantity: dec!(0.25),
            aggressor: OrderSide::Sell,
            timestamp: 1,
        };
        let trade = trade_from_print(&print).unwrap();
        assert!((trade.price - 20000.5).abs() < 1e-9);
        assert!((trade.size - 0.25).abs() < 1e-12);
        assert_eq!(trade.side, Side::Sell);
        assert_eq!(trade.timestamp, 1);
    }

    #[test]
    fn snapshot_converts_to_core_order_book() {
        let snapshot = OrderBookSnapshot {
            symbol: sym(),
            last_update_id: 7,
            bids: vec![
                BookLevel::new(dec!(100), dec!(2)),
                BookLevel::new(dec!(99), dec!(3)),
            ],
            asks: vec![
                BookLevel::new(dec!(101), dec!(1)),
                BookLevel::new(dec!(102), dec!(4)),
            ],
        };
        let book = order_book_from_snapshot(&snapshot).unwrap();
        assert!((book.best_bid().unwrap().price - 100.0).abs() < 1e-9);
        assert!((book.best_ask().unwrap().price - 101.0).abs() < 1e-9);
        assert_eq!(book.mid(), Some(100.5));
    }

    #[test]
    fn crossed_snapshot_is_rejected() {
        let snapshot = OrderBookSnapshot {
            symbol: sym(),
            last_update_id: 0,
            bids: vec![BookLevel::new(dec!(102), dec!(1))],
            asks: vec![BookLevel::new(dec!(101), dec!(1))],
        };
        assert!(matches!(
            order_book_from_snapshot(&snapshot).unwrap_err(),
            Error::Deserialization(_)
        ));
    }

    #[test]
    fn breadth_universe_converts_to_cross_section() {
        let members = [
            BreadthMember {
                change: dec!(1.5),
                volume: dec!(1000),
                new_high: true,
                new_low: false,
            },
            BreadthMember {
                change: dec!(-0.5),
                volume: dec!(500),
                new_high: false,
                new_low: true,
            },
        ];
        let section = cross_section(&members, 42).unwrap();
        assert_eq!(section.advancers(), 1);
        assert_eq!(section.decliners(), 1);
        assert_eq!(section.timestamp, 42);
    }

    #[test]
    fn empty_universe_is_rejected() {
        assert!(matches!(
            cross_section(&[], 0).unwrap_err(),
            Error::Deserialization(_)
        ));
    }

    #[test]
    fn derivatives_channels_fold_into_a_tick() {
        let mut builder = DerivativesTickBuilder::new();
        builder.apply(&DerivativesFeed::MarkIndex(MarkIndex {
            symbol: sym(),
            mark_price: dec!(20000),
            index_price: dec!(19995),
            timestamp: 10,
        }));
        builder.apply(&DerivativesFeed::Funding(FundingRate {
            symbol: sym(),
            rate: dec!(-0.0001),
            mark_price: dec!(20010),
            timestamp: 11,
        }));
        builder.apply(&DerivativesFeed::OpenInterest(OpenInterest {
            symbol: sym(),
            open_interest: dec!(1234.5),
            timestamp: 12,
        }));
        builder.apply(&DerivativesFeed::LongShortRatio(LongShortRatio {
            symbol: sym(),
            long_size: dec!(60),
            short_size: dec!(40),
            timestamp: 13,
        }));
        // A liquidated long is a forced sell.
        builder.apply(&DerivativesFeed::Liquidation(Liquidation {
            symbol: sym(),
            side: OrderSide::Sell,
            price: dec!(20000),
            quantity: dec!(0.5),
            timestamp: 14,
        }));

        let tick = builder.build().unwrap();
        assert!((tick.funding_rate + 0.0001).abs() < 1e-12);
        assert!((tick.mark_price - 20010.0).abs() < 1e-9);
        assert!((tick.index_price - 19995.0).abs() < 1e-9);
        assert!((tick.open_interest - 1234.5).abs() < 1e-9);
        assert!((tick.long_size - 60.0).abs() < 1e-9);
        assert!((tick.long_liquidation - 10000.0).abs() < 1e-9);
        assert!(tick.short_liquidation.abs() < 1e-9);
        assert_eq!(tick.timestamp, 14);
    }

    #[test]
    fn short_liquidation_accumulates_and_resets() {
        let mut builder = DerivativesTickBuilder::new();
        builder.apply(&DerivativesFeed::MarkIndex(MarkIndex {
            symbol: sym(),
            mark_price: dec!(100),
            index_price: dec!(100),
            timestamp: 1,
        }));
        // A liquidated short is a forced buy; accumulate two of them.
        for _ in 0..2 {
            builder.apply(&DerivativesFeed::Liquidation(Liquidation {
                symbol: sym(),
                side: OrderSide::Buy,
                price: dec!(100),
                quantity: dec!(1),
                timestamp: 2,
            }));
        }
        assert!((builder.build().unwrap().short_liquidation - 200.0).abs() < 1e-9);
        builder.reset_flows();
        assert!(builder.build().unwrap().short_liquidation.abs() < 1e-9);
    }

    #[test]
    fn build_before_a_price_channel_is_rejected() {
        // No mark/index applied: prices are zero, violating the core's positivity.
        assert!(matches!(
            DerivativesTickBuilder::new().build().unwrap_err(),
            Error::Deserialization(_)
        ));
    }
}
