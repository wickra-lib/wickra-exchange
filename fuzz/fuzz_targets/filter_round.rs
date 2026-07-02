#![no_main]
//! Fuzz the instrument-filter rounding with a value and realistic (step, tick)
//! increments. The rounding must not panic and, when stepping is active, must
//! never round the value up.
//!
//! Increments are generated as **clean decimals** (a small mantissa on a
//! `scale` grid, e.g. `0.01`, `0.5`, `25`) rather than arbitrary `f64`s — a
//! documented precondition that models reality: venues send tick/step as decimal
//! strings parsed exactly, never `f64`-mantissa-noisy values. An arbitrary `f64`
//! increment only exercises `Decimal`'s 28-significant-digit precision cliff
//! (where the divide/floor/multiply round-trip cannot stay exact), not the
//! rounding logic under test. The value is a non-negative price/size within a
//! realistic band.

use libfuzzer_sys::fuzz_target;
use rust_decimal::Decimal;
use wickra_exchange_core::InstrumentFilters;

/// A non-negative price/size within a realistic band, or `None` to skip.
fn bounded_value(raw: f64) -> Option<Decimal> {
    if !raw.is_finite() || raw.abs() > 1.0e9 {
        return None;
    }
    Decimal::from_f64_retain(raw.abs())
}

/// A realistic venue increment: `mantissa * 10^-scale` with a small mantissa and
/// `scale` in `0..=8`, e.g. `0.01`, `5.0`, `0.00000255`. A zero mantissa means no
/// stepping. This is the clean decimal grid real venues quote on.
fn realistic_increment(mantissa: u8, scale: u8) -> Decimal {
    Decimal::new(i64::from(mantissa), u32::from(scale % 9))
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }
    let Some(value) = bounded_value(f64::from_le_bytes(data[0..8].try_into().unwrap())) else {
        return;
    };
    let step = realistic_increment(data[8], data[9]);
    let tick = realistic_increment(data[10], data[11]);

    let filters = InstrumentFilters {
        min_quantity: Decimal::ZERO,
        max_quantity: Decimal::ZERO,
        step_size: step,
        min_price: Decimal::ZERO,
        max_price: Decimal::ZERO,
        tick_size: tick,
        min_notional: Decimal::ZERO,
    };

    let rounded_qty = filters.round_quantity(value);
    let rounded_price = filters.round_price(value);
    // Rounding down never increases the value (a no-op when the step is zero).
    assert!(rounded_qty <= value);
    assert!(rounded_price <= value);
    let _ = filters.validate(value, Some(value));
});
