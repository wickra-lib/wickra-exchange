//! Derivatives position state.
//!
//! On a perpetual/futures account a [`Position`] is the net exposure in one
//! market: a side, a magnitude, an entry and mark price, leverage and margin
//! mode. Venues report unrealized PnL, but it is also computable from the entry
//! and mark prices — which the paper and replay exchanges need — so that
//! calculation lives here, tested.

use crate::options::MarginMode;
use crate::symbol::Symbol;
use crate::types::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The direction of a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionSide {
    /// Long: profits when the price rises.
    Long,
    /// Short: profits when the price falls.
    Short,
}

impl PositionSide {
    /// `+1` for long, `-1` for short.
    #[must_use]
    pub fn sign(self) -> Decimal {
        match self {
            PositionSide::Long => Decimal::ONE,
            PositionSide::Short => Decimal::NEGATIVE_ONE,
        }
    }

    /// Which side of a hedged account an order acts on.
    ///
    /// In [`PositionMode::Hedge`](crate::PositionMode) a symbol carries a long
    /// and a short position at once, so an order has to name the one it means.
    /// [`OrderRequest`](crate::OrderRequest) does not carry that name, and does
    /// not need to: buying opens the long side or closes the short one, and
    /// selling does the reverse. `reduce_only` is what separates the two.
    ///
    /// | side | `reduce_only` | acts on |
    /// |------|---------------|---------|
    /// | buy  | `false`       | long, opening |
    /// | sell | `false`       | short, opening |
    /// | buy  | `true`        | short, closing |
    /// | sell | `true`        | long, closing |
    ///
    /// Every venue that supports hedged accounts spells this the same way under
    /// a different field name, so the mapping lives here once.
    #[must_use]
    pub fn for_order(side: OrderSide, reduce_only: bool) -> Self {
        match (side, reduce_only) {
            (OrderSide::Buy, false) | (OrderSide::Sell, true) => PositionSide::Long,
            (OrderSide::Sell, false) | (OrderSide::Buy, true) => PositionSide::Short,
        }
    }
}

/// A net position in one derivatives market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// The market.
    pub symbol: Symbol,
    /// Long or short.
    pub side: PositionSide,
    /// Position size in base units (magnitude, always positive).
    pub quantity: Decimal,
    /// Average entry price.
    pub entry_price: Decimal,
    /// Current mark price.
    pub mark_price: Decimal,
    /// Account leverage for this position.
    pub leverage: Decimal,
    /// Unrealized PnL as reported by the venue.
    pub unrealized_pnl: Decimal,
    /// Cross or isolated margin.
    pub margin_mode: MarginMode,
}

impl Position {
    /// Signed size: positive for long, negative for short.
    #[must_use]
    pub fn signed_quantity(&self) -> Decimal {
        self.quantity * self.side.sign()
    }

    /// Notional value at the mark price (`quantity * mark_price`).
    #[must_use]
    pub fn notional(&self) -> Decimal {
        self.quantity * self.mark_price
    }

    /// Unrealized PnL computed from entry and mark prices:
    /// `(mark - entry) * signed_quantity`. Used by the paper/replay exchanges
    /// where the venue does not supply it.
    #[must_use]
    pub fn computed_unrealized_pnl(&self) -> Decimal {
        (self.mark_price - self.entry_price) * self.signed_quantity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn position(side: PositionSide, entry: Decimal, mark: Decimal) -> Position {
        Position {
            symbol: Symbol::new("BTC", "USDT"),
            side,
            quantity: dec!(2),
            entry_price: entry,
            mark_price: mark,
            leverage: dec!(10),
            unrealized_pnl: dec!(0),
            margin_mode: MarginMode::Cross,
        }
    }

    #[test]
    fn hedge_side_follows_side_and_reduce_only() {
        // Opening: the side of the order is the side of the position.
        assert_eq!(
            PositionSide::for_order(OrderSide::Buy, false),
            PositionSide::Long
        );
        assert_eq!(
            PositionSide::for_order(OrderSide::Sell, false),
            PositionSide::Short
        );
        // Closing: the order takes the opposite side of the position it reduces.
        assert_eq!(
            PositionSide::for_order(OrderSide::Buy, true),
            PositionSide::Short
        );
        assert_eq!(
            PositionSide::for_order(OrderSide::Sell, true),
            PositionSide::Long
        );
    }

    #[test]
    fn side_sign() {
        assert_eq!(PositionSide::Long.sign(), dec!(1));
        assert_eq!(PositionSide::Short.sign(), dec!(-1));
    }

    #[test]
    fn signed_quantity_and_notional() {
        let long = position(PositionSide::Long, dec!(100), dec!(110));
        assert_eq!(long.signed_quantity(), dec!(2));
        assert_eq!(long.notional(), dec!(220));

        let short = position(PositionSide::Short, dec!(100), dec!(110));
        assert_eq!(short.signed_quantity(), dec!(-2));
        assert_eq!(short.notional(), dec!(220));
    }

    #[test]
    fn computed_pnl_for_both_sides() {
        // Long up 10 on 2 units = +20.
        let long = position(PositionSide::Long, dec!(100), dec!(110));
        assert_eq!(long.computed_unrealized_pnl(), dec!(20));
        // Short, price up 10 = -20.
        let short = position(PositionSide::Short, dec!(100), dec!(110));
        assert_eq!(short.computed_unrealized_pnl(), dec!(-20));
        // Short, price down 10 = +20.
        let short_win = position(PositionSide::Short, dec!(100), dec!(90));
        assert_eq!(short_win.computed_unrealized_pnl(), dec!(20));
    }
}
