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

fuzz_target!(|data: &[u8]| {
    if data.len() < 24 {
        return;
    }
    let read = |i: usize| f64::from_le_bytes(data[i..i + 8].try_into().unwrap());
    let (Some(step), Some(tick), Some(value)) = (
        bounded_decimal(read(0).abs()),
        bounded_decimal(read(8).abs()),
        bounded_decimal(read(16)),
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
