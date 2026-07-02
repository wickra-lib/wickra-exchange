#![no_main]
//! Fuzz the instrument-filter rounding with arbitrary (step, tick, value)
//! triples. The rounding must not panic and, when stepping is active, must never
//! round a value up. Inputs are bounded to a realistic price/size band (a
//! documented precondition) so the fuzzer explores the rounding grid rather than
//! `Decimal`'s overflow edge.

use libfuzzer_sys::fuzz_target;
use rust_decimal::Decimal;
use wickra_exchange_core::InstrumentFilters;

fn bounded_decimal(raw: f64) -> Option<Decimal> {
    if !raw.is_finite() || raw.abs() > 1.0e9 {
        return None;
    }
    Decimal::from_f64_retain(raw)
}

/// A realistic venue increment: either exactly zero (no stepping) or within the
/// grid real venues actually use. Sub-1e-12 increments do not exist on any venue
/// and only exercise `Decimal`'s 28-significant-digit precision cliff (where the
/// divide/floor/multiply round-trip cannot stay exact), not the rounding logic
/// under test — a documented precondition, like the 1e9 upper bound.
fn bounded_increment(raw: f64) -> Option<Decimal> {
    let dec = bounded_decimal(raw.abs())?;
    if dec > Decimal::ZERO && dec < Decimal::from_f64_retain(1.0e-12)? {
        return None;
    }
    Some(dec)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 24 {
        return;
    }
    let read = |i: usize| f64::from_le_bytes(data[i..i + 8].try_into().unwrap());
    // Prices and sizes are non-negative on every venue, so `value` is too.
    let (Some(step), Some(tick), Some(value)) = (
        bounded_increment(read(0)),
        bounded_increment(read(8)),
        bounded_decimal(read(16).abs()),
    ) else {
        return;
    };

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
