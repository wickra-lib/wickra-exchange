//! The differentiator demo: an offline paper account fills orders deterministically
//! through the same `Exchange` API a live venue uses.
//!
//! Run with: `cargo run -p wickra-exchange-examples --bin paper_trade`

use rust_decimal_macros::dec;
use wickra_exchange::{Exchange, Execution, MarketData, OrderRequest, PaperExchange, Symbol};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let market = Symbol::new("BTC", "USDT");

    let mut exchange = PaperExchange::new()
        .with_fees(dec!(1), dec!(5)) // maker / taker basis points
        .with_slippage_bps(dec!(10))
        .with_balance("USDT", dec!(100000));
    exchange.set_price(&market, dec!(20000));

    println!("venue: {}", exchange.name());

    let order = exchange.place_order(&OrderRequest::market_buy(market.clone(), dec!(1)))?;
    println!(
        "filled {} {} at {:?} (status {:?})",
        order.filled_quantity, market, order.average_price, order.status
    );

    for balance in exchange.balances()? {
        println!("  {} free: {}", balance.asset, balance.free);
    }

    // Execution events flow through the same pull loop as market data.
    for event in exchange.poll_events() {
        println!("event: {event:?}");
    }

    Ok(())
}
