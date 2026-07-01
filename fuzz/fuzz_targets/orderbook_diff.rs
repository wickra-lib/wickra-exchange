#![no_main]
//! Fuzz the local order-book maintenance: apply an arbitrary snapshot, then a
//! stream of arbitrary diffs. The builder's sequence-gap detection and ladder
//! mutation must never panic, whatever the ordering, ids or (removed) levels.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use rust_decimal::Decimal;
use wickra_exchange_core::{BookDelta, BookLevel, OrderBookBuilder, OrderBookSnapshot, Symbol};

#[derive(Arbitrary, Debug)]
struct RawLevel {
    price: u16,
    quantity: u16,
}

#[derive(Arbitrary, Debug)]
struct RawDelta {
    first_update_id: u32,
    final_update_id: u32,
    bids: Vec<RawLevel>,
    asks: Vec<RawLevel>,
}

#[derive(Arbitrary, Debug)]
struct Input {
    snapshot_id: u32,
    snapshot_bids: Vec<RawLevel>,
    snapshot_asks: Vec<RawLevel>,
    deltas: Vec<RawDelta>,
}

fn levels(raw: &[RawLevel]) -> Vec<BookLevel> {
    raw.iter()
        .map(|level| BookLevel::new(Decimal::from(level.price), Decimal::from(level.quantity)))
        .collect()
}

fuzz_target!(|input: Input| {
    let symbol = Symbol::new("BTC", "USDT");
    let mut book = OrderBookBuilder::new(symbol.clone());

    let snapshot = OrderBookSnapshot {
        symbol: symbol.clone(),
        last_update_id: u64::from(input.snapshot_id),
        bids: levels(&input.snapshot_bids),
        asks: levels(&input.snapshot_asks),
    };
    book.apply_snapshot(&snapshot);

    for delta in &input.deltas {
        let diff = BookDelta {
            symbol: symbol.clone(),
            first_update_id: u64::from(delta.first_update_id),
            final_update_id: u64::from(delta.final_update_id),
            bids: levels(&delta.bids),
            asks: levels(&delta.asks),
        };
        let _ = book.apply_delta(&diff);
    }
});
