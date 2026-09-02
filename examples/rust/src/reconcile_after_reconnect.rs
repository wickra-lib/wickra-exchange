//! Reconciling order state after a dropped stream.
//!
//! Run with: `cargo run -p wickra-exchange-examples --bin reconcile_after_reconnect`
//!
//! While a stream is down, an order can fill or cancel without the client ever
//! seeing the update. The library cannot resolve this alone: it does not know
//! what *you* believe is open, only what the venue currently lists. So it gives
//! you the two halves — `Event::Reconnected` to know when to look, and
//! `reconcile_orders` to compare — and this is the loop that joins them.
//!
//! It runs entirely offline. A `ReplayExchange` is driven by a tape of events,
//! and a tape can contain `Disconnected` / `Reconnected` just as a live stream
//! emits them, so the control flow here is exactly the one a live caller
//! writes.

use rust_decimal_macros::dec;
use wickra_exchange::{
    reconcile_orders, Event, Execution, MarketData, OrderRequest, OrderSide, PaperExchange,
    ReplayExchange, Symbol, TradePrint,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let market = Symbol::new("BTC", "USDT");

    let paper = PaperExchange::new().with_balance("USDT", dec!(100000));
    let tape = vec![
        trade(&market, dec!(20000), 1),
        // The stream drops here. Anything that happens to the account in
        // between is invisible to this client.
        Event::Disconnected,
        Event::Reconnected,
        trade(&market, dec!(20100), 2),
    ];
    let mut exchange = ReplayExchange::with_paper(tape, paper);

    // One frame first: a paper book fills against a mark price, and the mark
    // comes from the tape. Nothing can be placed into a market that has not
    // printed yet.
    exchange.poll_events();

    // Two resting orders, and this client remembers both as open.
    let first = exchange.place_order(&OrderRequest::limit_buy(
        market.clone(),
        dec!(1),
        dec!(19000),
    ))?;
    let second = exchange.place_order(&OrderRequest::limit_buy(
        market.clone(),
        dec!(1),
        dec!(18000),
    ))?;
    let believed_open = vec![first.id.clone(), second.id.clone()];
    println!("believed open: {believed_open:?}");

    // Stand in for what happens unseen while the stream is down: the second
    // order leaves the book. On a live venue this would be a fill or a
    // cancellation whose update never arrived.
    exchange.cancel_order(&market, &second.id)?;

    loop {
        let events = exchange.poll_events();
        if events.is_empty() {
            break;
        }
        for event in events {
            if !matches!(event, Event::Reconnected) {
                continue;
            }
            // The stream is back. Ask the venue what it actually has, and
            // compare it against what this client believed.
            let venue_open = exchange.open_orders(None)?;
            let diff = reconcile_orders(&believed_open, &venue_open);

            println!("reconnected — reconciling");
            println!("  still open: {:?}", diff.still_open);
            println!("  appeared:   {:?}", diff.appeared);
            println!("  vanished:   {:?}", diff.vanished);

            // `vanished` is the half that matters: the client thinks these are
            // working, and the venue does not list them. Each one filled or
            // cancelled unseen, so its final state has to be fetched before any
            // position or risk figure derived from it can be trusted.
            for id in &diff.vanished {
                let order = exchange.query_order(&market, id)?;
                println!(
                    "  {id} was not open after all: status {:?}, filled {}",
                    order.status, order.filled_quantity
                );
            }
            assert!(diff.has_divergence(), "the cancelled order must show up");
        }
    }

    Ok(())
}
