# wickra.exchange (R)

R bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI (`.Call`): one synchronous, pull-based API over the ten
largest crypto exchanges, plus offline paper and replay simulators that share the
same API.

```r
library(wickraexchange)

ex <- wkex_paper(c(USDT = 100000), taker_bps = 5)
wkex_set_price(ex, "BTC/USDT", 20000)
order <- wkex_place_market(ex, "BTC/USDT", "buy", 1)
order$status          # "filled"
wkex_balance(ex, "BTC")  # 1
```

## Installing

`configure` fetches the C ABI for your platform from the GitHub release matching
this package's version, stages it into `src/`, and `install.libs.R` bundles it
beside the compiled object — so the installed package carries its own native
library and needs nothing on `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH` or `PATH`.

```r
install.packages("wickraexchange", repos = "https://wickra-lib.r-universe.dev")
```

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
