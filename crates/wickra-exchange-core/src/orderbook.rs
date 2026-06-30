//! A locally maintained level-2 order book.
//!
//! Streaming depth arrives as a snapshot followed by incremental diffs. Keeping
//! a correct local book is its own fiddly problem: diffs must be applied in
//! sequence, stale diffs (already covered by the snapshot) dropped, and a
//! **sequence gap** — a missing diff — detected so the book can be re-synced
//! from a fresh snapshot rather than silently drifting. [`OrderBookBuilder`]
//! does exactly that, following the canonical depth-management algorithm.

use crate::events::{BookDelta, BookLevel, OrderBookSnapshot};
use crate::symbol::Symbol;
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// The outcome of applying a [`BookDelta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookUpdate {
    /// The diff was applied and advanced the book.
    Applied,
    /// The diff is older than the current state and was ignored.
    Stale,
    /// A sequence gap was detected: a diff is missing. The book is now marked
    /// uninitialized and must be re-synced from a fresh snapshot.
    Gap,
    /// No snapshot has been applied yet; the diff cannot be used.
    Uninitialized,
}

/// A local L2 book built from a snapshot and a stream of diffs.
#[derive(Debug)]
pub struct OrderBookBuilder {
    symbol: Symbol,
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    last_update_id: u64,
    initialized: bool,
}

impl OrderBookBuilder {
    /// A new, uninitialized builder for `symbol`.
    #[must_use]
    pub fn new(symbol: Symbol) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_update_id: 0,
            initialized: false,
        }
    }

    /// The market this book tracks.
    #[must_use]
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// Whether a snapshot has been applied and no unresolved gap exists.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// The last sequence id the book has incorporated.
    #[must_use]
    pub fn last_update_id(&self) -> u64 {
        self.last_update_id
    }

    /// Reset the book from a full snapshot, marking it initialized.
    pub fn apply_snapshot(&mut self, snapshot: &OrderBookSnapshot) {
        self.bids = snapshot
            .bids
            .iter()
            .filter(|l| l.quantity > Decimal::ZERO)
            .map(|l| (l.price, l.quantity))
            .collect();
        self.asks = snapshot
            .asks
            .iter()
            .filter(|l| l.quantity > Decimal::ZERO)
            .map(|l| (l.price, l.quantity))
            .collect();
        self.last_update_id = snapshot.last_update_id;
        self.initialized = true;
    }

    /// Apply an incremental diff, returning how it was handled.
    ///
    /// Stale diffs are ignored; a gap marks the book uninitialized (the caller
    /// should re-fetch a snapshot and call [`apply_snapshot`](Self::apply_snapshot)).
    pub fn apply_delta(&mut self, delta: &BookDelta) -> BookUpdate {
        if !self.initialized {
            return BookUpdate::Uninitialized;
        }
        // Entirely before our state: already covered by the snapshot.
        if delta.final_update_id <= self.last_update_id {
            return BookUpdate::Stale;
        }
        // A missing diff: the next expected id is last_update_id + 1.
        if delta.first_update_id > self.last_update_id + 1 {
            self.initialized = false;
            return BookUpdate::Gap;
        }
        for level in &delta.bids {
            apply_level(&mut self.bids, level);
        }
        for level in &delta.asks {
            apply_level(&mut self.asks, level);
        }
        self.last_update_id = delta.final_update_id;
        BookUpdate::Applied
    }

    /// The best (highest-price) bid as `(price, quantity)`.
    #[must_use]
    pub fn best_bid(&self) -> Option<(Decimal, Decimal)> {
        self.bids.iter().next_back().map(|(&p, &q)| (p, q))
    }

    /// The best (lowest-price) ask as `(price, quantity)`.
    #[must_use]
    pub fn best_ask(&self) -> Option<(Decimal, Decimal)> {
        self.asks.iter().next().map(|(&p, &q)| (p, q))
    }

    /// The mid price `(best_bid + best_ask) / 2`, or `None` if either side is empty.
    #[must_use]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some((bid + ask) / Decimal::from(2)),
            _ => None,
        }
    }

    /// The spread `best_ask - best_bid`, or `None` if either side is empty.
    #[must_use]
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some(ask - bid),
            _ => None,
        }
    }

    /// Export the top `depth` levels per side as a snapshot, best-first.
    #[must_use]
    pub fn to_snapshot(&self, depth: usize) -> OrderBookSnapshot {
        let bids = self
            .bids
            .iter()
            .rev()
            .take(depth)
            .map(|(&price, &quantity)| BookLevel { price, quantity })
            .collect();
        let asks = self
            .asks
            .iter()
            .take(depth)
            .map(|(&price, &quantity)| BookLevel { price, quantity })
            .collect();
        OrderBookSnapshot {
            symbol: self.symbol.clone(),
            last_update_id: self.last_update_id,
            bids,
            asks,
        }
    }
}

