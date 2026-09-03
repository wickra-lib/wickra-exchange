//! Every venue's parser, against that venue's own recorded answer.
//!
//! # What this proves that the module tests do not
//!
//! The 543 unit tests drive each client over [`MockHttpTransport`] with
//! hand-written JSON. They prove the parser reads what the author believed the
//! venue sends. They cannot prove that belief was right, because the fixture and
//! the parser were written by the same hand from the same reading of the same
//! documentation, and they agree whether or not the reading was correct. That is
//! not hypothetical: it is how seven clients came to never send `time_in_force`
//! under a green suite.
//!
//! `tests/live_public.rs` closes the other half by asking the real venue, and it
//! is the right tool for detecting drift -- but it needs a network, and it skips
//! out loud when the runner is blocked or rate-limited. What it cannot be is
//! reproducible. A test that sometimes verifies nothing cannot be the only proof
//! that a parser reads real venue output.
//!
//! So the real answers are recorded once, by
//! `wickra-exchange/tests/record_fixtures.rs`, and replayed here offline. These
//! bytes came off the venue's own wire; nobody wrote them from documentation.
//!
//! # What a failure means
//!
//! The parser can no longer read a reply this venue actually sent. Refreshing
//! the fixture is only correct once the venue is known to have changed; until
//! then, the parser is wrong about the venue.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use wickra_exchange_core::{
    Binance, Bitget, Bybit, ExchangeOptions, Gate, HttpRequest, HttpResponse, HttpTransport, Htx,
    Kraken, KuCoin, MarketType, MockHttpTransport, Okx, Result, Symbol, Upbit,
};

/// Venues whose public reads are recorded, with the market each was recorded on.
///
/// Coinbase is absent, and deliberately: its market endpoints are signed, so a
/// public recording of them is not possible without a key. `live_public.rs`
/// records the same exclusion for the same reason. A venue that quietly stopped
/// having fixtures would look exactly like this list shrinking, so the list is
/// asserted rather than inferred from what is on disk.
const RECORDED: [(&str, &str, &str); 9] = [
    ("binance", "BTC", "USDT"),
    ("bybit", "BTC", "USDT"),
    ("okx", "BTC", "USDT"),
    ("bitget", "BTC", "USDT"),
    ("kucoin", "BTC", "USDT"),
    ("gate", "BTC", "USDT"),
    ("htx", "BTC", "USDT"),
    ("kraken", "BTC", "USDT"),
    ("upbit", "BTC", "KRW"),
];

const ENDPOINTS: [&str; 3] = ["ticker", "klines", "order_book"];

struct ArcTransport(Arc<MockHttpTransport>);

impl HttpTransport for ArcTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.0.execute(request)
    }
}

fn testdata(venue: &str, endpoint: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testdata")
        .join(venue)
        .join(format!("{endpoint}.json"))
}

/// The recorded body for one venue and endpoint.
///
/// Missing is a failure, not a skip. A fixture that disappears takes its
/// coverage with it silently, which is the shape of gap this file exists to
/// close.
fn fixture(venue: &str, endpoint: &str) -> String {
    let path = testdata(venue, endpoint);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{venue}/{endpoint}: no recorded fixture at {} ({error}).\n\
             Record one with: cargo test -p wickra-exchange --test record_fixtures \
             -- --ignored",
            path.display()
        )
    })
}

/// A client over a mock transport primed with one recorded response.
fn primed(venue: &str, endpoint: &str) -> Arc<MockHttpTransport> {
    let mock = Arc::new(MockHttpTransport::new());
    mock.push_json(200, fixture(venue, endpoint));
    mock
}

/// Ask one venue's client for one read, over its own recorded answer.
macro_rules! replay {
    ($venue:literal, $client:ident, $base:expr, $quote:expr) => {{
        let options = ExchangeOptions::mainnet(MarketType::Spot);
        let symbol = Symbol::new($base, $quote);

        let mock = primed($venue, "ticker");
        let client = $client::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &options);
        let quote = client
            .ticker(&symbol)
            .unwrap_or_else(|e| panic!("{}: the recorded ticker no longer parses: {e}", $venue));
        assert!(
            quote.last > rust_decimal::Decimal::ZERO,
            "{}: recorded ticker parsed to a last price of {}",
            $venue,
            quote.last
        );

        let mock = primed($venue, "klines");
        let client = $client::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &options);
        let candles = client
            .klines(&symbol, "1m", 5)
            .unwrap_or_else(|e| panic!("{}: the recorded klines no longer parse: {e}", $venue));
        assert!(
            !candles.is_empty(),
            "{}: recorded klines parsed to no candles",
            $venue
        );
        for candle in &candles {
            assert!(
                candle.high >= candle.low,
                "{}: a recorded candle has high {} below low {}",
                $venue,
                candle.high,
                candle.low
            );
        }

        let mock = primed($venue, "order_book");
        let client = $client::with_http(Box::new(ArcTransport(Arc::clone(&mock))), &options);
        let book = client.order_book(&symbol, 5).unwrap_or_else(|e| {
            panic!("{}: the recorded order book no longer parses: {e}", $venue)
        });
        assert!(
            !book.bids.is_empty() && !book.asks.is_empty(),
            "{}: recorded book parsed to {} bids and {} asks",
            $venue,
            book.bids.len(),
            book.asks.len()
        );
        assert!(
            book.bids[0].price < book.asks[0].price,
            "{}: recorded book has a crossed top of book: bid {} >= ask {}",
            $venue,
            book.bids[0].price,
            book.asks[0].price
        );
    }};
}

/// Every recorded venue answers its own recording.
#[test]
fn every_recorded_venue_still_parses_its_own_answer() {
    replay!("binance", Binance, "BTC", "USDT");
    replay!("bybit", Bybit, "BTC", "USDT");
    replay!("okx", Okx, "BTC", "USDT");
    replay!("bitget", Bitget, "BTC", "USDT");
    replay!("kucoin", KuCoin, "BTC", "USDT");
    replay!("gate", Gate, "BTC", "USDT");
    replay!("htx", Htx, "BTC", "USDT");
    replay!("kraken", Kraken, "BTC", "USDT");
    replay!("upbit", Upbit, "BTC", "KRW");
}

/// The recordings that must exist, exist.
///
/// Separate from the parse test on purpose: a fixture deleted by accident should
/// say "the recording is gone", not "the parser broke". Both are failures, and
/// they call for opposite fixes.
#[test]
fn every_recorded_venue_has_all_three_fixtures() {
    for (venue, _, _) in RECORDED {
        for endpoint in ENDPOINTS {
            let path = testdata(venue, endpoint);
            assert!(
                path.is_file(),
                "{venue}/{endpoint}: recording missing at {}",
                path.display()
            );
            assert!(
                !fixture(venue, endpoint).trim().is_empty(),
                "{venue}/{endpoint}: recording is empty"
            );
        }
    }
}
