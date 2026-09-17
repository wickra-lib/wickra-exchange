<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514-7" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![npm](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/npm.svg)](https://www.npmjs.com/package/wickra-exchange)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — Node.js

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for Node.js. `npm install wickra-exchange` — prebuilt native binary, no system dependencies.**

Node.js bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange):
streaming-native, unified connectivity for the ten largest crypto exchanges, with
offline paper and replay simulators that share the exact same API.

## Install

```bash
npm install wickra-exchange
```

The native addon ships as a prebuilt binary per platform (Linux, macOS,
Windows — x64 and arm64), selected automatically through optional
dependencies. There is nothing to compile.

### Building from this repository (contributors)

```bash
npm install
npm run build   # regenerates index.js, index.d.ts and the native .node
npm test
```

Licensed under `MIT OR Apache-2.0`.

## Quick start

```js
const { Exchange, Credentials, OrderRequest } = require("wickra-exchange");

// Offline paper account — deterministic, network-free.
const ex = Exchange.paper({ USDT: 100000 }, 1, 5); // maker/taker bps
ex.setPrice("BTC/USDT", 20000);
const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
console.log(order.status); // "filled"
console.log(ex.balances());

// Replay a recorded tape through the same API:
const rex = Exchange.replayTrades("BTC/USDT", [100, 101, 110], { USDT: 10000 });
let events;
while ((events = rex.pollEvents()).length) {
  for (const event of events) {
    // drive your strategy
  }
}

// Live venue (needs API keys):
//   const creds = new Credentials("key", "secret");
//   const live = Exchange.connect("binance", creds);
```

The same strategy runs **paper, replay and live** by swapping the constructor.

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of napi-rs, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/node/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/node)

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
