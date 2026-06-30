# Contributing to wickra-exchange

Thanks for your interest. Issues, bug reports, ideas and pull requests are all
welcome at <https://github.com/wickra-lib/wickra-exchange>. For larger changes,
open an issue first so we can agree on the approach.

## Orientation

- The core — traits, types and the shared connectivity machinery — lives in
  `crates/wickra-exchange-core`. Each exchange is a module under
  `crates/wickra-exchange-core/src/exchanges/`.
- Every language binding lives under `bindings/<lang>/` and must preserve the
  **replay-parity invariant**: given the recorded responses in `golden/replay/`,
  each binding normalises them into the byte-identical structs in
  `golden/expected/`.
- The public surface is re-exported by the `wickra-exchange` facade crate.

## The dev loop

Every change runs green locally before a commit:

```bash
cargo fmt --all
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
```

`cargo fmt --all` and the `clippy -D warnings` gate are enforced in CI on three
operating systems. Tests that hit a live exchange run only against **testnets**,
are gated behind environment variables and are `#[ignore]` by default — never
add a test that uses mainnet or real keys.

## Conventions

- **Commits are signed** and follow Conventional Commits (`feat:`, `fix:`,
  `chore:`, `docs:`…). One logical change per commit. Open a PR against `main`;
  do not push to `main` directly.
- **All public artifacts are in English** — code, comments, commit messages, PR
  titles and bodies, issues and docs.
- **No secrets, ever** — not in code, tests, fixtures, logs, issues or PRs.
  Order-layer quantities use `Decimal`, not `f64`.
- **Production code only** — no mocks outside `#[cfg(test)]`, no TODO stubs, and
  no defensive branches that can never run (they fail coverage).

## Adding an exchange

A new exchange implements the `Exchange` trait (`MarketData` + `Execution`),
supplies its signing scheme with a unit test from the exchange's documented
example, registers in the `Exchange::new` factory, and ships replay fixtures plus
gated testnet integration tests. See `docs/EXCHANGES.md` and the Binance module as
the reference implementation.

## Developer Certificate of Origin

Contributions are accepted under the [DCO](DCO); sign off your commits with
`git commit -s`. By contributing you agree your work is dual-licensed under
`MIT OR Apache-2.0`.
