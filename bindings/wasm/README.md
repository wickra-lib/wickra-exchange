# wickra-exchange-wasm

WebAssembly bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange):
the offline **paper** and **replay** simulators, in the browser.

## What this package is, and what it is not

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

## Install

```bash
npm install wickra-exchange-wasm
```

## Use

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

## Build from source

```bash
wasm-pack build bindings/wasm --target web    --release --features panic-hook  # browsers
wasm-pack build bindings/wasm --target nodejs --release --out-dir pkg          # Node
node --test bindings/wasm/tests/
```

The `panic-hook` feature routes Rust panics to `console.error` with a readable
stack; without it a panic surfaces as "unreachable executed" and nothing points
at the cause. It costs a little size, which is why it is off by default and on
for the browser build.

## Licence

`MIT OR Apache-2.0`, the same as the workspace.
