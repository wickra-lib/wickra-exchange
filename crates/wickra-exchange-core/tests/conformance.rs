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
    MarketData, MarketType, MockHttpTransport, MockWsTransport, Okx, OrderRequest, OrderStatus,
    OrderType, PaperExchange, ReplayExchange, Result, SelfTradePrevention, Symbol, TimeInForce,
    TradePrint, Upbit, WsConnection, WsExecution, WsTransport, WsUserData,
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

/// The client order id the field contracts send.
///
/// Distinctive enough that finding it on the wire means the client put it there,
/// and lowercase so it survives the case-insensitive match. Venues that decorate
/// it -- Gate prefixes `t-` -- still contain it.
const CLIENT_ORDER_ID: &str = "conformance-7f3a";

/// Share one [`MockHttpTransport`] between the test and the client that owns it,
/// so the requests a client built can be read back after the call.
struct ArcTransport(Arc<MockHttpTransport>);

impl HttpTransport for ArcTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.0.execute(request)
    }
}

/// The same sharing for a [`MockWsTransport`], so the frames a client sent can
/// be read back after the call.
struct ArcWs(Arc<MockWsTransport>);

impl WsTransport for ArcWs {
    fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>> {
        self.0.connect(url)
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
        // Matched by the value, not the key. Kraken's trigger types move what
        // `price` means, so its trigger rides in a field every limit order also
        // has -- no list of spellings can tell the two apart. The request is a
        // market sell with no limit price, so 19000 on the wire is the trigger.
        let carried = sent.iter().any(|req| req.contains("19000"));
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
        // Base64, because Kraken's secret is decoded before it signs: a secret
        // that is not base64 fails before the request is built, and the client
        // then looks as though it refused the order.
        Credentials::new("APIKEY", "c2VjcmV0")
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
        // Matched by the value rather than by the key. Ten venues spell the key
        // ten ways -- `newClientOrderId`, `orderLinkId`, `clOrdId`, `clientOid`,
        // `text`, `cl_ord_id`, `identifier` -- and a list of keys is a list to
        // keep in step with ten clients. The id itself is on the wire whatever
        // the key is called, and a venue that prefixes it (Gate sends `t-`)
        // still contains it.
        Field {
            name: "client_order_id",
            apply: |r| r.with_client_order_id(CLIENT_ORDER_ID),
            spellings: &[CLIENT_ORDER_ID],
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

/// The same field contract on the WebSocket order path.
///
/// The third hand-written copy of the order builder, and until now the least
/// watched: the WebSocket path was under contract for `stop_price` alone
/// ([`the_batch_and_websocket_paths_refuse_triggers_too`]), while the single
/// order and the batch had every field held to account. A frame is a different
/// protocol from the REST body beside it -- Kraken's v2 socket names the same
/// fields `order_qty`, `limit_price` and `cl_ord_id`, so nothing carries over
/// from the REST spelling by accident -- which is exactly the condition under
/// which a field goes missing on one path and not the other.
///
/// What goes out is the contract, so the reply is not scripted: the client sends
/// its frame and then fails to read an answer, and the frame it sent is already
/// recorded. That keeps this test independent of ten venue-specific response
/// shapes, which are each venue's own parse tests to prove.
#[test]
fn the_websocket_path_carries_every_field_too() {
    struct Field {
        name: &'static str,
        apply: fn(OrderRequest) -> OrderRequest,
        spellings: &'static [&'static str],
    }

    const FIELDS: &[Field] = &[
        Field {
            name: "time_in_force = Ioc",
            apply: |r| r.with_time_in_force(TimeInForce::Ioc),
            spellings: IOC_SPELLINGS,
        },
        Field {
            name: "post_only",
            apply: OrderRequest::post_only,
            spellings: POST_ONLY_SPELLINGS,
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
        Field {
            name: "client_order_id",
            apply: |r| r.with_client_order_id(CLIENT_ORDER_ID),
            spellings: &[CLIENT_ORDER_ID],
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
                let http = Arc::new(MockHttpTransport::new());
                // Kraken buys a WebSocket token over REST before it may send an
                // order frame; the others need nothing. Enough replies for that,
                // shaped so the token parse succeeds.
                for _ in 0..3 {
                    http.push_json(
                        200,
                        r#"{"error":[],"result":{"token":"WSTOKEN","expires":900}}"#,
                    );
                }
                let ws = Arc::new(MockWsTransport::new());
                let mut client = $venue::with_credentials(
                    Box::new(ArcTransport(Arc::clone(&http))),
                    &options,
                    creds(),
                )
                .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
                let request =
                    (field.apply)(OrderRequest::limit_buy(market.clone(), dec!(1), dec!(100)));
                let outcome = WsExecution::place_order_ws(&mut client, &request);
                let refused = matches!(
                    &outcome,
                    Err(Error::Exchange { code, .. }) if code == "unsupported"
                );
                // The order frame is the last one out; a login or auth frame in
                // front of it is the client getting ready to send it.
                let sent: Vec<String> = ws.sent().last().cloned().into_iter().collect();
                if refused {
                    assert!(
                        sent.is_empty(),
                        "{}/{}: ws refused, yet sent a frame anyway",
                        $name,
                        field.name
                    );
                    continue;
                }
                assert!(
                    !sent.is_empty(),
                    "{}/{}: ws neither refused the field nor sent an order",
                    $name,
                    field.name
                );
                let wire = sent.join(" ").to_lowercase();
                assert!(
                    field.spellings.iter().any(|s| wire.contains(s)),
                    "{}/{}: the order frame went out without the field, while the \
                     single-order path carries it.\nframe: {wire}",
                    $name,
                    field.name
                );
            }
        }};
    }

    check!("Binance", Binance);
    check!("Bybit", Bybit);
    check!("OKX", Okx);
    check!("Gate.io", Gate);
    check!("Kraken", Kraken);
}

/// `reduce_only` is carried by a derivatives client and refused by a spot one,
/// on every order path.
///
/// It is the one order field whose meaning depends on the account rather than on
/// the venue: it says "close, do not open", and a spot account holds balances,
/// not positions, so there is nothing for it to close. Binance says so on the
/// wire -- spot rejects the parameter outright with -1104 -- and both it and
/// Upbit already refused. Six others dropped it in silence, and two sent it to a
/// spot endpoint that does not apply it, which is the same outcome reached from
/// the other side: the caller is told the order will only ever reduce, and it
/// will not.
///
/// A dropped `reduce_only` is the most expensive field to drop in the request.
/// Every other field makes the order behave differently; this one makes it open
/// a position where the caller asked to close one, which is the opposite trade.
#[test]
fn reduce_only_is_carried_on_a_derivatives_client_and_refused_on_a_spot_one() {
    /// Every spelling of "this order may only reduce" across the eight
    /// derivatives clients. Gate switches to `auto_size` on a hedged account and
    /// HTX has no flag at all -- it spells the same thing as the order's
    /// `offset`, which is `close` rather than `open`.
    const CARRIED: &[&str] = &[
        "reduceonly",
        "reduce_only",
        "reduce-only",
        "auto_size",
        "close_on_trigger",
        "\"offset\":\"close\"",
    ];

    fn transport() -> Arc<MockHttpTransport> {
        let mock = Arc::new(MockHttpTransport::new());
        for _ in 0..3 {
            mock.push_json(
                200,
                r#"{"status":"ok","code":"0","data":[{"id":42,"type":"spot","state":"working"}]}"#,
            );
        }
        mock
    }

    fn refused(outcome: &Result<wickra_exchange_core::Order>) -> bool {
        matches!(outcome, Err(Error::Exchange { code, .. }) if code == "unsupported")
    }

    /// A word for what happened, for the failure message.
    ///
    /// The outcome itself is not printed. It is the client's answer, and a
    /// client's answer can carry a signed request or a credential back with it;
    /// `CodeQL` reads a `Debug`-formatted one in a panic message as cleartext
    /// logging, and it is right to -- a failing test in CI prints to a log that
    /// outlives it. What the assertion needs is which of three things happened,
    /// and the wire is printed separately where it matters.
    fn verdict(outcome: &Result<wickra_exchange_core::Order>) -> &'static str {
        match outcome {
            Ok(_) => "accepted",
            Err(Error::Exchange { code, .. }) if code == "unsupported" => "refused",
            Err(_) => "failed for some other reason",
        }
    }

