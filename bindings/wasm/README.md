<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514-7" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![npm](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/npm.svg)](https://www.npmjs.com/package/wickra-exchange-wasm)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — WASM

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for WASM. `npm install wickra-exchange-wasm` — pure WebAssembly, runs anywhere a modern JS engine does.**

WebAssembly bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange):
the offline **paper** and **replay** simulators, in the browser.

## Install

```bash
npm install wickra-exchange-wasm
```

### Building from this repository (contributors)

```bash
wasm-pack build bindings/wasm --target web    --release --features panic-hook  # browsers
wasm-pack build bindings/wasm --target nodejs --release --out-dir pkg          # Node
node --test bindings/wasm/tests/
```

The `panic-hook` feature routes Rust panics to `console.error` with a readable
stack; without it a panic surfaces as "unreachable executed" and nothing points
at the cause. It costs a little size, which is why it is off by default and on
for the browser build.

## Quick start

```js
import init, { Exchange, OrderRequest } from "wickra-exchange-wasm";

await init();

const ex = Exchange.paper({ USDT: 100_000 }, 1, 5, 10); // maker/taker/slippage bps
ex.setPrice("BTC/USDT", 20_000);

// A number is fine; a string is exact, which is what a size with more than
// about fifteen significant digits needs -- JS has one number type and it is a
// double.
const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
console.log(order.status, order.averagePrice); // "filled" 20020

console.log(ex.balances()); // { BTC: 1, USDT: 79980 }
```

Replaying a recorded tape, one frame per `pollEvents()`:

```js
const replay = Exchange.replayTrades(
  "BTC/USDT",
  Float64Array.from([100, 101, 102, 110, 112]),
  { USDT: 100_000 },
);

for (;;) {
  const events = replay.pollEvents();
  if (events.length === 0) break; // an exhausted tape yields nothing further
  for (const event of events) {
    if (event.kind === "trade") {
      // ... your strategy sees the same events a live feed produces
    }
  }
}
```

### What this package is, and what it is not

The other bindings — Node, Python, C, C#, Go, Java, R — connect to live venues.
This one cannot, and that is a property of the target rather than a gap in the
work: `wasm32-unknown-unknown` has no TCP sockets and no TLS stack, and the
transport crate is built on tokio, reqwest and tokio-tungstenite, none of which
target the browser. A `connect()` here would compile and then fail at the first
request.

So this package carries the part of the library that is pure computation and
therefore genuinely runs in a page:

| exposed | absent |
| --- | --- |
| `Exchange.paper` — offline account with fees and slippage | `connect` — needs sockets |
| `Exchange.replayTrades` — recorded price tape | user-data streams, WebSocket execution |
| `placeOrder`, `cancelOrder`, `queryOrder`, `openOrders` | derivatives (live-only) |
| `balances`, `ticker`, `setPrice`, `pollEvents` | `klines` (needs a venue) |
| `OrderRequest` factories, `version()` | depth — see below |

There is no `orderBook`. The paper account has no depth feed and answers
`unsupported`; the replay backend delegates straight to it. On both backends
reachable from here the call cannot succeed, so exposing it would only add a
method that type-checks and always throws.

The surface that *is* here is the one a backtest uses, which is the point: a
strategy written against this runs unchanged against a live venue once it moves
off the browser.

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of wasm-bindgen, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/wasm/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/wasm)

Wickra Exchange ships native bindings for Python, Node.js, WASM and Rust, plus a C ABI hub that any
C-capable language (C, C++, C#, Go, Java, R) links against — all forwarding to the
same data-driven, `unsafe`-forbidden Rust core.

## Security

Found a security issue? **Please don't open a public issue.** Report it privately
via the repository's *Security* tab (*"Report a vulnerability"*) or email
**support@wickra.org** with a subject line starting `[wickra security]`. Full
policy: <https://github.com/wickra-lib/wickra-exchange/blob/main/SECURITY.md>.

## Disclaimer

Not a trading system and not financial advice. This library connects to exchanges
and can place real orders that risk real capital; any such use is **entirely at
your own risk**. Authentication, order rounding, reconnect handling and rate
limiting can fail in ways that lose money — test against testnets, use
withdrawal-disabled keys, and review the code before trading. The software is
provided **as is**, without warranty of any kind; see the license files for the
full terms.

## License

Licensed under either of [Apache-2.0](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-MIT) at your option.
