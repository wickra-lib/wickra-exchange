# wickra-exchange (C ABI)

The C ABI for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange) —
the hub every C-capable language (C, C++, C#, Go, Java, R) links against. It
exposes the crate's synchronous, pull-based API over an opaque handle with plain
`int32_t` status codes; no memory crosses the boundary except the handle.

## Contract

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

## Build

```bash
cargo build --release -p wickra-exchange-c
# regenerate the header after any ABI change:
cbindgen --config bindings/c/cbindgen.toml --crate wickra-exchange-c \
         --output bindings/c/include/wickra_exchange.h
```

See `examples/c/` for a C (`replay.c`) and C++ (`paper.cpp`) consumer plus a
`CMakeLists.txt` that links this library.

Licensed under `MIT OR Apache-2.0`.