    let market = market();
    let creds = || {
        Credentials::new("APIKEY", "c2VjcmV0")
            .with_passphrase("PASS")
            .with_private_key(EC_KEY)
    };
    let reducing = || OrderRequest::limit_buy(market.clone(), dec!(1), dec!(100)).reduce_only();

    // On a derivatives client the flag must reach the venue.
    macro_rules! futures {
        ($name:literal, $venue:ident) => {{
            let mock = transport();
            let options = ExchangeOptions::mainnet(MarketType::UsdMFutures);
            let client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&mock))),
                &options,
                creds(),
            );
            let outcome = client.place_order(&reducing());
            let wire = mock
                .recorded_requests()
                .last()
                .map(|r| format!("{} {}", r.url, r.body.clone().unwrap_or_default()))
                .unwrap_or_default()
                .to_lowercase();
            assert!(
                !wire.is_empty(),
                "{}: futures client sent no order at all ({})",
                $name,
                verdict(&outcome)
            );
            assert!(
                CARRIED.iter().any(|s| wire.contains(s)),
                "{}: a reduce-only futures order went out without saying so; it \
                 opens a position where the caller asked to close one.\nwire: {wire}",
                $name
            );
        }};
    }

    futures!("Binance", Binance);
    futures!("Bybit", Bybit);
    futures!("OKX", Okx);
    futures!("Bitget", Bitget);
    futures!("KuCoin", KuCoin);
    futures!("Gate.io", Gate);
    futures!("HTX", Htx);
    futures!("Kraken", Kraken);

    // On a spot client it must be refused, on every path that takes an order.
    let spot = ExchangeOptions::mainnet(MarketType::Spot);

    macro_rules! spot_single {
        ($name:literal, $venue:ident) => {{
            let mock = transport();
            let client =
                $venue::with_credentials(Box::new(ArcTransport(Arc::clone(&mock))), &spot, creds());
            let outcome = client.place_order(&reducing());
            assert!(
                refused(&outcome),
                "{}: spot accepted reduce_only, which it cannot honour ({})",
                $name,
                verdict(&outcome)
            );
            assert!(
                mock.recorded_requests().is_empty(),
                "{}: spot refused reduce_only, yet sent a request anyway",
                $name
            );
        }};
    }

    spot_single!("Binance", Binance);
    spot_single!("Bybit", Bybit);
    spot_single!("OKX", Okx);
    spot_single!("Bitget", Bitget);
    spot_single!("KuCoin", KuCoin);
    spot_single!("Gate.io", Gate);
    spot_single!("HTX", Htx);
    spot_single!("Kraken", Kraken);
    spot_single!("Coinbase", Coinbase);
    spot_single!("Upbit", Upbit);

    macro_rules! spot_batch {
        ($name:literal, $venue:ident) => {{
            let mock = transport();
            let mut client =
                $venue::with_credentials(Box::new(ArcTransport(Arc::clone(&mock))), &spot, creds());
            let outcome = AdvancedOrders::place_batch(&mut client, &[reducing()]);
            // A batch may refuse as a whole or per leg -- Binance reports one
            // result per request and rejects the leg -- and both keep the
            // promise. What neither may do is let the order reach the venue.
            let whole = matches!(&outcome, Err(Error::Exchange { code, .. }) if code == "unsupported");
            let per_leg = matches!(&outcome, Ok(results) if !results.is_empty()
                && results.iter().all(|r| matches!(r, Err(Error::Exchange { code, .. }) if code == "unsupported")));
            assert!(
                whole || per_leg,
                "{}: spot batch neither refused reduce_only outright nor rejected \
                 every leg carrying it",
                $name
            );
            assert!(
                mock.recorded_requests().is_empty(),
                "{}: spot batch refused reduce_only, yet sent a request anyway",
                $name
            );
        }};
    }

    spot_batch!("Binance", Binance);
    spot_batch!("Bybit", Bybit);
    spot_batch!("OKX", Okx);
    spot_batch!("Bitget", Bitget);
    spot_batch!("KuCoin", KuCoin);
    spot_batch!("Gate.io", Gate);
    spot_batch!("HTX", Htx);
    spot_batch!("Kraken", Kraken);

    macro_rules! spot_ws {
        ($name:literal, $venue:ident) => {{
            let http = transport();
            let ws = Arc::new(MockWsTransport::new());
            let mut client =
                $venue::with_credentials(Box::new(ArcTransport(Arc::clone(&http))), &spot, creds())
                    .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
            let outcome = WsExecution::place_order_ws(&mut client, &reducing());
            assert!(
                refused(&outcome),
                "{}: spot ws accepted reduce_only ({})",
                $name,
                verdict(&outcome)
            );
            assert!(
                ws.sent().is_empty(),
                "{}: spot ws refused reduce_only, yet sent a frame anyway",
                $name
            );
        }};
    }

    spot_ws!("Binance", Binance);
    spot_ws!("Bybit", Bybit);
    spot_ws!("OKX", Okx);
    spot_ws!("Gate.io", Gate);
    spot_ws!("Kraken", Kraken);

    // And where the socket does serve a derivatives account, the frame carries
    // the flag. Binance's did not: it spelled the hedged `positionSide` and
    // stopped there, so a one-way close sent over the socket opened a position.
    // Gate and Kraken are absent because their order sockets are spot-only and
    // refuse a futures client outright.
    macro_rules! futures_ws {
        ($name:literal, $venue:ident) => {{
            let http = transport();
            let ws = Arc::new(MockWsTransport::new());
            let options = ExchangeOptions::mainnet(MarketType::UsdMFutures);
            let mut client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&http))),
                &options,
                creds(),
            )
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
            let _ = WsExecution::place_order_ws(&mut client, &reducing());
            let frame = ws.sent().last().cloned().unwrap_or_default().to_lowercase();
            assert!(
                !frame.is_empty(),
                "{}: futures ws sent no order frame at all",
                $name
            );
            assert!(
                CARRIED.iter().any(|s| frame.contains(s)),
                "{}: a reduce-only futures order frame went out without saying \
                 so; it opens a position where the caller asked to close one.\n\
                 frame: {frame}",
                $name
            );
        }};
    }

    futures_ws!("Binance", Binance);
    futures_ws!("Bybit", Bybit);
    futures_ws!("OKX", Okx);
}

