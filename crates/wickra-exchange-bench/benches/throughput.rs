//! Throughput benchmarks for the hot connectivity paths: request signing,
//! response parsing, filter rounding and local order-book diffing.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use wickra_exchange_core::{
    format_decimal, hmac_sha256_hex, hmac_sha512_hex, parse_decimal, sha256, BookDelta, BookLevel,
    Event, InstrumentFilters, OrderBookBuilder, OrderBookSnapshot, Symbol,
};

fn bench_signing(c: &mut Criterion) {
    let secret = b"0123456789abcdef0123456789abcdef";
    let message =
        b"timestamp=1700000000000&symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=1&price=20000";
    let mut group = c.benchmark_group("signing");
    group.bench_function("hmac_sha256_hex", |b| {
        b.iter(|| hmac_sha256_hex(black_box(secret), black_box(message)));
    });
    group.bench_function("hmac_sha512_hex", |b| {
        b.iter(|| hmac_sha512_hex(black_box(secret), black_box(message)));
    });
    group.bench_function("sha256", |b| {
        b.iter(|| sha256(black_box(message)));
    });
    group.finish();
}

fn bench_parse(c: &mut Criterion) {
    let trade = r#"{"type":"trade","symbol":"BTC/USDT","price":"20123.45","quantity":"0.5","aggressor":"Buy","timestamp":1700000000000}"#;
    let mut group = c.benchmark_group("parse");
    group.bench_function("parse_decimal", |b| {
        b.iter(|| parse_decimal(black_box("20123.456789")));
    });
    group.bench_function("format_decimal", |b| {
        b.iter(|| format_decimal(black_box(dec!(20123.456789))));
    });
    group.bench_function("event_from_json", |b| {
        b.iter(|| serde_json::from_str::<Event>(black_box(trade)).unwrap());
    });
    group.finish();
}

fn bench_filter(c: &mut Criterion) {
    let filters = InstrumentFilters {
        min_quantity: Decimal::ZERO,
        max_quantity: Decimal::ZERO,
        step_size: dec!(0.00001),
        min_price: Decimal::ZERO,
        max_price: Decimal::ZERO,
        tick_size: dec!(0.01),
        min_notional: dec!(10),
    };
    let quantity = dec!(1.234567891234);
    let price = dec!(20123.456789);
    let mut group = c.benchmark_group("filter");
    group.bench_function("round_quantity", |b| {
        b.iter(|| filters.round_quantity(black_box(quantity)));
    });
    group.bench_function("round_price", |b| {
        b.iter(|| filters.round_price(black_box(price)));
    });
    group.finish();
}

fn book_levels(base: i64, count: i64, descending: bool) -> Vec<BookLevel> {
    (0..count)
        .map(|i| {
            let offset = if descending { -i } else { i };
            BookLevel::new(Decimal::from(base + offset), dec!(1))
        })
        .collect()
}

fn bench_orderbook_diff(c: &mut Criterion) {
    let symbol = Symbol::new("BTC", "USDT");
    let snapshot = OrderBookSnapshot {
        symbol: symbol.clone(),
        last_update_id: 1,
        bids: book_levels(20000, 50, true),
        asks: book_levels(20001, 50, false),
    };
    let delta = BookDelta {
        symbol: symbol.clone(),
        first_update_id: 2,
        final_update_id: 3,
        bids: book_levels(19995, 10, true),
        asks: book_levels(20005, 10, false),
    };
    let mut group = c.benchmark_group("orderbook");
    group.bench_function("apply_snapshot", |b| {
        b.iter(|| {
            let mut book = OrderBookBuilder::new(symbol.clone());
            book.apply_snapshot(black_box(&snapshot));
        });
    });
    group.bench_function("apply_delta", |b| {
        let mut book = OrderBookBuilder::new(symbol.clone());
        book.apply_snapshot(&snapshot);
        b.iter(|| book.apply_delta(black_box(&delta)));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_signing,
    bench_parse,
    bench_filter,
    bench_orderbook_diff
);
criterion_main!(benches);
