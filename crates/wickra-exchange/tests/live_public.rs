//! Gated live checks: every venue client against that venue's real public API.
//!
//! These are `#[ignore]`d and run from `.github/workflows/testnet.yml` on a
//! nightly schedule, never on a push.
//!
//! # Why this exists
//!
//! The offline suite drives every client over `MockHttpTransport` with
//! hand-written JSON. It proves that the request builder builds what the author
//! believed and that the parser reads what the author believed. It cannot prove
//! that either matches the venue, because the venue never appears in it — the
//! fixture and the parser were written by the same hand, from the same reading
//! of the same documentation, and they agree with each other whether or not that
//! reading was right.
//!
//! That is not a hypothetical gap. It is how a run of defects stayed invisible
//! under a green suite: seven clients never sent `time_in_force` at all, and no
//! offline test could notice, because the fixture did not expect it either.
//!
//! These tests close the other half. They ask the real venue and hand the answer
//! to the real parser. No credentials: every endpoint here is public.
//!
//! # What counts as a failure
//!
//! Only one thing: **the venue answered and the parser could not read it.** That
//! is upstream drift — a renamed field, a changed shape, a type that turned from
//! string to number — and it is exactly what this suite is for.
//!
//! Everything else is skipped out loud, because it says nothing about the code:
//!
//! * a network error, timeout, or rate limit — the runner's connection
//! * HTTP 451/403 — several venues geo-restrict data-centre IP ranges, and CI
//!   runners live in data centres
//! * an authentication error — a venue whose "public" endpoint is not
//!
//! Skipping those is what keeps a nightly job honest rather than merely green:
//! a suite that failed on a blocked IP would be turned off within a week, and
//! then the drift it exists to catch would go unnoticed too.

use wickra_exchange::{connect, Credentials, Error, ExchangeOptions, MarketType, Symbol};

/// Public endpoints need no key. An empty credential set reaches them, and any
/// venue that rejects it says so as an auth error, which is skipped.
fn anonymous() -> Credentials {
    Credentials::new("", "")
}

/// Whether this outcome says something about *our* code, or about the runner's
/// connection and location.
///
/// `Deserialization` is the one that does: the venue replied and the parser
/// could not read the reply.
fn is_drift(error: &Error) -> bool {
    // Everything else is the runner or the venue rejecting the request rather
    // than failing to be understood: transport failures, HTTP 451/403 (several
    // venues geo-restrict data-centre ranges), rate limits, and the venue's own
    // complaints about a symbol or a missing key. None of those is drift in a
    // parser, so they are skipped rather than failed.
    matches!(error, Error::Deserialization(_))
}

/// Run one public read and report the outcome, failing only on drift.
///
/// Returns whether the call actually reached the venue and parsed, so the caller
/// can tell "the venue answered correctly" from "we learned nothing".
fn check<T>(venue: &str, what: &str, outcome: Result<T, Error>) -> bool {
    match outcome {
        Ok(_) => {
            eprintln!("  {venue}: {what} ok");
            true
        }
        Err(error) => {
            assert!(
                !is_drift(&error),
                "{venue}: {what} reached the venue and the parser could not read \
                 the reply -- this is upstream API drift, not a flaky runner: {error}"
            );
            eprintln!("  {venue}: {what} skipped ({error})");
            false
        }
    }
}

/// Whether this venue stamps its public reads, checked against the live API
/// when these were written.
///
/// A venue listed here that stops sending a timestamp is drift as surely as a
/// renamed price field, and the offline fixtures cannot notice: they contain
/// whatever the author put in them. A venue *not* listed reports `0`, and that
/// is recorded rather than assumed -- Gate, Bybit and Kraken publish no stamp on
/// these endpoints, and Coinbase's need a key.
#[derive(Clone, Copy)]
struct Stamps {
    ticker: bool,
    order_book: bool,
}

