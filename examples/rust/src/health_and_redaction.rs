//! Folding the event stream into a health snapshot, and keeping secrets out of
//! whatever you log.
//!
//! Run with: `cargo run -p wickra-exchange-examples --bin health_and_redaction`
//!
//! `Health` is a snapshot, not a subscription: nothing in the library fills it,
//! because the pull-based model already hands the caller every input. The
//! stream reports `Disconnected` / `Reconnected`, each event carries the moment
//! it happened, `sync_time` returns the clock offset, and the rate budget lives
//! in the `ThrottledTransport` the caller wrapped. This is the fold that turns
//! those into one value you can log or serialise.
//!
//! It runs entirely offline: a `ReplayExchange` tape carries `Disconnected` /
//! `Reconnected` exactly as a live stream emits them, so the loop here is the
//! one a live caller writes.

use rust_decimal_macros::dec;
use wickra_exchange::{
    redact, Event, Health, MarketData, OrderSide, PaperExchange, ReplayExchange, Symbol, TradePrint,
};

fn trade(market: &Symbol, price: rust_decimal::Decimal, timestamp: i64) -> Event {
    Event::Trade(TradePrint {
        symbol: market.clone(),
        price,
        quantity: dec!(1),
        aggressor: OrderSide::Buy,
        timestamp,
    })
}

/// The moment an event happened, for the events that carry one. Only
/// `TradePrint` does: tickers, book snapshots and deltas are identified by
/// update id rather than time, and the control events (`Disconnected`,
/// `Reconnected`, `Subscribed`) are observed rather than timestamped by the
/// venue. For all of those, the caller's own clock stands in.
fn event_time_ms(event: &Event) -> Option<i64> {
    match event {
        Event::Trade(print) => Some(print.timestamp),
        _ => None,
    }
}

/// Apply one event to a running [`Health`].
fn apply(health: &mut Health, event: &Event, now_ms: i64) {
    match event {
        Event::Disconnected => health.connected = false,
        Event::Reconnected => {
            health.connected = true;
            health.reconnects += 1;
        }
        other => {
            // Any data event is proof the stream is alive.
            health.connected = true;
            health.last_message_ms = Some(event_time_ms(other).unwrap_or(now_ms));
        }
    }
}

fn main() {
    let market = Symbol::new("BTC", "USDT");

    let paper = PaperExchange::new().with_balance("USDT", dec!(100_000));
    let tape = vec![
        trade(&market, dec!(20000), 1_000),
        trade(&market, dec!(20050), 2_000),
        // The peer closes; the client reconnects and replays its subscriptions.
        Event::Disconnected,
        Event::Reconnected,
        trade(&market, dec!(20100), 9_000),
    ];
    let mut exchange = ReplayExchange::with_paper(tape, paper);

    let mut health = Health {
        // On a live client this is what `sync_time` returned: the offset
        // between this machine's clock and the venue's, in milliseconds.
        clock_offset_ms: -12,
        // And this is `WeightedRateLimiter::used()` on the budget inside the
        // `ThrottledTransport` the caller built the client with.
        rate_budget_used: 40,
        ..Health::default()
    };

    let mut now_ms = 1_000;
    loop {
        let events = exchange.poll_events();
        if events.is_empty() {
            break;
        }
        for event in &events {
            apply(&mut health, event, now_ms);
            now_ms = health.last_message_ms.unwrap_or(now_ms);
        }
    }

    // A monitoring loop asks two questions of the snapshot, and both are
    // relative to *now* rather than to the last message: a stream that stopped
    // delivering is still `connected`.
    let checked_at = 10_000;
    println!("connected:    {}", health.connected);
    println!("reconnects:   {}", health.reconnects);
    println!("clock offset: {} ms", health.clock_offset_ms);
    println!("rate budget:  {} used", health.rate_budget_used);
    println!("staleness:    {:?} ms", health.staleness_ms(checked_at));
    println!("healthy(2s):  {}", health.is_healthy(checked_at, 2_000));
    println!("healthy(0.5s): {}", health.is_healthy(checked_at, 500));

    assert!(health.connected);
    assert_eq!(health.reconnects, 1);
    assert_eq!(health.staleness_ms(checked_at), Some(1_000));
    assert!(health.is_healthy(checked_at, 2_000));
    // A second of silence is within a two-second tolerance and outside a
    // half-second one. The socket is open either way: `connected` alone never
    // catches a stream that stopped delivering.
    assert!(!health.is_healthy(checked_at, 500));

    // Whatever this snapshot ends up in — a log line, an alert, a status
    // endpoint — the request that produced it must not carry credentials into
    // it. `redact` takes the secret out of a string before it is written; an
    // empty secret is ignored, so it is safe to call unconditionally.
    let api_secret = "s3cr3t-signing-key";
    let line = format!("GET /api/v3/account signed with {api_secret} -> 200");
    println!("{}", redact(&line, api_secret));
    assert!(!redact(&line, api_secret).contains(api_secret));
}
