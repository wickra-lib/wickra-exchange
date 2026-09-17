<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![r-universe](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/r-universe.svg)](https://wickra-lib.r-universe.dev)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — R

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for R. `install.packages("wickraexchange", repos = "https://wickra-lib.r-universe.dev")` — over the C ABI via `.Call`, prebuilt library fetched on install.**

R bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI (`.Call`): one synchronous, pull-based API over the ten
largest crypto exchanges, plus offline paper and replay simulators that share the
same API.

## Install

From r-universe:

```r
install.packages("wickraexchange", repos = "https://wickra-lib.r-universe.dev")
```

The package's `configure` downloads the prebuilt C ABI library for this exact
version from the GitHub release and bundles it, so an ordinary install needs
nothing but a C toolchain (Rtools on Windows) for the thin `.Call` glue layer. To
build against a local checkout instead, point it at the header and library with
the environment variables below.

`configure` fetches the C ABI for your platform from the GitHub release matching
this package's version, stages it into `src/`, and `install.libs.R` bundles it
beside the compiled object — so the installed package carries its own native
library and needs nothing on `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH` or `PATH`.

To build against a locally built C ABI instead — after
`cargo build -p wickra-exchange-c --release` — point `WKEX_INC` at the header
directory and `WKEX_LIB` at the library directory, and `configure` uses those
rather than downloading:

```sh
WKEX_INC=/path/to/bindings/c/include \
WKEX_LIB=/path/to/target/release \
  R CMD INSTALL bindings/r
```

There is no WebAssembly build. This package is a network client, and
`wasm32-unknown-emscripten` has no sockets; `configure` says so and stops rather
than producing something that compiles and then cannot connect. The offline
paper and replay simulators are published separately as the
[`wickra-exchange-wasm`](https://github.com/wickra-lib/wickra-exchange/blob/main/bindings/wasm/README.md) npm package.

The same strategy runs **paper, replay and live** by swapping the constructor.
Licensed under `MIT OR Apache-2.0`.

## Quick start

```r
library(wickraexchange)

ex <- wkex_paper(c(USDT = 100000), taker_bps = 5)
wkex_set_price(ex, "BTC/USDT", 20000)
order <- wkex_place_market(ex, "BTC/USDT", "buy", 1)
order$status          # "filled"
wkex_balance(ex, "BTC")  # 1
```

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of R's native `.Call` interface over the C ABI, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/r/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/r)

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
