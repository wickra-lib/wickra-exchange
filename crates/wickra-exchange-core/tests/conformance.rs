//! A shared conformance suite: contracts every `Exchange` implementation must
//! satisfy, exercised without a network.
//!
//! The order-lifecycle contract (place a resting order -> query it -> cancel it)
//! runs against the two fixture-free backends, [`PaperExchange`] and
//! [`ReplayExchange`]. The object-safety + naming contract is checked against all
//! ten venue clients built over the mock transport; each venue's request/parse
//! path is covered by its own module tests.

use rust_decimal_macros::dec;
use wickra_exchange_core::{
    Binance, Bitget, Bybit, Coinbase, Event, Exchange, ExchangeOptions, Gate, Htx, Kraken, KuCoin,
    MarketData, MarketType, MockHttpTransport, Okx, OrderRequest, OrderStatus, PaperExchange,
    ReplayExchange, Symbol, TradePrint, Upbit,
};

/// The order lifecycle every execution backend must honour: a resting limit
/// order is placed, found by query and open-orders, then cancelled.
fn assert_lifecycle(exchange: &mut dyn Exchange, market: &Symbol) {
    let request = OrderRequest::limit_buy(market.clone(), dec!(1), dec!(19000));
    let placed = exchange.place_order(&request).expect("place must succeed");
    assert_eq!(placed.status, OrderStatus::New, "a below-mark limit rests");

    let queried = exchange
        .query_order(market, &placed.id)
        .expect("query must find it");
    assert_eq!(queried.id, placed.id);

    let open = exchange.open_orders(None).expect("open orders");
    assert_eq!(open.len(), 1);

    exchange
        .cancel_order(market, &placed.id)
        .expect("cancel must succeed");
    let after = exchange
        .query_order(market, &placed.id)
        .expect("query after cancel");
    assert_eq!(after.status, OrderStatus::Canceled);
    assert!(exchange.open_orders(None).unwrap().is_empty());

    assert!(!exchange.balances().unwrap().is_empty());
    assert!(!exchange.name().is_empty());
}

fn market() -> Symbol {
    Symbol::new("BTC", "USDT")
}

#[test]
fn paper_exchange_satisfies_the_lifecycle() {
    let mut paper = PaperExchange::new().with_balance("USDT", dec!(100000));
    paper.set_price(&market(), dec!(20000));
    assert_lifecycle(&mut paper, &market());
}

#[test]
fn replay_exchange_satisfies_the_lifecycle() {
    let paper = PaperExchange::new().with_balance("USDT", dec!(100000));
    let frames = vec![Event::Trade(TradePrint {
        symbol: market(),
        price: dec!(20000),
        quantity: dec!(1),
        aggressor: wickra_exchange_core::OrderSide::Buy,
        timestamp: 0,
    })];
    let mut replay = ReplayExchange::with_paper(frames, paper);
    replay.poll_events(); // advance the mark to 20000
    assert_lifecycle(&mut replay, &market());
}

#[test]
fn every_venue_client_is_object_safe_and_named() {
    let options = ExchangeOptions::mainnet(MarketType::Spot);
    let clients: Vec<Box<dyn Exchange>> = vec![
        Box::new(Binance::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Bybit::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Okx::with_http(Box::new(MockHttpTransport::new()), &options)),
        Box::new(Bitget::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(KuCoin::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Gate::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Htx::with_http(Box::new(MockHttpTransport::new()), &options)),
        Box::new(Kraken::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Coinbase::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
        Box::new(Upbit::with_http(
            Box::new(MockHttpTransport::new()),
            &options,
        )),
    ];
    assert_eq!(clients.len(), 10);
    for client in &clients {
        assert!(!client.name().is_empty());
    }
}
