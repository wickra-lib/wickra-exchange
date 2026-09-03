# Benchmarks

Connectivity throughput is dominated by the network, not by CPU, so the
benchmarks here measure the **CPU-bound work the library does per request** —
the parts that must not become a bottleneck under load — not round-trip latency
to an exchange (which is not reproducible and not ours to measure).

## What is measured

The `wickra-exchange-bench` crate (criterion) covers:

- **Request signing** — HMAC-SHA256 / HMAC-SHA512 / JWT signature construction
  per signing family, in signatures per second.
- **Response parsing** — deserialising recorded REST/WS payloads into the typed
  structs, in messages per second.
- **Filter rounding** — rounding a price/quantity to an exchange's
  lot/tick/min-notional filters with `Decimal`, in operations per second.
- **Order-book diff apply** — applying a depth diff to the local L2 book and
  detecting sequence gaps, in updates per second.

## Methodology

Run on a single core against fixed, representative in-process inputs, so the
numbers are reproducible and contain no network variance:

```bash
cargo bench -p wickra-exchange-bench
```

## Results

Measured with `cargo bench -p wickra-exchange-bench` (criterion, 100 samples per
benchmark) on an AMD Ryzen 9 9950X, single-threaded. Figures are the median
estimate; treat them as orders of magnitude, not guarantees — they will vary with
CPU and toolchain.

| Group      | Operation                          | Median   | Throughput      |
|------------|------------------------------------|----------|-----------------|
| signing    | `hmac_sha256_hex` (signed query)   | 2.15 µs  | ~465 K/s        |
| signing    | `hmac_sha512_hex`                  | 1.59 µs  | ~627 K/s        |
| signing    | `sha256` (raw digest)              | 570 ns   | ~1.75 M/s       |
| parse      | `parse_decimal`                    | 20.8 ns  | ~48 M/s         |
| parse      | `format_decimal`                   | 128 ns   | ~7.8 M/s        |
| parse      | `event_from_json` (trade frame)    | 846 ns   | ~1.18 M/s       |
| filter     | `round_quantity` (floor to step)   | 39.0 ns  | ~25 M/s         |
| filter     | `round_price` (floor to tick)      | 32.2 ns  | ~31 M/s         |
| orderbook  | `apply_snapshot` (50 levels/side)  | 3.62 µs  | ~276 K/s        |
| orderbook  | `apply_delta` (10 levels/side)     | 2.72 ns  | ~368 M/s        |

The takeaway: every hot path is comfortably faster than any exchange's rate limit
(signing a request costs ~2 µs, parsing a frame ~0.8 µs), so the library never
becomes the bottleneck — the network and the venue's limits do.

## What a binding costs

The figures above measure the library's own work. They say nothing about what it
costs to *reach* that work from another language — and a binding is a boundary:
marshalling a string, projecting a struct, crossing an FFI or an interpreter.
None of that appeared anywhere, in any language, however thin the bindings were
described as being.

`benchmarks/` runs the same two operations in every language against the same
offline paper account: `ticker`, which is almost entirely the crossing itself,
and `place_order`, which is real work on both sides of it. Same machine, same
20 000 iterations after a 1 000-call warm-up.

| Language | `ticker` | `place_order` | overhead per order |
|----------|---------:|--------------:|-------------------:|
| **Rust** (in-process, the baseline) | 62 ns | 824 ns | — |
| C (the ABI itself) | 188 ns | 1 254 ns | +0.4 µs |
| C# | 448 ns | 1 385 ns | +0.6 µs |
| Go | 400 ns | 1 553 ns | +0.7 µs |
| Python | 556 ns | 1 533 ns | +0.7 µs |
| Node.js | 1 467 ns | 2 669 ns | +1.8 µs |
| R | 1 373 ns | 3 203 ns | +2.4 µs |
| Java | 1 471 ns | 3 150 ns | +2.3 µs |

Read across the `ticker` column and the boundary is visible on its own: the C ABI
costs about 130 ns over an in-process call, and each language adds its own
marshalling on top of that. Read the `place_order` column and it is in
proportion: the work is the same everywhere, and the crossing is a share of it.

**This is not a ranking of the languages.** One exchange round trip costs on the
order of a millisecond — roughly a thousand times the widest gap in this table —
and every venue's rate limit is far below what even the slowest binding here can
produce. The number that matters is the last column: the most a binding adds to
an order is about three microseconds, so the choice of language is a choice about
the code you want to write, not about throughput.

WASM is deliberately absent: it carries the offline simulators and no venue
client, and what a call into WebAssembly costs is a property of the browser's
engine rather than of this library.

## Caveats

These figures bound the library's own overhead only. End-to-end latency in a live
deployment is dominated by exchange round-trip time, rate-limit pacing and your
network path — none of which these benchmarks capture.

The binding figures were measured on one machine (Windows, AMD Ryzen 9 9950X)
with each language's own release toolchain. They will move with the toolchain and
the platform; the shape of the table — a hundred-nanosecond boundary, a
microsecond or two of marshalling — is the part that travels.
