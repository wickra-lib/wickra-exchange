# wickra-exchange WASM examples

Browser demos for the `wickra-exchange-wasm` binding.

The WASM surface is deliberately smaller than the others: `wasm32-unknown-unknown`
has no sockets, so no live venue client exists there, and signed execution needs
secret keys a browser sandbox has no business holding. What it does carry is the
part that needs neither — the offline `PaperExchange` and `ReplayExchange`, with
the same order, balance and event API the live clients expose. See
[bindings/wasm/README.md](../../bindings/wasm/README.md) for the full boundary.

## Build

The module ships as a `wasm-pack` `--target web` bundle. Build it once from the
repository root:

```bash
wasm-pack build bindings/wasm --target web --release --features panic-hook
```

That writes `bindings/wasm/pkg/` with the `.wasm` binary, the JS loader and the
TypeScript types. The demos import the loader via
`../../bindings/wasm/pkg/wickra_exchange_wasm.js`.

## Serve

ES-module imports need a real HTTP origin, not `file://`. Any static server from
the repository root works:

```bash
python -m http.server 8000
```

Then open `http://localhost:8000/examples/wasm/paper_trade.html`.

## Demos

| File | What it does |
| --- | --- |
| `paper_trade.html` | Seeds an offline paper account, sets a mark price, places a market buy and prints the fill, the resulting balances and the execution events drained from `pollEvents()`. The page counterpart of `examples/node/paper_trade.js` and `examples/rust/src/paper_trade.rs`. |

## See also

- [examples/README.md](../README.md) — the same scenario in every other language.
- [docs/STREAMING.md](../../docs/STREAMING.md) — the pull-based event model the
  demo drains, which is identical here and against a live venue.
