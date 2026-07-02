//! Instrument metadata, symbol filters and order rounding.
//!
//! Every venue constrains orders with per-symbol filters — a quantity step
//! (`LOT_SIZE`), a price tick (`PRICE_FILTER`), a minimum notional. Getting the
//! rounding wrong means a rejected or mis-sized order, so it is done here in
//! exact [`Decimal`] arithmetic and tested at the edges. [`InstrumentCache`]
//! holds the `exchangeInfo` metadata with a deterministic, caller-driven refresh
//! clock (no hidden wall-clock, so it is testable).

use crate::error::{Error, Result};
use crate::symbol::Symbol;
use rust_decimal::Decimal;
use std::collections::HashMap;

/// The order constraints for one instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentFilters {
    /// Minimum order quantity (`LOT_SIZE` `minQty`).
    pub min_quantity: Decimal,
    /// Maximum order quantity (`LOT_SIZE` `maxQty`); `0` means no maximum.
    pub max_quantity: Decimal,
    /// Quantity increment (`LOT_SIZE` `stepSize`); `0` means no stepping.
    pub step_size: Decimal,
    /// Minimum price (`PRICE_FILTER` `minPrice`).
    pub min_price: Decimal,
    /// Maximum price (`PRICE_FILTER` `maxPrice`); `0` means no maximum.
    pub max_price: Decimal,
    /// Price increment (`PRICE_FILTER` `tickSize`); `0` means no stepping.
    pub tick_size: Decimal,
    /// Minimum order notional (`price * quantity`); `0` means no minimum.
    pub min_notional: Decimal,
}

/// Round `value` down to the nearest multiple of `increment`. A zero or negative
/// `increment` is a no-op (the value is returned unchanged).
fn round_down_to(value: Decimal, increment: Decimal) -> Decimal {
    if increment <= Decimal::ZERO {
        return value;
    }
    // A vanishingly small increment relative to the value (e.g. a malformed
    // venue filter) makes `value / increment` exceed `Decimal`'s range. Use the
    // checked ops and fall back to the unrounded value rather than panicking on
    // untrusted filter data.
    match value
        .checked_div(increment)
        .map(|steps| steps.floor())
        .and_then(|steps| steps.checked_mul(increment))
    {
        Some(rounded) => rounded,
        None => value,
    }
}

impl InstrumentFilters {
    /// Round a quantity **down** to the venue's step size, so the result never
    /// exceeds the requested amount.
    #[must_use]
    pub fn round_quantity(&self, quantity: Decimal) -> Decimal {
        round_down_to(quantity, self.step_size)
    }

    /// Round a price **down** to the venue's tick size, putting it on the price
    /// grid the venue accepts.
    #[must_use]
    pub fn round_price(&self, price: Decimal) -> Decimal {
        round_down_to(price, self.tick_size)
    }

    /// Validate a quantity and optional price against every filter.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Filter`] naming the first filter the order violates:
    /// quantity below `min`/above `max`/off the step grid, price below `min`/
    /// above `max`/off the tick grid, or notional below the minimum.
    pub fn validate(&self, quantity: Decimal, price: Option<Decimal>) -> Result<()> {
        if quantity < self.min_quantity {
            return Err(Error::Filter(format!(
                "LOT_SIZE: quantity {quantity} below minimum {}",
                self.min_quantity
            )));
        }
        if self.max_quantity > Decimal::ZERO && quantity > self.max_quantity {
            return Err(Error::Filter(format!(
                "LOT_SIZE: quantity {quantity} above maximum {}",
                self.max_quantity
            )));
        }
        if self.step_size > Decimal::ZERO && (quantity % self.step_size) != Decimal::ZERO {
            return Err(Error::Filter(format!(
                "LOT_SIZE: quantity {quantity} not a multiple of step {}",
                self.step_size
            )));
        }
        if let Some(price) = price {
            if price < self.min_price {
                return Err(Error::Filter(format!(
                    "PRICE_FILTER: price {price} below minimum {}",
                    self.min_price
                )));
            }
            if self.max_price > Decimal::ZERO && price > self.max_price {
                return Err(Error::Filter(format!(
                    "PRICE_FILTER: price {price} above maximum {}",
                    self.max_price
                )));
            }
            if self.tick_size > Decimal::ZERO && (price % self.tick_size) != Decimal::ZERO {
                return Err(Error::Filter(format!(
                    "PRICE_FILTER: price {price} not a multiple of tick {}",
                    self.tick_size
                )));
            }
            if self.min_notional > Decimal::ZERO && quantity * price < self.min_notional {
                return Err(Error::Filter(format!(
                    "MIN_NOTIONAL: notional {} below minimum {}",
                    quantity * price,
                    self.min_notional
                )));
            }
        }
        Ok(())
    }
}

/// One tradable instrument and its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// The canonical symbol.
    pub symbol: Symbol,
    /// Base-asset display precision (decimal places).
    pub base_precision: u32,
    /// Quote-asset display precision (decimal places).
    pub quote_precision: u32,
    /// The order filters.
    pub filters: InstrumentFilters,
}

/// A cache of instrument metadata with a caller-driven refresh clock.
///
/// The refresh decision takes the current time as an argument rather than
/// reading a hidden clock, so staleness logic is deterministic and testable.
#[derive(Debug, Default)]
pub struct InstrumentCache {
    by_symbol: HashMap<Symbol, Instrument>,
    fetched_at_ms: Option<i64>,
}

