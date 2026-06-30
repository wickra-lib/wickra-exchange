# Architecture

`wickra-exchange` mirrors the tiering of the Wickra core and backtester: a Rust
core, native PyO3/napi bindings, and a C ABI hub that reaches C, C++, C#, Go, Java
and R — **eight languages**. There is no WASM binding (authenticated trading needs
raw sockets and secret keys; a browser sandbox forbids both).

## Crates

```
crates/
├── wickra-exchange-core/   traits, types, credentials, the shared connectivity
│                           machinery, and the per-exchange implementations
├── wickra-exchange/        facade crate — the stable public re-export
├── wickra-exchange-cli/    the `wkex` command-line client
└── wickra-exchange-bench/  criterion benchmarks (signing / parse / filter)
```

## The unified trait

One trait spans every venue; the implementation behind it is bespoke per exchange.

```
trait MarketData   klines / ticker / order book + pull-based subscribe / poll_events
trait Execution    place / cancel / query order, open orders, balances, positions
trait Exchange:    MarketData + Execution
```

`Exchange::new("<name>", credentials, options)` is the factory over all ten
venues. The *shape* is identical everywhere; the **auth scheme, WebSocket state
machine and symbol/filter mapping are unique per exchange**, which is the
irreducible core that cannot be shared.

## Two deliberate deviations from the Wickra tiering

1. **No WASM binding.** See above. The browser-safe slice — public market data —
   is already covered elsewhere (`wickra-wasm` over a browser WebSocket).
2. **Pull-based streams, not push callbacks.** Subscriptions fill a bounded
   buffer that the consumer drains with `poll_events() -> Vec<Event>`. Push
   callbacks out of a background thread need bespoke per-language glue (Python
   GIL, Go cgo, Java JNI, C# delegates) and break entirely for single-threaded R.
   Pull lets the consumer own its loop, so the C ABI carries streaming as trivially
   as a synchronous call — across all eight languages, R included.

## The transport abstraction

All network I/O goes through a `Transport` trait (HTTP + WebSocket). Production
uses a thin real-socket adapter; tests inject a `MockTransport` that replays
recorded fixtures. This is what lets the signing, parsing, filter-rounding,
WebSocket state machine, rate limiter and error paths all run **offline, under
test, to near-total coverage** — the only excluded lines are the thin real-socket
adapter itself.

## Exact arithmetic

Order-layer price and quantity use `rust_decimal::Decimal`. Floats mis-round money
(scientific notation, `1e-8` drift) and exchanges reject mis-rounded values.
Indicator inputs stay `f64`; the boundary is the order layer.

## Connectivity machinery (shared modules)

`symbol` (unified `BTC/USDT` ↔ `BTCUSDT` ↔ `XBTUSD` mapping), `clock` (server-time
sync, nonce, JWT TTL), `ratelimiter` (weight-based + 429/418 back-off),
`retry`/`idempotency`, `instruments` (exchangeInfo cache), `orderbook` (local L2
builder with diff-apply + gap detection + auto-resync), `positions`/`reconcile`,
`observability` (tracing + redaction + health), `error` (a unified taxonomy
mapping exchange codes to an enum), and `normalize`.

## Integration with the rest of Wickra

The differentiators are what make this more than "a typed ccxt":

- **`PaperExchange`** implements the same `Exchange` trait but simulates fills
  through the `wickra-backtest` engine — so a strategy runs paper ↔ live by
  swapping the implementation.
- **Microstructure feeds** (`feeds`) emit the exact typed shapes `wickra-core`
  consumes (`DerivativesTick`, `OrderBook`, `TradePrint`, `CrossSection`), feeding
  the 514 indicators and the backtester with zero glue.
- **`ReplayExchange`** drives a recorded feed through the same trait.

A Rust trading backend depends on `wickra-core`, `wickra-backtest-core` and
`wickra-exchange` as three Cargo crates and composes them in one binary, no FFI.

## Parity

Connectivity is I/O, so there is no byte-deterministic golden corpus. Parity is
proven by **replay**: recorded exchange responses in `golden/replay/`, with each
of the eight languages required to reproduce the normalised structs in
`golden/expected/`.
