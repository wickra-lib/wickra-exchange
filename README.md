<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514" alt="Wickra — streaming-first technical indicators" width="100%"></a>
</p>

[![Built on Wickra](https://img.shields.io/badge/built%20on-wickra-3b82f6)](https://github.com/wickra-lib/wickra)
[![Status](https://img.shields.io/badge/status-pre--alpha%20(scaffolding)-red)](https://github.com/wickra-lib/wickra-exchange)
[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![CodeQL](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codeql.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/codeql.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![GitHub release](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/release.svg)](https://github.com/wickra-lib/wickra-exchange/releases/latest)
[![crates.io](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/crates.svg)](https://crates.io/crates/wickra-exchange)
[![PyPI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/pypi.svg)](https://pypi.org/project/wickra-exchange/)
[![npm](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/npm.svg)](https://www.npmjs.com/package/wickra-exchange)
[![NuGet](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/nuget.svg)](https://www.nuget.org/packages/WickraExchange)
[![Maven Central](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/maven.svg)](https://central.sonatype.com/artifact/org.wickra/wickra-exchange)
[![Go module](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/go.svg)](https://pkg.go.dev/github.com/wickra-lib/wickra-exchange-go)
[![R-universe](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/r-universe.svg)](https://wickra-lib.r-universe.dev)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](#license)
[![OpenSSF Scorecard](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/scorecard.svg)](https://scorecard.dev/viewer/?uri=github.com/wickra-lib/wickra-exchange)
[![OpenSSF Best Practices](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/best-practices.svg)](https://www.bestpractices.dev)
[![Build provenance](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/provenance.svg)](https://github.com/wickra-lib/wickra-exchange/attestations)
[![Docs](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/docs.svg)](https://wickra.org)
[![Verified across 9 languages](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/verified.svg)](golden/)
[![Live demo](https://img.shields.io/badge/live%20demo-live.wickra.org-3b82f6)](https://live.wickra.org)

---

**One typed API. Ten exchanges. Eight languages.** Streaming-native crypto-exchange
connectivity — market data *and* signed order execution — built on the
[Wickra](https://github.com/wickra-lib/wickra) core.

```rust
use wickra_exchange::{Exchange, ExchangeOptions, MarketData, OrderRequest, PaperExchange};

// Offline: fills are simulated, deterministically.
let mut ex = PaperExchange::new().with_balance("USDT", dec!(100_000));
ex.set_price(&market, dec!(20_000));

// Live: same trait, same calls, same order type — only the constructor differs.
let mut ex = Exchange::new("binance", creds, ExchangeOptions::testnet(MarketType::Spot))?;

let order = ex.place_order(&OrderRequest::market_buy(market, dec!(1)))?;
println!("{} filled at {:?}", order.filled_quantity, order.average_price);
```

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

> **Part of the [Wickra ecosystem](https://github.com/wickra-lib):** the same data-driven core and ten-language binding surface also power [wickra-backtest](https://github.com/wickra-lib/wickra-backtest), [wickra-terminal](https://github.com/wickra-lib/wickra-terminal), [wickra-screener](https://github.com/wickra-lib/wickra-screener) and 20 more — see [the full list](https://github.com/wickra-lib).

A single, compile-time-typed `Exchange` trait spans the ten largest venues
(Binance, OKX, Bybit, Coinbase, Upbit, Bitget, Gate.io, Kraken, KuCoin, HTX)
behind bespoke authentication and WebSocket state machines. Market-data streams
are **pull-based** (`poll_events`), so the same surface crosses the C ABI to
every binding — including single-threaded R — as trivially as a synchronous
call. Quantities in the order layer are exact [`Decimal`], never `f64`.

What makes it more than "a typed ccxt" is that it **plugs straight into the rest
of Wickra**:

- **`PaperExchange`** — a first-class `Exchange` implementation that simulates
  fills through the [wickra-backtest](https://github.com/wickra-lib/wickra-backtest)
  engine. The *same* strategy runs paper ↔ live by swapping the implementation.
- **Microstructure-native feeds** — trades and depth arrive from the venue
  streams already shaped for `wickra-core`, and one call converts each into the
  exact input type the indicators consume (`Trade`, `OrderBook`, `CrossSection`)
  — feeding 514 indicators and the backtester with zero glue. The derivatives
  channels (funding, open interest, liquidations, positioning, mark/index) are
  typed too and fold into a `DerivativesTick`, and **all eight futures venues
  subscribe to them**. A venue that publishes no such channel refuses it rather
  than subscribing to nothing — the matrix says which, and why. The matrix says which channel each venue publishes,
  and where a venue publishes none it refuses rather than subscribing to
  nothing. See [Derivatives feeds](docs/DERIVATIVES.md#derivatives-feeds).
- **`ReplayExchange`** — a recorded feed driven through the same trait, so a
  backtest runs on *real* recorded microstructure.

The same `Exchange` API is reachable from **Rust, Python, Node.js, C, C++, C#,
Go, Java and R** — native PyO3 / napi bindings plus a C ABI hub.

There is also a **WASM** binding, and it is deliberately smaller than the rest:
`wasm32-unknown-unknown` has no sockets, so no live venue client can exist there,
and authenticated trading needs raw sockets and secret keys a browser sandbox
forbids. What it does carry is the part that needs neither — the offline
`PaperExchange` and `ReplayExchange`, with the same order, balance and event API
the live clients expose. That is enough to run a strategy, or a replay UI, in a
page; the browser-safe slice of *public market data* remains covered by
`wickra-wasm` over a browser WebSocket. See
[bindings/wasm](bindings/wasm/README.md) for what is present and what is not.

[`Decimal`]: https://docs.rs/rust_decimal

## Status

**Pre-alpha — scaffolding.** This repository is being built out from the
[`wickra-backtest`](https://github.com/wickra-lib/wickra-backtest) template. The
workspace, the core crate and the project governance are in place; the exchange
implementations, the connectivity machinery and the language bindings are landing
incrementally. **The API shown below is the target surface, not yet shippable.**
Track progress in [ROADMAP.md](ROADMAP.md). Not released to any registry.

> ⚠️ **Real orders move real money.** Every signed-execution code path is
> safety-critical. Use withdrawal-disabled keys, test against exchange testnets
> first, and never put secret keys in a browser or client.

## Documentation

- **[Exchanges](docs/EXCHANGES.md)** — the ten venues, their market types and the
  per-exchange capability matrix.
- **[Authentication](docs/AUTH.md)** — the signing families (HMAC-SHA256/512,
  JWT ES256/HS512, passphrase) and how `Credentials` map onto each.
- **[Streaming](docs/STREAMING.md)** — the pull-based event model, the local
  order-book builder and reconnect/resubscribe semantics.
- **[Derivatives & advanced orders](docs/DERIVATIVES.md)** — the `Derivatives`,
  `AdvancedOrders`, `WsUserData` and `WsExecution` traits, futures routing, and
  per-venue gaps.
- **[Capability matrix](docs/CAPABILITIES.md)** — real per-venue support for
  spot/futures, positions/leverage/margin, and STP/amend/batch/OCO/WS-order.
- **[Architecture](ARCHITECTURE.md)** — crates, traits, the transport abstraction
  and design decisions.
- **[Benchmarks](BENCHMARKS.md)** — signing / parse / filter-rounding throughput.
- **[Examples](examples/)** — one runnable program per language.

## Quickstart

Connect, read a ticker, subscribe to a pull-based stream, and place an order on a
testnet:

```rust
use wickra_exchange::{Exchange, Credentials, ExchangeOptions, MarketType, OrderRequest};

let creds = Credentials::new("api-key", "api-secret");
let opts = ExchangeOptions::testnet(MarketType::Spot);
let mut ex = Exchange::new("binance", creds, opts)?;

// REST: a typed ticker.
let ticker = ex.ticker("BTC/USDT")?;
println!("last = {}", ticker.last);

// Streaming: subscribe, then drain events from your own loop (pull, not callbacks).
ex.subscribe_trades("BTC/USDT")?;
for event in ex.poll_events() {
    println!("{event:?}");
}

// Signed execution (testnet) with exact Decimal price/qty.
let order = OrderRequest::limit_buy("BTC/USDT", "0.001".parse()?, "20000".parse()?);
let placed = ex.place_order(&order)?;
println!("order id = {}", placed.id);
```

The same flow is available from every binding — see the per-language quickstarts
below.

## Exchanges

The ten largest venues by volume, each behind the same `Exchange` trait. The
*signing family* drives the per-exchange authentication; everything above it
(symbols, order types, the order-book builder, reconnect) is shared.

| #  | Exchange | Spot | USDⓈ-M | COIN-M | Signing family |
|----|----------|:----:|:------:|:------:|----------------|
| 1  | Binance  | ✅ | ✅ | ⏳ | HMAC-SHA256 (query string), Ed25519 optional |
| 2  | OKX      | ✅ | ✅ | ⏳ | HMAC-SHA256 (ts+method+path+body, b64) + passphrase |
| 3  | Bybit    | ✅ | ✅ | ⏳ | HMAC-SHA256 (ts+key+recvWindow+payload) |
| 4  | Coinbase | ✅ | — | — | JWT ES256 / Ed25519 per request |
| 5  | Upbit    | ✅ | — | — | JWT HS512 + SHA512 query hash |
| 6  | Bitget   | ✅ | ✅ | ⏳ | HMAC-SHA256 (b64) + passphrase |
| 7  | Gate.io  | ✅ | ✅ | ⏳ | HMAC-SHA512 (SHA512 payload hash) |
| 8  | Kraken   | ✅ | ⏳ | — | HMAC-SHA512 (URI + SHA256(nonce+body)), b64 secret |
| 9  | KuCoin   | ✅ | ⏳ | — | HMAC-SHA256 (b64) + signed passphrase |
| 10 | HTX      | ✅ | ⏳ | — | HMAC-SHA256 AWS-style (method+host+path+sorted) |

✅ implemented · ⏳ planned · — not offered by the venue. The current state of
each cell is tracked in [docs/EXCHANGES.md](docs/EXCHANGES.md).

## Use the same API in any language

| Language | Binding | Quickstart |
|----------|---------|------------|
| Rust     | `wickra-exchange` crate | this README |
| Python   | PyO3 / maturin | [bindings/python](bindings/python/README.md) |
| Node.js  | napi-rs | [bindings/node](bindings/node/README.md) |
| C / C++  | C ABI (cbindgen) | [bindings/c](bindings/c/README.md) |
| C#       | P/Invoke | [bindings/csharp](bindings/csharp/README.md) |
| Go       | cgo | [bindings/go](bindings/go/README.md) |
| Java     | FFM / Panama | [bindings/java](bindings/java/README.md) |
| R        | `.Call` | [bindings/r](bindings/r/README.md) |
| WASM     | wasm-pack | [bindings/wasm](bindings/wasm/README.md) — paper / replay only |

The C, C++, C#, Go, Java and R bindings all call through the same C ABI hub. The
[replay corpus](golden/) asserts every language normalises recorded exchange
responses into byte-identical typed structs.

## Project layout

```
wickra-exchange/
├── crates/
│   ├── wickra-exchange-core/   traits + types (Decimal) + credentials + the
│   │                           connectivity machinery (symbol, clock, ratelimiter,
│   │                           orderbook builder, transport, ws, reconcile, feeds)
│   │                           and the per-exchange implementations
│   ├── wickra-exchange/        facade crate (re-exports the public surface)
│   └── wickra-exchange-bench/  criterion signing / parse / filter benchmarks
├── bindings/
│   ├── python/   PyO3 + maturin          ├── csharp/  P/Invoke over the C ABI
│   ├── node/     napi-rs                 ├── go/      cgo over the C ABI
│   ├── c/        C ABI (cdylib + header) ├── java/    FFM over the C ABI
│   ├── r/        .Call over the C ABI    └── wasm/    wasm-pack (paper/replay only)
├── golden/       recorded-response replay corpus + expected normalised structs
├── examples/     one runnable program per language
├── docs/         exchanges, auth, streaming and architecture guides
└── fuzz/         cargo-fuzz targets (nightly)
```

`bindings/wasm/` carries the offline simulators only — see the intro for why.

## Building everything from source

```bash
# Rust core + tests + lints
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo bench -p wickra-exchange-bench

# Python binding (requires a Rust toolchain + maturin)
cd bindings/python && maturin develop --release && pytest

# Node binding (requires @napi-rs/cli)
cd bindings/node && npm install && npm run build && npm test

# C ABI (cdylib + staticlib + generated header)
cargo build -p wickra-exchange-c --release

# C# binding (requires the .NET 8 SDK; links the C ABI above)
dotnet test bindings/csharp/WickraExchange.Tests/WickraExchange.Tests.csproj

# Go binding (requires a C compiler for cgo; links the C ABI above)
cd bindings/go && go test ./...

# Java binding (requires JDK 22+ and Maven; links the C ABI above)
mvn -f bindings/java test

# R binding (requires a C toolchain / Rtools; links the C ABI above)
WKEX_INC="$PWD/bindings/c/include" WKEX_LIB="$PWD/target/debug" R CMD INSTALL bindings/r
```

Integration tests that hit a live exchange run only against **testnets**, are
gated behind environment variables and are `#[ignore]` by default — they never
touch mainnet with real keys. Fuzzing requires a nightly toolchain — see
[`fuzz/`](fuzz/).

## Testing

Run the suites with the commands in
[Building everything from source](#building-everything-from-source).

- **`wickra-exchange-core`** — 632 unit tests. Every venue client is generic over
  an injected transport, so its request-build, signing, parse and normalise path
  is exercised offline against `MockHttpTransport` / `MockWsTransport`: queued
  responses in, recorded requests out.
- **`tests/conformance.rs`** — contracts every `Exchange` implementation must
  satisfy, not one venue's behaviour: the order lifecycle (place a resting order
  → query it → find it in open orders → cancel it) against `PaperExchange` and
  `ReplayExchange`, and object-safety plus naming against all ten venue clients.
- **`tests/proptest_invariants.rs`** — property tests over the shared machinery:
  filter rounding stays on the step/tick grid, and decimal and symbol
  parse/format round-trips.
- **`tests/golden.rs`** — the recorded scenarios under [`golden/`](golden/).
- **`crates/wickra-exchange/tests/live_public.rs`** — the other half of the
  offline suite, and the reason it is not enough on its own. A mock-transport
  test proves the parser reads what the author believed; it cannot prove that
  belief matched the venue, because the fixture and the parser were written by
  the same hand from the same reading. This suite asks the real venue and hands
  the answer to the real parser — all ten clients, through `ticker`, `klines`
  and `order_book`, with no credentials. It is `#[ignore]`d and runs nightly
  from `testnet.yml`, never on a push. A failure means the venue answered and
  the parser could not read the reply; network errors, geo-blocks and auth
  errors are skipped out loud, because they say nothing about the code.
- **`fuzz/`** — five `cargo-fuzz` targets over everything a remote server can put
  on the wire: JSON responses, WebSocket frames, order-book diffs, filter
  rounding and credential/symbol parsing. Nothing may panic, because a panic
  across the C ABI is undefined behaviour. CI runs each briefly on every push.
- **Bindings** — `pytest` (Python), `node --test` (Node), `dotnet test` (C#),
  `go test` (Go), JUnit (Java) and `testthat` (R), each covering construction,
  the market-data and execution surface, derivatives and replay.
- **Surface parity** — `scripts/check_binding_surface.py` reads the trait methods
  out of `crates/wickra-exchange-core/src/traits.rs` and holds all seven bindings
  to them. A method or constructor present in one binding and missing from
  another fails CI; nothing else compares the bindings to each other.
- **Examples** — every example is built or parsed on each push, the compiled ones
  against the binding in this tree.

### What the offline suites cannot tell you

Every venue test above feeds the client a response the test itself wrote. That
proves the parser handles the shape it was given; it cannot prove the shape is
the one the venue actually sends. Live coverage is deliberately narrow and
deliberately separate: `.github/workflows/testnet.yml` runs the `#[ignore]`
integration tests nightly and on demand against public endpoints. They are
`#[ignore]` by default and gated behind environment variables, so a normal
`cargo test` never opens a socket and mainnet with real keys is never touched.

## Benchmarks

Connectivity throughput is dominated by the network, not the CPU, so the
benchmarks measure the CPU-bound work done per request — the part that must not
become the bottleneck under load — rather than round-trip latency, which is not
reproducible and not ours to measure.

```bash
cargo bench -p wickra-exchange-bench
```

Four groups: request signing (HMAC-SHA256/512 and JWT construction), response
parsing (recorded payloads into the typed structs), filter rounding (a price or
quantity onto a venue's lot/tick/min-notional grid, in `Decimal`), and
order-book diff application including sequence-gap detection.

Measured figures, the methodology and the machine they were taken on are in
[BENCHMARKS.md](BENCHMARKS.md).

## Requirements

The minimum supported version per language. The same Rust core runs behind every
binding; the C-ABI bindings that compile on install — Go (cgo) and R (`.Call`) —
also need a C compiler, and Java runs with `--enable-native-access=ALL-UNNAMED`.

| Language | Package                                    | Minimum supported          |
|----------|--------------------------------------------|----------------------------|
| Rust     | crates.io · `wickra-exchange`              | 1.86 (MSRV)                |
| Python   | PyPI · `wickra-exchange` (abi3 wheel)      | 3.9 (tested through 3.13)  |
| Node.js  | npm · `wickra-exchange` (N-API 8)          | 22 (tested on 22 · 24 LTS) |
| C        | `wickra_exchange.h` + library (releases)   | C99 compiler               |
| C++      | over the C ABI                             | C++14 compiler             |
| C#       | NuGet · `WickraExchange`                    | .NET 8 (`net8.0`)          |
| Go       | module · `wickra-lib/wickra-exchange-go`   | Go 1.23 (cgo)              |
| Java     | Maven Central · `org.wickra:wickra-exchange` | Java 22 (FFM / Panama)   |
| R        | r-universe · `wickraexchange`              | R ≥ 2.10 (Rtools on Win.)  |

## Ecosystem

Part of the [Wickra](https://github.com/wickra-lib/wickra) family — each one a
data-driven core with a CLI and the same ten-language binding surface:

- [**wickra**](https://github.com/wickra-lib/wickra) — main library (Rust core + Python / Node.js / WASM bindings + a C ABI for C / C++ / C# / Go / Java / R)
- [**wickra-playground**](https://github.com/wickra-lib/wickra-playground) — a polyglot strategy playground: one StrategySpec live side by side in Python, Rust, JS and Go, entirely in the browser
- [**wickra-backtest**](https://github.com/wickra-lib/wickra-backtest) — event-driven backtester over the Wickra core
- [**wickra-terminal**](https://github.com/wickra-lib/wickra-terminal) — the trading terminal: a TUI and a browser renderer over the stack
- [**wickra-screener**](https://github.com/wickra-lib/wickra-screener) — parallel multi-symbol screening over 514 streaming indicators
- [**wickra-xray**](https://github.com/wickra-lib/wickra-xray) — market-microstructure explorer: footprint, order-book heatmap, liquidation map, funding/OI divergence
- [**wickra-radar**](https://github.com/wickra-lib/wickra-radar) — perp-universe alert radar: OI delta, funding flip, book imbalance, liquidation clusters, OI/price divergence
- [**wickra-copilot**](https://github.com/wickra-lib/wickra-copilot) — local market copilot grounded in real order-book, liquidation and funding microstructure
- [**wickra-shazam**](https://github.com/wickra-lib/wickra-shazam) — match an asset's current microstructure fingerprint against its entire history
- [**wickra-benchmark**](https://github.com/wickra-lib/wickra-benchmark) — reproducible, golden-verified benchmark suite — recompute any (strategy, dataset, report) in ten languages and confirm it byte-for-byte
- [**wickra-strategy-ci**](https://github.com/wickra-lib/wickra-strategy-ci) — Jest for trading strategies: golden-pin the report, catch regressions in CI, property-test against fuzzed data
- [**wickra-verify**](https://github.com/wickra-lib/wickra-verify) — confirm or refute a claimed backtest report against its strategy and data, in ten languages
- [**wickra-proof**](https://github.com/wickra-lib/wickra-proof) — Proof-of-Backtest: deterministic (spec, data) → report + blake3 hash, recomputable byte-for-byte in ten languages
- [**wickra-zk**](https://github.com/wickra-lib/wickra-zk) — prove a backtest zero-knowledge — on-chain-verifiable performance without revealing the data or the strategy
- [**wickra-impact**](https://github.com/wickra-lib/wickra-impact) — the backtester that knows you would have moved the market: agent-based fills on the real historical L2 order book
- [**wickra-darwin**](https://github.com/wickra-lib/wickra-darwin) — evolutionary strategy search at millions of backtests per second, mutating and crossing JSON specs across the 514-indicator space
- [**wickra-gym**](https://github.com/wickra-lib/wickra-gym) — a Gymnasium-compatible, microstructure-aware backtest environment with O(1) steps for deterministic RL rollouts
- [**wickra-feature-store**](https://github.com/wickra-lib/wickra-feature-store) — OHLCV and microstructure streams into ML-ready feature matrices over 514 O(1) streaming indicators
- [**wickra-genome**](https://github.com/wickra-lib/wickra-genome) — a vector database of the whole market: every asset a 514-dim live vector, for similarity search, clustering and anomaly detection
- [**wickra-timemachine**](https://github.com/wickra-lib/wickra-timemachine) — scrub the whole market like a video — every symbol, full order book, rewound to any moment via deterministic re-fold
- [**wickra-synth**](https://github.com/wickra-lib/wickra-synth) — deterministic synthetic market microstructure: OHLCV, order book, trades and funding from a single seed
- [**wickra-compile**](https://github.com/wickra-lib/wickra-compile) — compile a strategy spec into a standalone deployable: a WASM module, a self-contained binary, or a `no_std` artifact
- [**wickra-embed**](https://github.com/wickra-lib/wickra-embed) — allocation-free, `no_std` streaming indicators for bare-metal and HFT, byte-for-byte identical to the core
- [**wickra-pico**](https://github.com/wickra-lib/wickra-pico) — the O(1) indicator core running bare-metal on a $5 Raspberry Pi Pico — the LED blinks on the EMA cross

Docs at [docs.wickra.org](https://docs.wickra.org); the marketing site and
in-browser demo at [wickra.org](https://wickra.org).

## Contributing

Contributions are welcome — issues, bug reports, ideas and pull requests all land
at <https://github.com/wickra-lib/wickra-exchange>. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the orientation: the core lives in
`crates/wickra-exchange-core`, every binding under `bindings/<lang>` keeps the
replay-parity invariant, and `cargo fmt --all` +
`cargo clippy --workspace --all-targets --all-features -- -D warnings` are CI
gates. For larger changes, open an issue first.

## Security

Found a security issue? **Please don't open a public issue.** Report it privately
via the repository's *Security* tab (*"Report a vulnerability"*) or email
**support@wickra.org**. Full policy: [SECURITY.md](SECURITY.md). The handling of
secret key material is documented in [THREAT_MODEL.md](THREAT_MODEL.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. Use it, fork it, modify it, redistribute it — commercially or
not — file issues, send pull requests; all welcome.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

## Disclaimer

Not a trading system and not financial advice. This library connects to exchanges
and can place real orders that risk real capital; any such use is **entirely at
your own risk**. Authentication, order rounding, reconnect handling and rate
limiting can fail in ways that lose money — test against testnets, use
withdrawal-disabled keys, and review the code before trading. The software is
provided **as is**, without warranty of any kind; see the license files for the
full terms.

---

<p align="center">
  Built on <a href="https://github.com/wickra-lib/wickra">Wickra</a>. If it saved you time, ⭐ the repo.
</p>

---

<p align="center">
  <a href="https://github.com/wickra-lib/wickra-exchange">
    <img alt="GitHub stars" src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/stars.svg">
  </a>
  <a href="https://github.com/wickra-lib/wickra-exchange/network/members">
    <img alt="GitHub forks" src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/forks.svg">
  </a>
  <a href="https://github.com/wickra-lib/wickra-exchange/issues">
    <img alt="GitHub issues" src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/issues.svg">
  </a>
</p>

<p align="center">
  Built on <a href="https://github.com/wickra-lib/wickra">Wickra</a>. If it saved you time, the cheapest way to say thanks is to ⭐ the repo.
</p>

<p align="center">
  <img alt="wickra-exchange star history" width="640"
       src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/star-history.svg">
</p>