impl InstrumentCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the cache contents with a freshly fetched set, stamping the fetch
    /// time (milliseconds since the Unix epoch).
    pub fn replace(&mut self, instruments: impl IntoIterator<Item = Instrument>, now_ms: i64) {
        self.by_symbol = instruments
            .into_iter()
            .map(|inst| (inst.symbol.clone(), inst))
            .collect();
        self.fetched_at_ms = Some(now_ms);
    }

    /// Look up an instrument by symbol.
    #[must_use]
    pub fn get(&self, symbol: &Symbol) -> Option<&Instrument> {
        self.by_symbol.get(symbol)
    }

    /// The number of cached instruments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_symbol.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty()
    }

    /// Whether the cache should be refreshed: never fetched, or older than
    /// `ttl_ms` relative to `now_ms`.
    #[must_use]
    pub fn needs_refresh(&self, now_ms: i64, ttl_ms: i64) -> bool {
        match self.fetched_at_ms {
            None => true,
            Some(fetched) => now_ms - fetched >= ttl_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn filters() -> InstrumentFilters {
        InstrumentFilters {
            min_quantity: dec!(0.001),
            max_quantity: dec!(1000),
            step_size: dec!(0.001),
            min_price: dec!(0.01),
            max_price: dec!(1000000),
            tick_size: dec!(0.01),
            min_notional: dec!(10),
        }
    }

    #[test]
    fn rounds_quantity_and_price_down_to_grid() {
        let f = filters();
        assert_eq!(f.round_quantity(dec!(0.0012345)), dec!(0.001));
        assert_eq!(f.round_quantity(dec!(1.2399)), dec!(1.239));
        assert_eq!(f.round_price(dec!(20000.017)), dec!(20000.01));
        assert_eq!(f.round_price(dec!(20000.00)), dec!(20000.00));
    }

    #[test]
    fn zero_increment_is_a_no_op() {
        let f = InstrumentFilters {
            step_size: Decimal::ZERO,
            tick_size: Decimal::ZERO,
            ..filters()
        };
        assert_eq!(f.round_quantity(dec!(1.23456789)), dec!(1.23456789));
        assert_eq!(f.round_price(dec!(1.23456789)), dec!(1.23456789));
    }

    #[test]
    fn tiny_increment_does_not_panic_on_overflow() {
        // A vanishingly small step relative to the value overflows the
        // intermediate `value / increment`; rounding must fall back to the
        // value rather than panic (found by the `filter_round` fuzz target).
        let f = InstrumentFilters {
            step_size: dec!(0.0000000000000000000000001), // 1e-25
            tick_size: dec!(0.0000000000000000000000001),
            ..filters()
        };
        let value = dec!(1000000000); // 1e9; 1e9 / 1e-25 = 1e34 > Decimal::MAX
        assert_eq!(f.round_quantity(value), value);
        assert_eq!(f.round_price(value), value);
    }

    #[test]
    fn validate_accepts_a_clean_order() {
        let f = filters();
        assert!(f.validate(dec!(0.5), Some(dec!(20000.00))).is_ok());
        // No price: notional and price filters are skipped.
        assert!(f.validate(dec!(0.5), None).is_ok());
    }

    #[test]
    fn validate_rejects_quantity_violations() {
        let f = filters();
        assert!(matches!(
            f.validate(dec!(0.0005), Some(dec!(20000))).unwrap_err(),
            Error::Filter(m) if m.contains("below minimum")
        ));
        assert!(matches!(
            f.validate(dec!(2000), Some(dec!(20000))).unwrap_err(),
            Error::Filter(m) if m.contains("above maximum")
        ));
        assert!(matches!(
            f.validate(dec!(0.0015), Some(dec!(20000))).unwrap_err(),
            Error::Filter(m) if m.contains("multiple of step")
        ));
    }

    #[test]
    fn validate_rejects_price_and_notional_violations() {
        let f = filters();
        assert!(matches!(
            f.validate(dec!(0.5), Some(dec!(0.001))).unwrap_err(),
            Error::Filter(m) if m.contains("price") && m.contains("below minimum")
        ));
        assert!(matches!(
            f.validate(dec!(0.5), Some(dec!(2000000))).unwrap_err(),
            Error::Filter(m) if m.contains("price") && m.contains("above maximum")
        ));
        assert!(matches!(
            f.validate(dec!(0.5), Some(dec!(20000.017))).unwrap_err(),
            Error::Filter(m) if m.contains("multiple of tick")
        ));
        // 0.001 * 5000 = 5 < min_notional 10.
        assert!(matches!(
            f.validate(dec!(0.001), Some(dec!(5000))).unwrap_err(),
            Error::Filter(m) if m.contains("MIN_NOTIONAL")
        ));
    }

    #[test]
    fn cache_stores_and_refreshes_deterministically() {
        let mut cache = InstrumentCache::new();
        assert!(cache.is_empty());
        assert!(cache.needs_refresh(1_000, 60_000));

        let inst = Instrument {
            symbol: Symbol::new("BTC", "USDT"),
            base_precision: 8,
            quote_precision: 2,
            filters: filters(),
        };
        cache.replace([inst.clone()], 1_000);
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());
        assert_eq!(cache.get(&Symbol::new("BTC", "USDT")), Some(&inst));
        assert!(cache.get(&Symbol::new("ETH", "USDT")).is_none());

        // Within TTL: fresh. Past TTL: stale.
        assert!(!cache.needs_refresh(30_000, 60_000));
        assert!(cache.needs_refresh(61_000, 60_000));
    }
}
