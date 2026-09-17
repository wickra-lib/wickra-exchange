# Wickra Exchange examples

Runnable examples in every supported language. Each `paper_trade` example is the
differentiator demo — an offline paper account fills orders deterministically
through the same API a live venue uses.

## Rust — `examples/rust/`

| Example | What it does |
| --- | --- |
| `src/health_and_redaction.rs` | Folding the event stream into a health snapshot, and keeping secrets out of whatever you log. |
| `src/paper_trade.rs` | The differentiator demo: an offline paper account fills orders deterministically through the same `Exchange` API a live venue uses. |
| `src/reconcile_after_reconnect.rs` | Reconciling order state after a dropped stream. |
| `src/ticker.rs` | Fetch a live public ticker from a venue (no API keys needed for public market data). |

## C / C++ — `examples/c/`

Build the library first (`cargo build -p wickra-exchange-c --release`), then build and run
the examples via CMake, as the CI C ABI job does:

```bash
cmake -S examples/c -B examples/c/build
cmake --build examples/c/build --config Release
ctest --test-dir examples/c/build -C Release --output-on-failure
```

| Example | What it does |
| --- | --- |
| `paper.cpp` | Paper-trading example from C++ (the header is `extern "C"` under __cplusplus). |
| `replay.c` | Replay-parity example: a recorded tape drives a signal that fills on the book. |

## C# — `examples/csharp/`

| Example | What it does |
| --- | --- |
| `Program.cs` | Paper-trade differentiator demo. |

## Go — `examples/go/`

| Example | What it does |
| --- | --- |
| `paper_trade.go` | Paper-trade differentiator demo. |

## R — `examples/r/`

| Example | What it does |
| --- | --- |
| `paper_trade.R` | A runnable example against this binding. |

## Java — `examples/java/`

| Example | What it does |
| --- | --- |
| `PaperTrade.java` | Paper-trade differentiator demo. |

## Python — `examples/python/`

| Example | What it does |
| --- | --- |
| `paper_trade.py` | Paper-trade differentiator demo. |

## Node.js — `examples/node/`

| Example | What it does |
| --- | --- |
| `paper_trade.js` | Paper-trade differentiator demo. |

## WASM — `examples/wasm/`

Build the WASM package, serve the repository root, and open the page in a browser;
the module script inside it is what runs (CI parses it with `node --check`):

```bash
wasm-pack build bindings/wasm --target web
python -m http.server 8000     # then open http://localhost:8000/examples/wasm/
```

| Example | What it does |
| --- | --- |
| `paper_trade.html` | A runnable example against this binding. |

## Example datasets

The examples are self-contained: the spec and the input are inline, so there is
no shared `data/` directory to load. The cross-language golden fixtures, which
every binding is checked against byte for byte, live in [`../golden/`](../golden).
