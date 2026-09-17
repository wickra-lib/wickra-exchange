# Golden fixtures

Committed replay tapes (`replay/`) and their expected outcomes (`expected/`). The
`tests/golden.rs` suite drives each tape through `ReplayExchange` + a fixed SMA
strategy and asserts the fill price and resulting balances match the expected
file exactly — so the deterministic replay → paper-fill pipeline can never drift
silently. Regenerate the expected files only when the fill semantics change on
purpose, and review the diff.

## Layout

| Path | What it is |
|------|------------|
| `replay/<case>.json` | A replay tape: the candles the fixed SMA-cross strategy is driven over, and the cost model the case uses. |
| `expected/<case>.json` | What the run must produce: whether it filled, the average fill price and the BTC / USDT balances afterwards. |

Two cases ship, `sma_cross` (frictionless) and `sma_cross_with_costs` (with the
fee and slippage model on). Every binding checks the same two files:
`crates/wickra-exchange-core/tests/golden.rs`, `bindings/python/tests/test_golden.py`,
`bindings/node/__tests__/golden.test.js`, `bindings/wasm/tests/golden.test.js` and
`bindings/go/golden_test.go` read them from here, and the Go mirror repository
carries a copy so `go test` works from the published module.

## Blessing

There is no bless switch on purpose: an expected file is hand-checked numbers,
not a snapshot. When the fill semantics change deliberately, run the Rust suite
to see the new values, write them into `expected/<case>.json`, and let the
diff be reviewed as the semantic change it is. Every binding's golden test then
goes green on the same commit.
