# Roadmap

`wickra-exchange` is built out in phases, mirroring the proven structure of the
Wickra backtester. Each phase lands as reviewed, CI-green pull requests. Status
below is updated as phases complete.

## Phases

1. **Bootstrap** — workspace, core + facade crates, governance, supply-chain
   config, CI/OSSF scaffolding. *In progress.*
2. **Core** — the `Exchange`/`MarketData`/`Execution` traits, `Decimal` types,
   `Credentials`, and the connectivity machinery: options (market types), clock,
   weight-based rate limiter, retry/idempotency, instrument cache, unified symbol
   mapping, local order-book builder, positions/reconcile, observability, the
   transport abstraction, error taxonomy, HTTP/WS, and normalisation. Near-total
   coverage via the mock transport.
3. **Binance** — the full reference implementation: read-only data plus signed
   execution, per-symbol filters, advanced order ops, dead-man's-switch,
   reconcile-on-reconnect and microstructure feeds (spot + USDⓈ-M).
4. **The remaining nine venues** — Bybit, OKX, Bitget, KuCoin, HTX, Gate.io,
   Kraken, Coinbase, Upbit — each: auth + WebSocket state machine + filters +
   replay fixtures + gated testnet tests.
5. **Differentiators** — `PaperExchange` (fills via the backtest engine),
   microstructure feeds into `wickra-core` input types, and `ReplayExchange`,
   with an end-to-end `live → indicator → signal → paper-fill` demo.
6. **Bindings** — native Python and Node, plus the C ABI hub reaching C, C++, C#,
   Go, Java and R; replay parity green across all eight.
7. **Hardening** — conformance suite, property tests, fuzz targets, supply-chain
   gates, OpenSSF Scorecard and Best Practices.
8. **Ecosystem** — the `wickra-exchange-go` mirror repo and the r-universe entry.
9. **Docs, benchmarks, examples** — guides, the capability matrix, a ccxt
   migration guide, and one runnable example per language.
10. **Release** — version 0.1.0 to seven registries.

## Derivatives & advanced orders

Landed after the initial spot build (the venue clients were spot-only; a
derivatives `MarketType` only changed the host). Now futures-capable and
extended, tracked per venue in [docs/CAPABILITIES.md](docs/CAPABILITIES.md) with
a deep-dive in [docs/DERIVATIVES.md](docs/DERIVATIVES.md):

- **`Derivatives`** (positions / leverage / margin mode / reduce-only close) on
  all eight futures venues — Binance, Bybit, OKX, Bitget, KuCoin, Gate.io, HTX,
  Kraken. Coinbase and Upbit stay spot-only. The futures order lifecycle
  (`query_order` / `cancel_order` / `open_orders`) now routes to the futures
  endpoints on every venue (previously spot-shaped on Gate/Bitget/HTX/Kraken).
- **`AdvancedOrders`** (amend, batch place/cancel, OCO) + a self-trade-prevention
  field on `OrderRequest`, on all eight trading venues — native where the API
  supports it, a documented `Error::Exchange` where it does not.
- **`WsUserData`** (private account/order stream → `poll_events` surfaces the
  user's own `OrderUpdate` / `BalanceUpdate`) on all eight trading venues.
- **`WsExecution`** (order placement over the WebSocket order API) — native on
  Binance, Bybit, OKX, Gate.io and Kraken; a documented `Error::Exchange` on
  Bitget, KuCoin and HTX (no WebSocket order-entry API).
- All five surfaces are reachable through the facade factory (`connect`,
  `connect_derivatives`, `connect_advanced`, `connect_user_data`,
  `connect_ws_execution`) **and through all nine language bindings** — Python,
  Node.js, the C ABI hub and the Go / C# / Java / R wrappers over it.

### Follow-ups

- Private-stream keepalive (Binance listen-key `PUT`, KuCoin bullet token,
  per-venue re-auth on a dropped user-data stream).
- Native batch where currently sequential (KuCoin/Kraken cancel-by-id, Kraken
  `AddOrderBatch` indexed form).
- Kraken Futures WebSocket (separate challenge/response feed on
  `futures.kraken.com`).

## Non-goals

- **Breadth over ccxt.** The goal is a typed, unified API over the largest venues
  with first-class Wickra integration — not coverage of 100+ exchanges.
- **A browser/WASM binding.** Authenticated trading needs keys and raw sockets;
  the browser sandbox forbids both.
- **Withdrawals.** The default surface favours withdrawal-disabled keys.
