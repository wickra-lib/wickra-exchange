//! A shared conformance suite: contracts every `Exchange` implementation must
//! satisfy, exercised without a network.
//!
//! The order-lifecycle contract (place a resting order -> query it -> cancel it)
//! runs against the two fixture-free backends, [`PaperExchange`] and
//! [`ReplayExchange`]. The object-safety + naming contract is checked against all
//! ten venue clients built over the mock transport; each venue's request/parse
//! path is covered by its own module tests.

use rust_decimal_macros::dec;
use std::sync::Arc;
use wickra_exchange_core::{
    Binance, Bitget, Bybit, Coinbase, Credentials, Error, Event, Exchange, ExchangeOptions, Gate,
    HttpRequest, HttpResponse, HttpTransport, Htx, Kraken, KuCoin, MarketData, MarketType,
    MockHttpTransport, Okx, OrderRequest, OrderStatus, OrderType, PaperExchange, ReplayExchange,
    Result, Symbol, TradePrint, Upbit, WsExecution, WsUserData,
};

/// Share one [`MockHttpTransport`] between the test and the client that owns it,
/// so the requests a client built can be read back after the call.
struct ArcTransport(Arc<MockHttpTransport>);

impl HttpTransport for ArcTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.0.execute(request)
    }
}

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

/// The eight trading venues: each is object-safe as both `WsUserData` (private
/// account/order stream) and `WsExecution` (WebSocket order placement). Building
/// these `Box<dyn _>` vectors is itself the object-safety assertion.
#[test]
fn every_trading_venue_is_object_safe_as_ws_user_data_and_ws_execution() {
    let options = ExchangeOptions::mainnet(MarketType::Spot);
    let mock = || Box::new(MockHttpTransport::new());

    let user_data: Vec<Box<dyn WsUserData>> = vec![
        Box::new(Binance::with_http(mock(), &options)),
        Box::new(Bybit::with_http(mock(), &options)),
        Box::new(Okx::with_http(mock(), &options)),
        Box::new(Bitget::with_http(mock(), &options)),
        Box::new(KuCoin::with_http(mock(), &options)),
        Box::new(Gate::with_http(mock(), &options)),
        Box::new(Htx::with_http(mock(), &options)),
        Box::new(Kraken::with_http(mock(), &options)),
    ];
    let execution: Vec<Box<dyn WsExecution>> = vec![
        Box::new(Binance::with_http(mock(), &options)),
        Box::new(Bybit::with_http(mock(), &options)),
        Box::new(Okx::with_http(mock(), &options)),
        Box::new(Bitget::with_http(mock(), &options)),
        Box::new(KuCoin::with_http(mock(), &options)),
        Box::new(Gate::with_http(mock(), &options)),
        Box::new(Htx::with_http(mock(), &options)),
        Box::new(Kraken::with_http(mock(), &options)),
    ];
    assert_eq!(user_data.len(), 8);
    assert_eq!(execution.len(), 8);

    // `WsUserData: MarketData`, so a boxed user-data client can poll directly.
    let mut user_data = user_data;
    for client in &mut user_data {
        assert!(client.poll_events().is_empty());
    }
}

/// A trigger order either carries its trigger price to the venue, or is
/// refused. It must never be sent as a plain order.
///
/// This is the contract nine of the ten clients were breaking. `OrderRequest`
/// validates that a `StopMarket`/`StopLimit` carries a `stop_price`, and then
/// every path but `Binance::place_order` dropped it and mapped the type down to
/// `market`/`limit` -- so a stop-loss at 19000 was sent as a market order that
/// executed at once, at the price it existed to protect against.
///
/// The assertion is deliberately shaped as "sent with a trigger, or refused",
/// not as a list of which venues do which: a venue that gains native trigger
/// orders later moves from one branch to the other without touching this test,
/// and a venue added without either is caught.
#[test]
fn a_trigger_order_is_either_carried_or_refused_but_never_flattened() {
    // What the venue answered is irrelevant here -- the mock replies `{}` and
    // several clients then fail to parse it. What matters is what went out.
    fn assert_contract(name: &str, outcome: &Result<wickra_exchange_core::Order>, sent: &[String]) {
        let refused = matches!(outcome, Err(Error::Exchange { code, .. }) if code == "unsupported");
        if refused {
            assert!(
                sent.is_empty(),
                "{name}: refused, yet sent a request anyway"
            );
            return;
        }
        assert!(
            !sent.is_empty(),
            "{name}: neither refused the trigger order nor sent one"
        );
        let carried = sent
            .iter()
            .any(|req| req.contains("stopPrice") || req.contains("triggerPx"));
        assert!(
            carried,
            "{name}: sent a trigger order with no trigger price; it would execute now"
        );
    }

    let stop = |market: &Symbol| OrderRequest {
        order_type: OrderType::StopMarket,
        stop_price: Some(dec!(19000)),
        ..OrderRequest::market_sell(market.clone(), dec!(1))
    };
    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "SECRET")
            .with_passphrase("PASS")
            .with_private_key(
                "-----BEGIN EC PRIVATE KEY-----
x
-----END EC PRIVATE KEY-----",
            )
    };
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! check {
        ($name:literal, $venue:ident) => {{
            let mock = Arc::new(MockHttpTransport::new());
            // Enough of a response that a client which *accepts* the order gets
            // far enough to have sent its request; the contract is about what
            // went out, not about what came back.
            mock.push_json(200, "{}");
            let client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let outcome = client.place_order(&stop(&market));
            let sent: Vec<String> = mock
                .recorded_requests()
                .iter()
                .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                .collect();
            assert_contract($name, &outcome, &sent);
        }};
    }

    check!("Binance", Binance);
    check!("Bybit", Bybit);
    check!("OKX", Okx);
    check!("Bitget", Bitget);
    check!("KuCoin", KuCoin);
    check!("Gate.io", Gate);
    check!("HTX", Htx);
    check!("Kraken", Kraken);
    check!("Coinbase", Coinbase);
    check!("Upbit", Upbit);
}