/// Insert/update a level; a zero (or negative) quantity removes it.
fn apply_level(side: &mut BTreeMap<Decimal, Decimal>, level: &BookLevel) {
    if level.quantity <= Decimal::ZERO {
        side.remove(&level.price);
    } else {
        side.insert(level.price, level.quantity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn symbol() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    fn snapshot(id: u64) -> OrderBookSnapshot {
        OrderBookSnapshot {
            symbol: symbol(),
            last_update_id: id,
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
    fn diff_before_snapshot_is_uninitialized() {
        let mut book = OrderBookBuilder::new(symbol());
        assert!(!book.is_initialized());
        let delta = BookDelta {
            symbol: symbol(),
            first_update_id: 1,
            final_update_id: 2,
            bids: vec![],
            asks: vec![],
        };
        assert_eq!(book.apply_delta(&delta), BookUpdate::Uninitialized);
    }

    #[test]
    fn snapshot_then_best_levels() {
        let mut book = OrderBookBuilder::new(symbol());
        book.apply_snapshot(&snapshot(10));
        assert!(book.is_initialized());
        assert_eq!(book.last_update_id(), 10);
        assert_eq!(book.symbol(), &symbol());
        assert_eq!(book.best_bid(), Some((dec!(100), dec!(1))));
        assert_eq!(book.best_ask(), Some((dec!(101), dec!(1))));
        assert_eq!(book.mid_price(), Some(dec!(100.5)));
        assert_eq!(book.spread(), Some(dec!(1)));
    }

    #[test]
    fn contiguous_diff_applies_and_advances() {
        let mut book = OrderBookBuilder::new(symbol());
        book.apply_snapshot(&snapshot(10));
        let delta = BookDelta {
            symbol: symbol(),
            first_update_id: 11,
            final_update_id: 12,
            // Improve the bid, remove the top ask.
            bids: vec![BookLevel::new(dec!(100.5), dec!(5))],
            asks: vec![BookLevel::new(dec!(101), dec!(0))],
        };
        assert_eq!(book.apply_delta(&delta), BookUpdate::Applied);
        assert_eq!(book.last_update_id(), 12);
        assert_eq!(book.best_bid(), Some((dec!(100.5), dec!(5))));
        assert_eq!(book.best_ask(), Some((dec!(102), dec!(3))));
    }

    #[test]
    fn stale_diff_is_ignored() {
        let mut book = OrderBookBuilder::new(symbol());
        book.apply_snapshot(&snapshot(10));
        let delta = BookDelta {
            symbol: symbol(),
            first_update_id: 5,
            final_update_id: 9,
            bids: vec![BookLevel::new(dec!(100), dec!(99))],
            asks: vec![],
        };
        assert_eq!(book.apply_delta(&delta), BookUpdate::Stale);
        // Unchanged.
        assert_eq!(book.best_bid(), Some((dec!(100), dec!(1))));
        assert_eq!(book.last_update_id(), 10);
    }

    #[test]
    fn gap_marks_uninitialized_for_resync() {
        let mut book = OrderBookBuilder::new(symbol());
        book.apply_snapshot(&snapshot(10));
        // Expected next first id is 11; jumping to 13 is a gap.
        let delta = BookDelta {
            symbol: symbol(),
            first_update_id: 13,
            final_update_id: 14,
            bids: vec![],
            asks: vec![],
        };
        assert_eq!(book.apply_delta(&delta), BookUpdate::Gap);
        assert!(!book.is_initialized());

        // Re-syncing from a fresh snapshot recovers.
        book.apply_snapshot(&snapshot(14));
        assert!(book.is_initialized());
        assert_eq!(book.last_update_id(), 14);
    }

    #[test]
    fn snapshot_drops_zero_quantity_levels() {
        let mut book = OrderBookBuilder::new(symbol());
        let mut snap = snapshot(10);
        snap.bids.push(BookLevel::new(dec!(98), dec!(0)));
        book.apply_snapshot(&snap);
        // The zero-quantity bid is not retained.
        let exported = book.to_snapshot(10);
        assert!(exported.bids.iter().all(|l| l.quantity > dec!(0)));
    }

    #[test]
    fn to_snapshot_limits_depth_and_orders_best_first() {
        let mut book = OrderBookBuilder::new(symbol());
        book.apply_snapshot(&snapshot(10));
        let snap = book.to_snapshot(1);
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.asks.len(), 1);
        assert_eq!(snap.bids[0].price, dec!(100));
        assert_eq!(snap.asks[0].price, dec!(101));
        assert_eq!(snap.last_update_id, 10);
    }

    #[test]
    fn empty_book_has_no_mid_or_spread() {
        let book = OrderBookBuilder::new(symbol());
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
        assert!(book.mid_price().is_none());
        assert!(book.spread().is_none());
    }
}
