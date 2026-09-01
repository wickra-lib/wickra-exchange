# Documentation

The reference documentation for this project lives in this directory, beside the
code it describes. Unlike the [`wickra`](https://github.com/wickra-lib/wickra)
core — whose per-indicator pages are generated and published to
[docs.wickra.org](https://docs.wickra.org) — this is a small, hand-written set,
because what needs explaining here is per-venue behaviour rather than a
catalogue.

## Pages

| Page | What it covers |
| --- | --- |
| [EXCHANGES.md](EXCHANGES.md) | The ten venues, their market types, and which parts of the surface each supports. |
| [AUTH.md](AUTH.md) | The signing families — HMAC-SHA256/512, JWT ES256/HS512, passphrase — and how `Credentials` map onto each. |
| [STREAMING.md](STREAMING.md) | The pull-based event model, the local order-book builder, and reconnect/resubscribe semantics. |
| [DERIVATIVES.md](DERIVATIVES.md) | The `Derivatives`, `AdvancedOrders`, `WsUserData` and `WsExecution` traits, futures routing, and per-venue gaps. |
| [CAPABILITIES.md](CAPABILITIES.md) | The real per-venue support matrix: spot/futures, positions, leverage, margin mode, amend, batch, OCO, WebSocket order entry. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, traits, the transport seam, and the design decisions behind them. |
| [Cookbook.md](Cookbook.md) | Task-shaped recipes. |
| [ccxt-migration.md](ccxt-migration.md) | What maps onto what, coming from ccxt. |

Two more live at the repository root because they describe the project rather
than the API: [`ARCHITECTURE.md`](../ARCHITECTURE.md) for the crate layout and
[`BENCHMARKS.md`](../BENCHMARKS.md) for the measured throughput of signing,
parsing and filter rounding.

## Editing

These pages are part of this repository — change them in the same pull request
as the behaviour they describe. There is no separate docs repository and no
generated content here, so a page and the code it documents cannot drift apart
across two merges.

The capability matrix in [CAPABILITIES.md](CAPABILITIES.md) is the one to watch:
it makes a claim per venue per capability, and a venue client that gains or loses
a capability has to move a cell in it.
