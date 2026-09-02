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
    AdvancedOrders, Binance, Bitget, Bybit, Coinbase, Credentials, Error, Event, Exchange,
    ExchangeOptions, Gate, HttpRequest, HttpResponse, HttpTransport, Htx, Kraken, KuCoin,
    MarketData, MarketType, MockHttpTransport, Okx, OrderRequest, OrderStatus, OrderType,
    PaperExchange, ReplayExchange, Result, SelfTradePrevention, Symbol, TimeInForce, TradePrint,
    Upbit, WsExecution, WsUserData,
};

/// An EC private key, so the Coinbase client can sign the ES256 JWT its order
/// path builds. A client that cannot sign never reaches the wire, and a contract
/// about what goes out cannot be tested on one.
const EC_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgZZ/YugITtxORUz74
wHvqY4aizCFHQFTVNQCzDGy8/TOhRANCAAS69zNVQjOQ4RgxJVI8esP+jMfHLSTw
2iVqo0qWlda/1D2jN4O3zcv4juQF5iE4pU5qPkeECTgsKSIYwZaMVMyO
-----END PRIVATE KEY-----
";

/// Every spelling that proves post-only reached the wire, across all ten venues.
const POST_ONLY_SPELLINGS: &[&str] = &[
    "post_only",
    "postonly",
    "limit_maker",
    "limit-maker",
    "oflags=post",
    "\"poc\"",
];

/// Every spelling that proves an immediate-or-cancel reached the wire.
const IOC_SPELLINGS: &[&str] = &["ioc"];

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

/// The same contract on the other two order paths.
///
/// `place_order` is not the only way an order reaches a venue: there is a batch
/// endpoint on all eight trading venues and a WebSocket order API on five. The
/// guard has to be on each of them, because a trigger order that slips through
/// the batch path is exactly as immediate as one that slips through the single
/// path -- and the batch builders were where the earlier `reduce_only` drop hid
/// too.
///
/// Binance is checked in its own module tests instead: it *carries* the trigger
/// on all three paths, so it needs a live response and a WebSocket transport
/// rather than this refusal assertion.
#[test]
fn the_batch_and_websocket_paths_refuse_triggers_too() {
    fn stop(market: &Symbol) -> OrderRequest {
        OrderRequest {
            order_type: OrderType::StopMarket,
            stop_price: Some(dec!(19000)),
            ..OrderRequest::market_sell(market.clone(), dec!(1))
        }
    }
    fn refused(outcome: &Error) -> bool {
        matches!(outcome, Error::Exchange { code, .. } if code == "unsupported")
    }

    let market = market();
    let creds = || Credentials::new("APIKEY", "SECRET").with_passphrase("PASS");
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! batch {
        ($name:literal, $venue:ident) => {{
            let mock = Arc::new(MockHttpTransport::new());
            let mut client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let err = AdvancedOrders::place_batch(&mut client, &[stop(&market)])
                .expect_err(concat!($name, ": batch accepted a trigger order"));
            assert!(
                refused(&err),
                concat!($name, ": batch refused for the wrong reason")
            );
            assert!(
                mock.recorded_requests().is_empty(),
                concat!($name, ": batch refused, yet sent a request")
            );
        }};
    }

    batch!("Bybit", Bybit);
    batch!("OKX", Okx);
    batch!("Bitget", Bitget);
    batch!("KuCoin", KuCoin);
    batch!("Gate.io", Gate);
    batch!("HTX", Htx);
    batch!("Kraken", Kraken);

    // The WebSocket paths refuse before they open a connection, so no transport
    // is attached here: reaching the guard is the whole point.
    macro_rules! ws {
        ($name:literal, $venue:ident) => {{
            let mock = Arc::new(MockHttpTransport::new());
            let mut client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let err = WsExecution::place_order_ws(&mut client, &stop(&market))
                .expect_err(concat!($name, ": ws accepted a trigger order"));
            assert!(
                refused(&err),
                concat!($name, ": ws refused for the wrong reason")
            );
        }};
    }

    ws!("Bybit", Bybit);
    ws!("OKX", Okx);
    ws!("Gate.io", Gate);
    ws!("Kraken", Kraken);
}

