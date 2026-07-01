//! Property-based invariants for the shared connectivity machinery: filter
//! rounding, decimal parse/format round-trips and symbol round-trips.

use std::str::FromStr;

use proptest::prelude::*;
use rust_decimal::Decimal;
use wickra_exchange_core::{format_decimal, parse_decimal, InstrumentFilters, Symbol};

fn filters_with_step(step: Decimal, tick: Decimal) -> InstrumentFilters {
    InstrumentFilters {
        min_quantity: Decimal::ZERO,
        max_quantity: Decimal::ZERO,
        step_size: step,
        min_price: Decimal::ZERO,
        max_price: Decimal::ZERO,
        tick_size: tick,
        min_notional: Decimal::ZERO,
    }
}

proptest! {
    /// Rounding down to a positive step never increases the value, and the
    /// discarded remainder always lies in `[0, step)`.
    #[test]
    fn round_quantity_is_floor_to_step(
        mantissa in -1_000_000_000i64..1_000_000_000,
        scale in 0u32..6,
        step_mantissa in 1i64..1_000_000,
        step_scale in 0u32..6,
    ) {
        let value = Decimal::new(mantissa, scale);
        let step = Decimal::new(step_mantissa, step_scale);
        let filters = filters_with_step(step, Decimal::ZERO);

        let rounded = filters.round_quantity(value);
        prop_assert!(rounded <= value);
        let remainder = value - rounded;
        prop_assert!(remainder >= Decimal::ZERO);
        prop_assert!(remainder < step);
    }

    /// A zero step is a no-op: the value is returned unchanged.
    #[test]
    fn zero_step_is_identity(mantissa in -1_000_000i64..1_000_000, scale in 0u32..6) {
        let value = Decimal::new(mantissa, scale);
        let filters = filters_with_step(Decimal::ZERO, Decimal::ZERO);
        prop_assert_eq!(filters.round_quantity(value), value);
        prop_assert_eq!(filters.round_price(value), value);
    }

    /// `parse_decimal(format_decimal(d))` recovers the original value.
    #[test]
    fn decimal_format_parse_round_trips(mantissa in i64::MIN..i64::MAX, scale in 0u32..12) {
        let value = Decimal::new(mantissa, scale);
        let text = format_decimal(value);
        let parsed = parse_decimal(&text).expect("formatted decimal must re-parse");
        prop_assert_eq!(parsed, value);
    }

    /// A `BASE/QUOTE` symbol round-trips through its string form.
    #[test]
    fn symbol_round_trips(base in "[A-Z]{2,6}", quote in "[A-Z]{2,6}") {
        let symbol = Symbol::new(&base, &quote);
        let slashed = format!("{base}/{quote}");
        prop_assert_eq!(Symbol::from_str(&slashed).unwrap(), symbol.clone());
        // The dash separator is equivalent.
        let dashed = format!("{base}-{quote}");
        prop_assert_eq!(Symbol::from_str(&dashed).unwrap(), symbol);
    }
}
