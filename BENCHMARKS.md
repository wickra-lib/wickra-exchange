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

Run on a single core against the recorded fixtures in `golden/replay/`, so the
numbers are reproducible and contain no network variance:

```bash
cargo bench -p wickra-exchange-bench
```

Results are published here once the benchmark harness lands; this document
intentionally contains no figures until they are produced on a pinned machine, to
avoid quoting numbers that were never measured.

## Caveats

These figures bound the library's own overhead only. End-to-end latency in a live
deployment is dominated by exchange round-trip time, rate-limit pacing and your
network path — none of which these benchmarks capture.
