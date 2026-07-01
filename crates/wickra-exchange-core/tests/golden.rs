//! Golden-fixture parity: drive committed replay tapes through a `ReplayExchange`
//! running a fixed SMA strategy and assert the fill and balances match the
//! expected files exactly, so the deterministic replay-to-paper-fill pipeline can
//! never drift silently. Fixtures live in the repo-root `golden/` directory.

use std::fs;
use std::str::FromStr;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::Value;
use wickra_core::{Indicator, Sma};
use wickra_exchange_core::{
    Event, Execution, MarketData, OrderRequest, OrderSide, PaperExchange, ReplayExchange, Symbol,
    TradePrint,
};

fn golden_dir() -> String {
    format!("{}/../../golden", env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn dec(value: &Value) -> Decimal {
    Decimal::from_f64_retain(value.as_f64().unwrap()).unwrap()
}

fn run_case(name: &str) {
    let dir = golden_dir();
    let input = read_json(&format!("{dir}/replay/{name}.json"));
    let expected = read_json(&format!("{dir}/expected/{name}.json"));

    let symbol = Symbol::from_str(input["market"].as_str().unwrap()).unwrap();
    let tape: Vec<f64> = input["tape"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let period = usize::try_from(input["sma_period"].as_u64().unwrap()).unwrap();

    let mut paper = PaperExchange::new()
        .with_fees(dec(&input["maker_bps"]), dec(&input["taker_bps"]))
        .with_slippage_bps(dec(&input["slippage_bps"]));
    for (asset, amount) in input["balances"].as_object().unwrap() {
        paper = paper.with_balance(asset, dec(amount));
    }

    let frames: Vec<Event> = tape
        .iter()
        .enumerate()
        .map(|(i, &price)| {
            Event::Trade(TradePrint {
                symbol: symbol.clone(),
                price: Decimal::from_f64_retain(price).unwrap(),
                quantity: Decimal::ONE,
                aggressor: OrderSide::Buy,
                timestamp: i64::try_from(i).unwrap(),
            })
        })
        .collect();
    let mut exchange = ReplayExchange::with_paper(frames, paper);

    let mut sma = Sma::new(period).unwrap();
    let mut fill_price: Option<f64> = None;

    while !exchange.is_finished() {
        for event in exchange.poll_events() {
            let Event::Trade(trade) = event else { continue };
            let price = trade.price.to_f64().unwrap();
            if let Some(mean) = sma.update(price) {
                if fill_price.is_none() && price > mean {
                    let order = exchange
                        .place_order(&OrderRequest::market_buy(symbol.clone(), Decimal::ONE))
                        .unwrap();
                    fill_price = order.average_price.map(|p| p.to_f64().unwrap());
                }
            }
        }
    }

    let balances: std::collections::HashMap<String, f64> = exchange
        .balances()
        .unwrap()
        .into_iter()
        .map(|b| (b.asset, b.free.to_f64().unwrap()))
        .collect();

    assert_eq!(fill_price.is_some(), expected["filled"].as_bool().unwrap());
    let tol = 1e-6;
    assert!((fill_price.unwrap() - expected["average_price"].as_f64().unwrap()).abs() < tol);
    assert!((balances["BTC"] - expected["btc"].as_f64().unwrap()).abs() < tol);
    assert!((balances["USDT"] - expected["usdt"].as_f64().unwrap()).abs() < tol);
}

#[test]
fn golden_sma_cross_frictionless() {
    run_case("sma_cross");
}

#[test]
fn golden_sma_cross_with_costs() {
    run_case("sma_cross_with_costs");
}
