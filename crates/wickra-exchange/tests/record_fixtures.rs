//! Record each venue's real answer to each public read, into `testdata/`.
//!
//! `#[ignore]`d: it hits ten live venues and writes files. Run it deliberately,
//! when a fixture needs refreshing:
//!
//! ```text
//! cargo test -p wickra-exchange --test record_fixtures -- --ignored --nocapture
//! ```
//!
//! # Why the recorder drives the clients
//!
//! The obvious way to collect these is to write the URLs down and `curl` them.
//! That records what the author believes the client asks for, which is the
//! failure the fixtures exist to rule out: the offline suite was already written
//! from the same reading of the same documentation as the parsers, and it is how
//! seven clients came to never send `time_in_force` under a green suite.
//!
//! So nothing here names a URL. Each venue's real client is built over a
//! transport that wraps the real one and writes down whatever came back, and the
//! client is asked for a ticker, some candles and a book. The URL is the one the
//! client uses because the client chose it.
//!
//! A venue that is unreachable from this machine is skipped out loud and its
//! existing fixture is left alone, for the reason [`live_public`] gives: a
//! recorder that overwrote a good fixture with a 451 page would be worse than
//! no recorder.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use wickra_exchange::{
    Binance, Bitget, Bybit, Coinbase, ExchangeOptions, Gate, HttpRequest, HttpResponse,
    HttpTransport, Htx, Kraken, KuCoin, MarketType, Okx, ReqwestHttpTransport, Result, Symbol,
    Upbit,
};

/// The three public reads every venue client implements.
const ENDPOINTS: [&str; 3] = ["ticker", "klines", "order_book"];

/// Where the fixtures live, relative to this crate.
fn testdata_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testdata")
}

/// A transport that answers from the network and writes down what it heard.
///
/// One venue read can cost more than one request -- a client that resolves
/// something before it can ask its real question -- so the responses are
/// numbered in the order they arrived and replayed in that order.
struct Recorder {
    inner: ReqwestHttpTransport,
    venue: &'static str,
    endpoint: Mutex<&'static str>,
    seen: Mutex<usize>,
}

impl Recorder {
    fn new(venue: &'static str, options: &ExchangeOptions) -> Result<Self> {
        Ok(Self {
            inner: ReqwestHttpTransport::new(options)?,
            venue,
            endpoint: Mutex::new(""),
            seen: Mutex::new(0),
        })
    }

    /// Begin recording the next endpoint; resets the response counter.
    fn begin(&self, endpoint: &'static str) {
        *self.endpoint.lock().unwrap() = endpoint;
        *self.seen.lock().unwrap() = 0;
    }

    /// How many responses the last endpoint produced.
    fn seen(&self) -> usize {
        *self.seen.lock().unwrap()
    }
}

impl HttpTransport for Recorder {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        let response = self.inner.execute(request)?;
        let endpoint = *self.endpoint.lock().unwrap();
        let mut seen = self.seen.lock().unwrap();

        // A non-2xx answer is not a fixture. It is a geo-block, a rate limit or
        // an outage, and writing it over a good recording would turn this from a
        // refresher into a way to lose one.
        if response.is_success() {
            let dir = testdata_dir().join(self.venue);
            fs::create_dir_all(&dir).expect("testdata directory");
            let name = if *seen == 0 {
                format!("{endpoint}.json")
            } else {
                format!("{endpoint}.{}.json", *seen + 1)
            };
            fs::write(dir.join(&name), &response.body).expect("write fixture");
            eprintln!("  {}/{name}: {} bytes", self.venue, response.body.len());
        } else {
            eprintln!(
                "  {}/{endpoint}: HTTP {} -- not recorded, existing fixture kept",
                self.venue, response.status
            );
        }
        *seen += 1;
        Ok(response)
    }
}

/// Share one recorder between the test and the client that owns its transport.
struct ArcTransport(Arc<Recorder>);

impl HttpTransport for ArcTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.0.execute(request)
    }
}

/// Drive one venue's three public reads through a recording transport.
macro_rules! record {
    ($venue:literal, $client:ident, $symbol:expr) => {{
        let options = ExchangeOptions::mainnet(MarketType::Spot);
        let recorder =
            Arc::new(Recorder::new($venue, &options).expect("the real transport must build"));
        let client = $client::with_http(Box::new(ArcTransport(Arc::clone(&recorder))), &options);
        let symbol: Symbol = $symbol;

        eprintln!("{}:", $venue);
        for endpoint in ENDPOINTS {
            recorder.begin(endpoint);
            let outcome = match endpoint {
                "ticker" => client.ticker(&symbol).map(|_| ()),
                "klines" => client.klines(&symbol, "1m", 5).map(|_| ()),
                _ => client.order_book(&symbol, 5).map(|_| ()),
            };
            match outcome {
                Ok(()) => {}
                Err(error) => eprintln!(
                    "  {}/{endpoint}: skipped ({error}); {} response(s) seen",
                    $venue,
                    recorder.seen()
                ),
            }
        }
    }};
}

fn usdt(base: &str) -> Symbol {
    Symbol::new(base, "USDT")
}

#[test]
#[ignore = "hits ten live venues and writes testdata/; run explicitly with --ignored"]
fn record_every_venues_public_reads() {
    record!("binance", Binance, usdt("BTC"));
    record!("bybit", Bybit, usdt("BTC"));
    record!("okx", Okx, usdt("BTC"));
    record!("bitget", Bitget, usdt("BTC"));
    record!("kucoin", KuCoin, usdt("BTC"));
    record!("gate", Gate, usdt("BTC"));
    record!("htx", Htx, usdt("BTC"));
    record!("kraken", Kraken, usdt("BTC"));
    record!("coinbase", Coinbase, Symbol::new("BTC", "USD"));
    record!("upbit", Upbit, Symbol::new("BTC", "KRW"));
}
