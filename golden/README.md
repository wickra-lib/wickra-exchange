# Golden fixtures

Committed replay tapes (`replay/`) and their expected outcomes (`expected/`). The
`tests/golden.rs` suite drives each tape through `ReplayExchange` + a fixed SMA
strategy and asserts the fill price and resulting balances match the expected
file exactly — so the deterministic replay → paper-fill pipeline can never drift
silently. Regenerate the expected files only when the fill semantics change on
purpose, and review the diff.
