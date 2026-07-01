//! Fetch a live public ticker from a venue (no API keys needed for public
//! market data).
//!
//! Run with: `cargo run -p wickra-exchange-examples --bin ticker -- binance BTC/USDT`

use std::str::FromStr;

use wickra_exchange::{connect, Credentials, ExchangeOptions, MarketType, Symbol};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let venue = args.next().unwrap_or_else(|| "binance".to_string());
    let market = args.next().unwrap_or_else(|| "BTC/USDT".to_string());
    let symbol = Symbol::from_str(&market)?;

    // Public market data needs no real key material.
    let credentials = Credentials::new("", "");
    let options = ExchangeOptions::mainnet(MarketType::Spot);
    let mut exchange = connect(&venue, credentials, &options)?;

    let ticker = exchange.ticker(&symbol)?;
    println!(
        "{venue} {market}: last={} bid={} ask={}",
        ticker.last, ticker.bid, ticker.ask
    );

    Ok(())
}