/// Every field the caller set on an order either reaches the venue or refuses
/// the order. None of them may be dropped on the way.
///
/// This is [`a_trigger_order_is_either_carried_or_refused_but_never_flattened`]
/// widened from one field to the class it belongs to. The trigger contract was
/// written for `stop_price` after nine clients were found flattening it, but the
/// same shape of defect sat on the rest of the request and no test looked: seven
/// of the ten clients never sent `time_in_force` at all, so an `Ioc` -- "fill
/// what you can now, cancel the rest" -- was placed as the resting `Gtc` the
/// caller had asked it never to be, and four dropped the self-trade-prevention
/// policy the same way.
///
/// A field a venue genuinely cannot express is a legitimate answer; sending the
/// order anyway is not, because an order missing a field the caller set is a
/// different order, not a smaller one. So the assertion is again "carried, or
/// refused" -- a venue that gains a field later moves from one branch to the
/// other without this test changing, and a venue added without either is caught.
#[test]
fn every_order_field_is_either_carried_or_refused_but_never_dropped() {
    /// One field under test: how to set it on a request, and every spelling that
    /// proves it reached the wire. Matching is case-insensitive, so `IOC`,
    /// `ioc` and `Ioc` are one entry.
    struct Field {
        name: &'static str,
        apply: fn(OrderRequest) -> OrderRequest,
        spellings: &'static [&'static str],
    }

    const FIELDS: &[Field] = &[
        Field {
            name: "time_in_force = Ioc",
            apply: |r| r.with_time_in_force(TimeInForce::Ioc),
            spellings: &["ioc"],
        },
        Field {
            name: "time_in_force = Fok",
            apply: |r| r.with_time_in_force(TimeInForce::Fok),
            spellings: &["fok"],
        },
        Field {
            name: "post_only",
            apply: OrderRequest::post_only,
            spellings: &[
                "post_only",
                "postonly",
                "limit_maker",
                "limit-maker",
                "oflags=post",
                "\"poc\"",
            ],
        },
        Field {
            name: "stp",
            apply: |r| r.with_stp(SelfTradePrevention::ExpireMaker),
            spellings: &[
                "stpmode",
                "smptype",
                "selftradeprevention",
                "stp_act",
                "\"stp\"",
            ],
        },
    ];

    fn assert_contract(
        venue: &str,
        field: &Field,
        outcome: &Result<wickra_exchange_core::Order>,
        sent: &[String],
    ) {
        let refused = matches!(outcome, Err(Error::Exchange { code, .. }) if code == "unsupported");
        if refused {
            assert!(
                sent.is_empty(),
                "{venue}/{}: refused, yet sent a request anyway",
                field.name
            );
            return;
        }
        assert!(
            !sent.is_empty(),
            "{venue}/{}: neither refused the field nor sent an order",
            field.name
        );
        let wire = sent.join(" ").to_lowercase();
        let carried = field
            .spellings
            .iter()
            .any(|spelling| wire.contains(spelling));
        assert!(
            carried,
            "{venue}/{}: order went out without the field; the venue will place \
             a different order than the caller asked for.\nwire: {wire}",
            field.name
        );
    }

    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "c2VjcmV0")
            .with_passphrase("PASS")
            .with_private_key(EC_KEY)
    };
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! check {
        ($name:literal, $venue:ident) => {{
            for field in FIELDS {
                let mock = Arc::new(MockHttpTransport::new());
                // Enough replies, and enough shape, for the clients that fetch
                // something before they can build an order (HTX resolves its
                // spot account id first) to reach the order call at all. What
                // came back is irrelevant; what went out is the contract.
                for _ in 0..3 {
                    mock.push_json(
                        200,
                        r#"{"status":"ok","code":"0","data":[{"id":42,"type":"spot","state":"working"}]}"#,
                    );
                }
                let client = $venue::with_credentials(
                    Box::new(ArcTransport(Arc::clone(&mock))),
                    &options,
                    creds(),
                );
                // A limit order, so that the time-in-force is meaningful: a
                // market order is immediate by construction and several venues
                // legitimately have no separate spelling for it.
                let request =
                    (field.apply)(OrderRequest::limit_buy(market.clone(), dec!(1), dec!(100)));
                let outcome = client.place_order(&request);
                // The order request is the last one out; anything before it is
                // the client fetching what it needed to build the order.
                let sent: Vec<String> = mock
                    .recorded_requests()
                    .last()
                    .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                    .into_iter()
                    .collect();
                assert_contract($name, field, &outcome, &sent);
            }
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

/// The same field contract on the batch path.
///
/// A batch builder is a second, hand-written copy of the order builder, and the
/// two drift: PR #189 found Bitget's batch dropping `reduce_only` while its
/// single-order path carried it, and this audit found the same shape on six
/// more venues -- Gate's batch dropped `time_in_force`, `post_only` **and** the
/// STP policy that its own `place_order` sends. A caller does not expect an
/// order to mean something different because it travelled in a batch.
#[test]
fn the_batch_path_carries_every_field_too() {
    struct Field {
        name: &'static str,
        apply: fn(OrderRequest) -> OrderRequest,
        spellings: &'static [&'static str],
    }

    const FIELDS: &[Field] = &[
        Field {
            name: "time_in_force = Ioc",
            apply: |r| r.with_time_in_force(TimeInForce::Ioc),
            spellings: &["ioc"],
        },
        Field {
            name: "post_only",
            apply: OrderRequest::post_only,
            spellings: &[
                "post_only",
                "postonly",
                "limit_maker",
                "limit-maker",
                "oflags%5d=post",
                "oflags]=post",
                "\"poc\"",
            ],
        },
        Field {
            name: "stp",
            apply: |r| r.with_stp(SelfTradePrevention::ExpireMaker),
            spellings: &[
                "stpmode",
                "smptype",
                "selftradeprevention",
                "stp_act",
                "\"stp\"",
            ],
        },
    ];

    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "c2VjcmV0")
            .with_passphrase("PASS")
            .with_private_key(EC_KEY)
    };
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! check {
        ($name:literal, $venue:ident) => {{
            for field in FIELDS {
                let mock = Arc::new(MockHttpTransport::new());
                for _ in 0..3 {
                    mock.push_json(
                        200,
                        r#"{"status":"ok","code":"0","data":[{"id":42,"type":"spot","state":"working"}]}"#,
                    );
                }
                let mut client = $venue::with_credentials(
                    Box::new(ArcTransport(Arc::clone(&mock))),
                    &options,
                    creds(),
                );
                let request =
                    (field.apply)(OrderRequest::limit_buy(market.clone(), dec!(1), dec!(100)));
                let outcome = AdvancedOrders::place_batch(&mut client, &[request]);
                let refused = matches!(
                    &outcome,
                    Err(Error::Exchange { code, .. }) if code == "unsupported"
                );
                let sent: Vec<String> = mock
                    .recorded_requests()
                    .last()
                    .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                    .into_iter()
                    .collect();
                if refused {
                    assert!(
                        sent.is_empty(),
                        "{}/{}: batch refused, yet sent a request anyway",
                        $name,
                        field.name
                    );
                    continue;
                }
                assert!(
                    !sent.is_empty(),
                    "{}/{}: batch neither refused the field nor sent an order",
                    $name,
                    field.name
                );
                let wire = sent.join(" ").to_lowercase();
                assert!(
                    field.spellings.iter().any(|s| wire.contains(s)),
                    "{}/{}: batch went out without the field, while the \
                     single-order path carries it.\nwire: {wire}",
                    $name,
                    field.name
                );
            }
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
}

/// Two fields that a venue spells in *one* slot are refused together, never
/// resolved by dropping one.
///
/// Several venues carry post-only and the time-in-force in a single field:
/// Bybit's `timeInForce` takes `PostOnly` beside `IOC`, Bitget's `force` and
/// Gate's `time_in_force` take `post_only`/`poc` the same way, OKX and HTX fold
/// both into the order *type*, and Binance's post-only type accepts no
/// time-in-force at all. Asking for both is asking for two things in one slot.
///
/// Those builders used to answer by picking one and discarding the other in
/// silence -- `post_only` won, and an `Ioc` vanished -- which placed a resting
/// order where the caller had asked for one that must not rest. Where a venue
/// has room for both (Kraken spells post-only in `oflags`, KuCoin in its own
/// `postOnly` field) the order goes out carrying both; where it does not, the
/// order is refused.
#[test]
fn two_fields_in_one_venue_slot_are_refused_together_not_silently_resolved() {
    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "c2VjcmV0")
            .with_passphrase("PASS")
            .with_private_key(EC_KEY)
    };
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! check {
        ($name:literal, $venue:ident) => {{
            let mock = Arc::new(MockHttpTransport::new());
            for _ in 0..3 {
                mock.push_json(
                    200,
                    r#"{"status":"ok","code":"0","data":[{"id":42,"type":"spot","state":"working"}]}"#,
                );
            }
            let client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let request = OrderRequest::limit_buy(market.clone(), dec!(1), dec!(100))
                .post_only()
                .with_time_in_force(TimeInForce::Ioc);
            let outcome = client.place_order(&request);
            let sent: Vec<String> = mock
                .recorded_requests()
                .last()
                .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                .into_iter()
                .collect();
            if matches!(&outcome, Err(Error::Exchange { code, .. }) if code == "unsupported") {
                assert!(
                    sent.is_empty(),
                    concat!($name, ": refused the pair, yet sent a request anyway")
                );
            } else {
                let wire = sent.join(" ").to_lowercase();
                assert!(
                    POST_ONLY_SPELLINGS.iter().any(|s| wire.contains(s))
                        && IOC_SPELLINGS.iter().any(|s| wire.contains(s)),
                    "{}: accepted post_only + Ioc but sent only one of them.\nwire: {wire}",
                    $name
                );
            }
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

/// A market order that asks for fill-or-kill is refused wherever the venue's
/// market order has no such spelling.
///
/// A market order is immediate by construction, so GTC (the request default)
/// and IOC both describe what it already does and need no field. FOK does not:
/// "fill all of it now or none of it" is a different instruction, and most
/// venues express it only on a limit order -- Gate's futures market order *is*
/// a zero-price IOC, HTX's is `optimal_5`, Coinbase's is `market_market_ioc`.
/// Placing the order without the FOK would let a partial fill stand where the
/// caller asked for all-or-nothing.
#[test]
fn a_market_order_asking_for_fill_or_kill_is_refused_where_it_cannot_be_said() {
    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "c2VjcmV0")
            .with_passphrase("PASS")
            .with_private_key(EC_KEY)
    };
    let options = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! check {
        ($name:literal, $venue:ident) => {{
            let mock = Arc::new(MockHttpTransport::new());
            for _ in 0..3 {
                mock.push_json(
                    200,
                    r#"{"status":"ok","code":"0","data":[{"id":42,"type":"spot","state":"working"}]}"#,
                );
            }
            let client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let request = OrderRequest::market_buy(market.clone(), dec!(1))
                .with_time_in_force(TimeInForce::Fok);
            let outcome = client.place_order(&request);
            let sent: Vec<String> = mock
                .recorded_requests()
                .last()
                .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                .into_iter()
                .collect();
            if matches!(&outcome, Err(Error::Exchange { code, .. }) if code == "unsupported") {
                assert!(
                    sent.is_empty(),
                    concat!($name, ": refused the FOK, yet sent a request anyway")
                );
            } else {
                let wire = sent.join(" ").to_lowercase();
                assert!(
                    wire.contains("fok"),
                    "{}: accepted a FOK market order but sent no fill-or-kill.\nwire: {wire}",
                    $name
                );
            }
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
