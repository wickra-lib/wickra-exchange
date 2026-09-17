<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514-7" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![GitHub release](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/release.svg)](https://github.com/wickra-lib/wickra-exchange/releases/latest)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — C / C++

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for C / C++. `cargo build -p wickra-exchange-c --release` — a prebuilt shared/static library plus a generated `wickra_exchange.h`, no system dependencies.**

The C ABI for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange) —
the hub every C-capable language (C, C++, C#, Go, Java, R) links against. It
exposes the crate's synchronous, pull-based API over an opaque handle with plain
`int32_t` status codes; no memory crosses the boundary except the handle.

## Install

Grab the prebuilt header + library for your platform from the
[GitHub releases](https://github.com/wickra-lib/wickra-exchange/releases) — each archive
has `wickra_exchange.h`, the C++ wrapper where the binding ships one, and the shared/static
library — or build from source:

```bash
cargo build -p wickra-exchange-c --release
# -> target/release/libwickra_exchange.{so,dylib} or wickra_exchange.dll (+ import lib) + a staticlib
```

Then compile against the header and link the library.

### Building from this repository (contributors)

```bash
cargo build --release -p wickra-exchange-c
# regenerate the header after any ABI change:
cbindgen --config bindings/c/cbindgen.toml --crate wickra-exchange-c \
         --output bindings/c/include/wickra_exchange.h
```

See `examples/c/` for a C (`replay.c`) and C++ (`paper.cpp`) consumer plus a
`CMakeLists.txt` that links this library.

Licensed under `MIT OR Apache-2.0`.

## Quick start

[`examples/c/replay.c`](https://github.com/wickra-lib/wickra-exchange/blob/main/examples/c/replay.c) is the runnable example the CI smoke job executes; in full:

```c
/* Replay-parity example: a recorded tape drives a signal that fills on the book.
 *
 * Mirrors the Rust/Python/Node end-to-end tests: a rising price tape breaks
 * above a 3-period moving average, and the resulting market buy fills on the
 * paper book. Build with the CMakeLists.txt in this directory. */

/* These programs double as the C-side test suite: ctest runs them and a failed
 * expectation must fail the build. CI builds with `--config Release`, and on a
 * multi-config generator that defines NDEBUG -- which turns every assert below
 * into nothing at all, so the Windows runs were asserting no expectation while
 * reporting success. Undefining it before <assert.h> keeps the checks live in
 * every configuration. */
#undef NDEBUG
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

#include "wickra_exchange.h"

int main(void) {
    const double tape[] = {100.0, 101.0, 102.0, 110.0, 112.0};
    const size_t n_tape = sizeof(tape) / sizeof(tape[0]);

    const char *assets[] = {"USDT"};
    const double amounts[] = {100000.0};

    WickraExchange *ex =
        wickra_replay_new("BTC/USDT", tape, n_tape, assets, amounts, 1, 0.0, 0.0, 0.0);
    assert(ex != NULL);

    char name[32];
    wickra_exchange_name(ex, name, sizeof(name));
    assert(strcmp(name, "replay") == 0);

    double window[3];
    size_t seen = 0;
    int bought = 0;

    for (;;) {
        WickraEvent events[8];
        int count = wickra_exchange_poll(ex, events, 8);
        if (count <= 0) {
            break;
        }
        for (int i = 0; i < count; i++) {
            if (events[i].kind != WICKRA_EVENT_TRADE) {
                continue;
            }
            double price = events[i].price;
            window[seen % 3] = price;
            seen++;
            if (seen >= 3) {
                double mean = (window[0] + window[1] + window[2]) / 3.0;
                if (!bought && price > mean) {
                    WickraOrder order;
                    int rc = wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 1.0,
                                                          &order);
                    assert(rc == WICKRA_OK);
                    assert(order.status == WICKRA_STATUS_FILLED);
                    bought = 1;
                }
            }
        }
    }

    assert(bought);

    double btc = 0.0;
    wickra_exchange_balance(ex, "BTC", &btc);
    printf("filled; BTC balance = %.4f\n", btc);
    assert(fabs(btc - 1.0) < 1e-9);

    wickra_exchange_free(ex);
    printf("replay-parity example OK\n");
    return 0;
}
```

### Contract

- Construct a client: `wickra_paper_new(...)`, `wickra_replay_new(...)` or
  `wickra_connect(...)` — each returns an opaque `WickraExchange*` (or `NULL` on
  bad arguments).
- Every call returns `WICKRA_OK` (0) or a negative `WICKRA_ERR_*` code. Results
  are written into caller-owned `WickraOrder` / `WickraEvent` out-parameters.
- `wickra_exchange_poll(h, out, cap)` drains up to `cap` events into `out` and
  returns the count.
- Release the handle with `wickra_exchange_free(h)` — one free per constructor.

Panics abort (the release profile is built with `panic = "abort"`), so nothing
unwinds across the boundary. The header `include/wickra_exchange.h` is generated
by cbindgen and committed; CI fails if it drifts from the source.

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of the C ABI itself, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/c/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/c)

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