/// A futures client's market-data subscription reaches the futures market, or is
/// refused. It never quietly reaches spot.
///
/// This is [`the trigger contract`](a_trigger_order_is_either_carried_or_refused_but_never_flattened)
/// applied to the streams: carried, or refused, never silently something else.
/// The order paths were held to it; the sockets were not, and five of the eight
/// futures venues failed it.
///
/// Each opened one hardcoded **spot** socket and sent spot channels whatever
/// market the client was built for. So a futures client read futures over REST
/// and watched the spot book, the spot trades and the spot quote -- a different
/// instrument at a different price, with no error to say so. Bitget's `instType`
/// is fixed here; KuCoin, Gate, HTX and Kraken stream from a different host with
/// different channel names, which is a per-venue implementation rather than a
/// parameter, so they refuse until that lands.
///
/// A caller told "not implemented here" can fall back to the REST reads. A
/// caller handed the wrong market's book has no way to tell.
#[test]
fn a_futures_client_never_subscribes_to_a_spot_stream() {
    /// What proves a subscription reached the futures market on this venue:
    /// a marker in the URL it connected to, or in the frame it sent. `None`
    /// means the venue's futures stream is not implemented, so the only
    /// acceptable answer is a refusal.
    struct Expected {
        url: Option<&'static str>,
        frame: Option<&'static str>,
    }

    fn refused(error: &Error) -> bool {
        matches!(error, Error::Exchange { code, .. } if code == "unsupported")
    }

    let market = market();
    let options = ExchangeOptions::mainnet(MarketType::UsdMFutures);

    macro_rules! check {
        ($name:literal, $venue:ident, $expected:expr) => {{
            let expected: Expected = $expected;
            let http = Arc::new(MockHttpTransport::new());
            let ws = Arc::new(MockWsTransport::new());
            let mut client = $venue::with_http(Box::new(ArcTransport(Arc::clone(&http))), &options)
                .with_ws(Box::new(ArcWs(Arc::clone(&ws))));

            match MarketData::subscribe_trades(&mut client, &market) {
                Err(error) => {
                    assert!(
                        refused(&error),
                        "{}: subscription failed for the wrong reason: {error}",
                        $name
                    );
                    assert!(
                        expected.url.is_none() && expected.frame.is_none(),
                        "{}: refused a market it is recorded as streaming",
                        $name
                    );
                    assert!(
                        ws.sent().is_empty() && ws.connected_urls().is_empty(),
                        "{}: refused, yet opened a socket",
                        $name
                    );
                }
                Ok(()) => {
                    assert!(
                        expected.url.is_some() || expected.frame.is_some(),
                        "{}: subscribed on a futures client with no futures stream \
                         implemented -- this is the spot stream",
                        $name
                    );
                    if let Some(marker) = expected.url {
                        let urls = ws.connected_urls().join(" ");
                        assert!(
                            urls.contains(marker),
                            "{}: connected to {urls}, which is not the futures stream \
                             (expected {marker})",
                            $name
                        );
                    }
                    if let Some(marker) = expected.frame {
                        let sent = ws.sent().join(" ");
                        assert!(
                            sent.contains(marker),
                            "{}: subscribed with {sent}, which does not name the \
                             futures market (expected {marker})",
                            $name
                        );
                    }
                }
            }
        }};
    }

    check!(
        "Binance",
        Binance,
        Expected {
            url: Some("fstream.binance.com"),
            frame: None
        }
    );
    check!(
        "Bybit",
        Bybit,
        Expected {
            url: Some("/v5/public/linear"),
            frame: None
        }
    );
    check!(
        "OKX",
        Okx,
        Expected {
            url: None,
            frame: Some("BTC-USDT-SWAP")
        }
    );
    check!(
        "Bitget",
        Bitget,
        Expected {
            url: None,
            frame: Some("\"instType\":\"USDT-FUTURES\"")
        }
    );
    // Not implemented, so refused rather than served from spot.
    check!(
        "KuCoin",
        KuCoin,
        Expected {
            url: Some("ws-api-futures.kucoin.com"),
            frame: None
        }
    );
    check!(
        "Gate.io",
        Gate,
        Expected {
            url: Some("fx-ws.gateio.ws"),
            frame: None
        }
    );
    check!(
        "HTX",
        Htx,
        Expected {
            url: Some("api.hbdm.com"),
            frame: None
        }
    );
    check!(
        "Kraken",
        Kraken,
        Expected {
            url: Some("futures.kraken.com"),
            frame: None
        }
    );

    // The private stream has the same rule, and was wrong on the same venues. A
    // futures client watching the spot account waits for fills that cannot
    // arrive: its own orders never appear there, and nothing reports an error.
    macro_rules! private {
        ($name:literal, $venue:ident, $refuses:literal) => {{
            let http = Arc::new(MockHttpTransport::new());
            let ws = Arc::new(MockWsTransport::new());
            let mut client = $venue::with_credentials(
                Box::new(ArcTransport(Arc::clone(&http))),
                &options,
                Credentials::new("APIKEY", "c2VjcmV0").with_passphrase("PASS"),
            )
            .with_ws(Box::new(ArcWs(Arc::clone(&ws))));
            let outcome = WsUserData::subscribe_user_data(&mut client);
            let refused = matches!(
                &outcome,
                Err(Error::Exchange { code, .. }) if code == "unsupported"
            );
            assert_eq!(
                refused, $refuses,
                "{}: a futures client's private stream must {} here",
                $name,
                if $refuses { "refuse" } else { "reach the futures account" }
            );
            if refused {
                assert!(
                    ws.sent().is_empty(),
                    "{}: refused, yet sent a subscribe frame",
                    $name
                );
            }
        }};
    }

    private!("KuCoin", KuCoin, true);
    private!("Gate.io", Gate, true);
    private!("HTX", Htx, true);
    // Kraken already routes here: `subscribe_user_data` dispatches to
    // `subscribe_user_data_futures`, which is the rule being kept rather than
    // an exception to it.
    private!("Kraken", Kraken, false);
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
