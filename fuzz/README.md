# Fuzzing wickra-exchange

[`cargo-fuzz`](https://rust-fuzz.github.io/book/cargo-fuzz.html) harnesses for
the entry points that take input from somewhere other than the caller: a venue's
JSON, a WebSocket frame, a symbol string, a credential. Everything a remote
server can put on the wire is untrusted, and none of it may panic across the C
ABI, where a panic is undefined behaviour.

Fuzzing requires a nightly Rust toolchain. The crate is its own detached
workspace, because cargo-fuzz builds it with sanitiser flags that must not reach
the rest of the tree.

## Setup

```bash
cargo install cargo-fuzz
rustup toolchain install nightly
```

## Targets

| Target | What it exercises |
| --- | --- |
| `response_parse` | The JSON response path: arbitrary bytes through serde deserialization of the public wire types and through the decimal parser. |
| `ws_frame` | Arbitrary text frames deserialized into the public streaming event types. A malformed frame must yield a clean error, never a panic. |
| `orderbook_diff` | Local order-book maintenance: an arbitrary snapshot followed by a stream of arbitrary diffs, exercising sequence-gap detection and ladder invariants. |
| `filter_round` | Instrument-filter rounding over a value and realistic `(step, tick)` increments. Rounding must not panic and must stay on the grid when stepping is active. |
| `credentials_parse` | Credential construction and validation, and the `FromStr` symbol parser, over arbitrary strings. |

## Running

```bash
cargo +nightly fuzz run response_parse
cargo +nightly fuzz run ws_frame -- -max_total_time=60
```

CI runs each target for a few seconds in the `Fuzz (smoke)` job. That is a
regression check on the harnesses — that they still build and still survive a
short run — not a campaign. Finding new inputs needs a long run with a
persistent corpus on dedicated hardware.

## Corpora

`corpus/` and `artifacts/` are ignored. A crash reproducer worth keeping belongs
in a regression test in the crate it came from, not in this directory: a test
runs on every push, a corpus file only runs when somebody fuzzes.
