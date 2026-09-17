<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514-7" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![Go module](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/go.svg)](https://pkg.go.dev/github.com/wickra-lib/wickra-exchange-go)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — Go

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for Go. `go get github.com/wickra-lib/wickra-exchange-go` — over the C ABI via cgo, prebuilt library bundled in the module.**

[Wickra Exchange](https://github.com/wickra-lib/wickra-exchange) is one
synchronous, pull-based API over the ten largest crypto exchanges — market data,
orders, positions and private user-data streams — plus offline **paper** and
**replay** simulators that share the exact same API. The same strategy runs
paper, replay and live by swapping the constructor. This package is the Go
binding: it consumes the C ABI hub through cgo, so results are byte-identical to
the Rust, Python, Node.js, C#, Java and R bindings — one connectivity kernel
behind every language.

## Install

Use the published **`wickra-exchange-go`** module, which bundles the prebuilt C ABI
library for every platform, so `go get` + `go build` works with no extra steps
(a C compiler is still required, as the binding uses cgo):

```bash
go get github.com/wickra-lib/wickra-exchange-go
```

`wickra-exchange-go` is generated from this directory by the release pipeline: it mirrors
the Go sources, the vendored C ABI header (`include/wickra_exchange.h`) and the prebuilt
libraries under `lib/<goos>_<goarch>/`. On Linux/macOS the library path is baked
in via rpath; on Windows the DLL must be discoverable at run time (next to the
executable or on `PATH`).

### Building from this repository (contributors)

This `bindings/go` directory is the development source. To build it directly,
compile the C ABI hub and stage the library into the per-platform directory cgo
links against:

```bash
cargo build -p wickra-exchange-c --release
mkdir -p bindings/go/lib/linux_amd64                       # match your GOOS_GOARCH
cp target/release/libwickra_exchange.so    bindings/go/lib/linux_amd64/    # Linux
cp target/release/libwickra_exchange.dylib bindings/go/lib/darwin_arm64/   # macOS (arm64)
cp target/release/wickra_exchange.dll      bindings/go/lib/windows_amd64/  # Windows
```

Then, with the library on the loader path, run `go test ./...` from this
directory.

## Quick start

```go
package main

import (
	"fmt"

	wickraexchange "github.com/wickra-lib/wickra-exchange-go"
)

func main() {
	// A paper exchange: simulated fills, no keys, no network.
	ex, _ := wickraexchange.Paper(map[string]float64{"USDT": 100000}, 0, 5, 0)
	defer ex.Close()

	ex.SetPrice("BTC/USDT", 20000)
	order, _ := ex.PlaceMarket("BTC/USDT", wickraexchange.Buy, 1)
	fmt.Println(order.IsFilled()) // true
}
```

The same API drives **paper, replay and live** — swap the constructor. Errors
wrap the engine message; no panic crosses the FFI boundary. See the
[repository](https://github.com/wickra-lib/wickra-exchange) for the full surface
(market data, order lifecycle, derivatives, private user-data and execution
streams).

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of cgo over the C ABI, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/go/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/go)

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
