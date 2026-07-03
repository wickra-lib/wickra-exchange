<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514" alt="Wickra Exchange — unified crypto-exchange connectivity for Go" width="100%"></a>
</p>

[![Built on Wickra](https://img.shields.io/badge/built%20on-wickra-3b82f6)](https://github.com/wickra-lib/wickra)
[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![Go module](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/go.svg)](https://pkg.go.dev/github.com/wickra-lib/wickra-exchange-go)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — Go

---

**Streaming-native, unified crypto-exchange connectivity for Go, over the Wickra C ABI hub via cgo.**

[Wickra Exchange](https://github.com/wickra-lib/wickra-exchange) is one
synchronous, pull-based API over the ten largest crypto exchanges — market data,
orders, positions and private user-data streams — plus offline **paper** and
**replay** simulators that share the exact same API. The same strategy runs
paper, replay and live by swapping the constructor. This package is the Go
binding: it consumes the C ABI hub through cgo, so results are byte-identical to
the Rust, Python, Node.js, C#, Java and R bindings — one connectivity kernel
behind every language.

## Install

Use the published **`wickra-exchange-go`** module, which bundles the prebuilt C
ABI library for every platform, so `go get` + `go build` works with no extra
steps (a C compiler is still required, as the binding uses cgo):

```bash
go get github.com/wickra-lib/wickra-exchange-go
```

```go
import wickraexchange "github.com/wickra-lib/wickra-exchange-go"
```

`wickra-exchange-go` is generated from this directory by the release pipeline: it
mirrors the Go sources, the vendored C ABI header (`include/wickra_exchange.h`)
and the prebuilt libraries under `lib/<goos>_<goarch>/`. On Windows the DLL must
be discoverable at run time (next to the executable or on `PATH`).

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

## Building from this repository (contributors)

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

## License

Dual-licensed under [MIT](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-MIT)
or [Apache-2.0](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-APACHE), at your option.
