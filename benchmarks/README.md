# Binding benchmarks

`crates/wickra-exchange-bench` measures the library's own work — signing,
parsing, filter rounding, order-book diffs. This directory measures something
else: **what it costs to reach that work from another language.**

A binding is a boundary. Marshalling a string, projecting a struct, crossing an
FFI or an interpreter — all of it is real, none of it appeared anywhere, and the
bindings were described as thin without a number to say how thin.

## What is measured

The same two operations in every language, against the same offline paper
account, with the same iteration count (20 000, after a 1 000-call warm-up):

- **`ticker`** — the cheapest real call there is. Almost all of it is the
  crossing itself, so this is the boundary's own cost.
- **`place_order`** — a market order that fills. Real work on both sides of the
  boundary, so the crossing is a *share* of it rather than all of it.

The Rust program is the same two operations called in-process. **The difference
between a language and the Rust baseline is that binding's overhead**; the
baseline itself is what the work costs when there is no boundary at all.

## Running them

Build the C ABI first — six of the eight need it:

```bash
cargo build --release -p wickra-exchange-c
```

| Language | Command |
|----------|---------|
| Rust (baseline) | `cargo run -p wickra-exchange-bench --bin binding_baseline --release` |
| C        | `cmake -S benchmarks/c -B benchmarks/c/build && cmake --build benchmarks/c/build --config Release`, then run `binding_cost` |
| Python   | `python benchmarks/python/binding_cost.py` (after `maturin develop --release`) |
| Node.js  | `node benchmarks/node/binding_cost.js` (after `npm run build` in `bindings/node`) |
| C#       | `dotnet run -c Release --project benchmarks/csharp/BindingCost.csproj` |
| Go       | `cd benchmarks/go && go run binding_cost.go` |
| Java     | `javac -d out -cp <binding-classes> benchmarks/java/BindingCost.java`, then `java --enable-native-access=ALL-UNNAMED -Dnative.lib.dir=target/release -cp "<binding-classes>;out" BindingCost` |
| R        | `Rscript benchmarks/r/binding_cost.R` (after `R CMD INSTALL bindings/r`) |

On Windows the C ABI DLL has to be findable: `target/release` on `PATH`, which is
what the loader uses in place of an rpath.

**WASM is not here.** It targets `wasm32-unknown-unknown`, which has no sockets,
so it carries the offline paper and replay simulators and no venue client — and
what it costs to call into WebAssembly from JavaScript is a property of the
browser's engine rather than of this library.

Every program prints the same two lines, so the outputs can be read side by side:

```text
ticker               62 ns/op       16199579 ops/s
place_order         824 ns/op        1213003 ops/s
```

## Reading them

These are **not** a ranking of the languages. They measure one boundary crossing
each, in isolation, with no network anywhere. In a live deployment a single
exchange round trip costs on the order of a millisecond — a thousand times the
widest gap in this table — and every venue's rate limit is far below what even
the slowest binding here can produce.

What they are good for is the question they answer: whether a binding adds
something worth thinking about (it does not — the largest is about three
microseconds per order) and whether one of them has quietly become an outlier.

The measured results are in [`../BENCHMARKS.md`](../BENCHMARKS.md) beside the
library's own figures.