/// The public market-data surface, against the live venue.
///
/// `ticker`, `klines` and `order_book` are the three public reads every client
/// implements, and between them they exercise most of each venue's response
/// shapes: a scalar quote, an array of candles, and two arrays of levels.
fn public_reads(venue: &str, symbol: &Symbol, stamps: Stamps) {
    let options = ExchangeOptions::mainnet(MarketType::Spot);
    let Ok(mut client) = connect(venue, anonymous(), &options) else {
        panic!("{venue}: the factory could not build a client");
    };

    eprintln!("{venue}:");
    let mut reached = 0;

    let ticker = client.ticker(symbol);
    if let Ok(quote) = &ticker {
        assert_eq!(
            quote.timestamp > 0,
            stamps.ticker,
            "{venue}: ticker timestamp presence changed -- expected stamped={}, \
             got {}. Either the venue changed what it sends, or the parser \
             stopped reading it.",
            stamps.ticker,
            quote.timestamp
        );
    }
    reached += usize::from(check(venue, "ticker", ticker));

    reached += usize::from(check(venue, "klines", client.klines(symbol, "1m", 5)));

    let book = client.order_book(symbol, 5);
    if let Ok(snapshot) = &book {
        assert_eq!(
            snapshot.timestamp > 0,
            stamps.order_book,
            "{venue}: order-book timestamp presence changed -- expected \
             stamped={}, got {}",
            stamps.order_book,
            snapshot.timestamp
        );
    }
    reached += usize::from(check(venue, "order_book", book));

    if reached == 0 {
        eprintln!("  {venue}: unreachable from this runner; nothing was verified");
    }
}

fn usdt(base: &str) -> Symbol {
    Symbol::new(base, "USDT")
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn binance_public_reads_parse() {
    public_reads(
        "binance",
        &usdt("BTC"),
        // Spot depth carries no timestamp; the 24h ticker carries `closeTime`.
        Stamps {
            ticker: true,
            order_book: false,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn bybit_public_reads_parse() {
    public_reads(
        "bybit",
        &usdt("BTC"),
        // The ticker list entries carry none; the book result carries `ts`.
        Stamps {
            ticker: false,
            order_book: true,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn okx_public_reads_parse() {
    public_reads(
        "okx",
        &usdt("BTC"),
        Stamps {
            ticker: true,
            order_book: true,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn bitget_public_reads_parse() {
    public_reads(
        "bitget",
        &usdt("BTC"),
        Stamps {
            ticker: true,
            order_book: true,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn kucoin_public_reads_parse() {
    public_reads(
        "kucoin",
        &usdt("BTC"),
        Stamps {
            ticker: true,
            order_book: true,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn gate_public_reads_parse() {
    public_reads(
        "gateio",
        &usdt("BTC"),
        // Gate stamps the book (`update`) but not the ticker.
        Stamps {
            ticker: false,
            order_book: true,
        },
    );
}

#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn htx_public_reads_parse() {
    public_reads(
        "htx",
        &usdt("BTC"),
        Stamps {
            ticker: true,
            order_book: true,
        },
    );
}

/// Kraken quotes BTC against USD and spells the asset `XBT` on the wire; the
/// client's own symbol mapping is part of what this checks.
#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn kraken_public_reads_parse() {
    public_reads(
        "kraken",
        &Symbol::new("BTC", "USD"),
        // Kraken stamps individual depth levels, never the book or quote.
        Stamps {
            ticker: false,
            order_book: false,
        },
    );
}

/// Coinbase Advanced Trade quotes against USD.
#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn coinbase_public_reads_parse() {
    public_reads(
        "coinbase",
        &Symbol::new("BTC", "USD"),
        Stamps {
            ticker: false,
            order_book: false,
        },
    );
}

/// Upbit is a KRW venue and spells its markets quote-first (`KRW-BTC`), which
/// the client's symbol mapping handles.
#[test]
#[ignore = "hits the live venue; run explicitly with --ignored"]
fn upbit_public_reads_parse() {
    public_reads(
        "upbit",
        &Symbol::new("BTC", "KRW"),
        Stamps {
            ticker: true,
            order_book: true,
        },
    );
}
