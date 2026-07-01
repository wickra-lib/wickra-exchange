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

## Caveats

These figures bound the library's own overhead only. End-to-end latency in a live
deployment is dominated by exchange round-trip time, rate-limit pacing and your
network path — none of which these benchmarks capture.
