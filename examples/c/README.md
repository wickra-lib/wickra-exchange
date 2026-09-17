# Wickra Exchange — C / C++ examples

The Wickra Exchange C ABI is a single shared/static library plus a generated header
([`bindings/c/include/wickra_exchange.h`](../../bindings/c/include/wickra_exchange.h)). Any C-capable
language links against the same artifact; these examples show the plain-C path
and, through [`wickra_exchange.hpp`](../../bindings/c/include/wickra_exchange.hpp), the C++ one.

## Build the library

From the workspace root:

```sh
cargo build -p wickra-exchange-c --release
```

This produces, in `target/release/`:

| Platform | Shared library | Link target |
|----------|----------------|-------------|
| Linux    | `libwickra_exchange.so`     | `-lwickra_exchange` |
| macOS    | `libwickra_exchange.dylib`  | `-lwickra_exchange` |
| Windows (MSVC) | `wickra_exchange.dll` | `wickra_exchange.dll.lib` (import lib) |

A static library (`libwickra_exchange.a` / `wickra_exchange.lib`) is emitted alongside.

## Build and run the examples

With CMake, as the CI C ABI job does:

```sh
cmake -S examples/c -B examples/c/build
cmake --build examples/c/build --config Release
ctest --test-dir examples/c/build -C Release --output-on-failure
```

## The examples

| Example | What it does |
|---------|--------------|
| `paper.cpp` | Paper-trading example from C++ (the header is `extern "C"` under __cplusplus). |
| `replay.c` | Replay-parity example: a recorded tape drives a signal that fills on the book. |

## Usage shape

Every call follows the same handle discipline: construct from a spec JSON, drive
with command JSON, read the response, free the handle exactly once. `wickra_exchange.h` is
the whole contract; the C++ header, where one ships, wraps the handle in a
move-only RAII type. See [`bindings/c/README.md`](../../bindings/c/README.md).
