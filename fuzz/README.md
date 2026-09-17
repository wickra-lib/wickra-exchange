# Fuzzing Wickra Exchange

[`cargo-fuzz`](https://rust-fuzz.github.io/book/cargo-fuzz.html) harnesses for
the entry points that take input from somewhere other than the caller: a venue's
JSON, a WebSocket frame, a symbol string, a credential. Everything a remote
server can put on the wire is untrusted, and none of it may panic across the C
ABI, where a panic is undefined behaviour.

## Setup

```bash
cargo install cargo-fuzz
rustup toolchain install nightly-2026-07-01
```

The date is the family's fuzz nightly, pinned in `ci.yml`: a rolling `nightly`
regressed with a codegen ICE unrelated to this code, so every repository moves
the date together, on purpose.

## Targets

| Target | What it exercises |
| --- | --- |
| `response_parse` | The JSON response-parsing path: arbitrary bytes are fed to serde deserialization of the public wire types and to the decimal parser. |
| `credentials_parse` | Credential construction/validation and symbol parsing with arbitrary strings. |
| `filter_round` | The instrument-filter rounding with a value and realistic (step, tick) increments. |
| `ws_frame` | The WebSocket-frame parsing path: arbitrary text frames are deserialized into the public streaming event types. |
| `orderbook_diff` | The local order-book maintenance: apply an arbitrary snapshot, then a stream of arbitrary diffs. |

## Run

```bash
# From the repository root:
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu response_parse
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu credentials_parse
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu filter_round
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu ws_frame
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu orderbook_diff
```

Each run continues until a crash is found or it is interrupted. A short
time-boxed smoke run is what CI does:

```bash
cargo +nightly-2026-07-01 fuzz run --target x86_64-unknown-linux-gnu response_parse -- -max_total_time=30
```

The expectation for every target is that it never panics: malformed or
adversarial input must surface as an `Err` or an in-band error, never a crash.
