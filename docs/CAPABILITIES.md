# Capability matrix

Every venue implements the full `Exchange` surface (market data + execution +
streaming). The trait is uniform by design; this document records the axes that
legitimately differ per venue, and — for derivatives and advanced orders — the
**real** per-venue support, including honestly-documented gaps.

## Core

| Venue    | Spot | Derivatives | Passphrase | Signing      | WS market data | WS order placement |
|----------|:----:|:-----------:|:----------:|--------------|:--------------:|:------------------:|
| Binance  |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |         ✅         |
| Bybit    |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |         —          |
| OKX      |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |         —          |
| Bitget   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |         —          |
| KuCoin   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |         —          |
| Gate.io  |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |         —          |
| HTX      |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |         —          |
| Kraken   |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |         —          |
| Coinbase |  ✅  |     —       |     —      | ES256 JWT    |       ✅       |         —          |
| Upbit    |  ✅  |     —       |     —      | HS512 JWT    |       ✅       |         —          |

All order types are common across venues: market, limit, stop-market,
stop-limit; time-in-force GTC / IOC / FOK; `reduce_only` and `post_only` flags.
Per-symbol filters (lot step, price tick, min-notional) are enforced through
`InstrumentFilters` before an order is sent.

> **WS user-data streams** (private account/order pushes) are **not yet
> implemented** on any venue — a documented follow-up. WS order placement
> ([`WsExecution`]) is implemented on Binance as the reference.

## Derivatives (`Derivatives` trait)

Implemented on the eight venues with futures/perpetual markets. Coinbase and
Upbit are spot-only and do not implement it. A derivatives
[`MarketType`](../crates/wickra-exchange-core/src/options.rs) selects the futures
path/host; see [DERIVATIVES.md](DERIVATIVES.md).

| Venue   | Futures routing        | positions | leverage | margin Cross | margin Isolated | close_position |
|---------|------------------------|:---------:|:--------:|:------------:|:---------------:|:--------------:|
| Binance | path `/fapi`           |    ✅     |    ✅    |      ✅      |       ✅        |      ✅        |
| Bybit   | param `category`       |    ✅     |    ✅    |      ✅      |       ✅        |      ✅        |
| OKX     | param `instType` SWAP  |    ✅     |    ✅    |      ✅      |       ✅        |      ✅        |
| Bitget  | mix `productType`      |    ✅     |    ✅    |      ✅      |       ✅        |      ✅        |
| KuCoin  | host `api-futures`     |    ✅     |   ✅¹    |      ✅      |       ✅        |      ✅        |
| Gate.io | path `/futures/usdt`   |    ✅     |    ✅    |      ✅      |       ✅        |      ✅        |
| HTX     | host `api.hbdm.com`    |    ✅     |    ✅    |      ✅      |       —²        |      ✅        |
| Kraken  | host `futures.kraken`  |    ✅³    |    ✅    |      ✅      |       —²        |      ✅        |

1. KuCoin sets leverage **per order**, not per account; `set_leverage` records it
   locally and applies it on the next futures order.
2. HTX (cross-margin swap family) and Kraken Futures (flex multi-collateral
   account) select margin mode at the account/family level, not per symbol, so
   `set_margin_mode(Isolated)` returns `Error::Exchange`.
3. Kraken `openpositions` omits mark price and unrealized PnL (reported as zero);
   leverage is the recorded preference, not a per-position field.

**Futures order-shape gaps (documented follow-ups):** on Gate, Bitget, HTX and
Kraken, `query_order` / `cancel_order` / `open_orders` still use the **spot**
order shape on a futures client (market data, `place_order`, `balances`,
`positions` and `close_position` are futures-correct).

## Advanced orders (`AdvancedOrders` trait) + STP

`AdvancedOrders` is implemented on all eight trading venues; the operation is
used where the venue supports it natively, and returns a documented
`Error::Exchange` where it does not.

| Venue   | STP¹ | amend            | batch place | batch cancel | OCO           |
|---------|:----:|------------------|:-----------:|:------------:|---------------|
| Binance |  ✅  | ✅ replace/PUT   |     ✅      |     ✅       | ✅ spot only  |
| Bybit   |  ✅  | ✅ native        |     ✅      |     ✅       | —             |
| OKX     |  ✅  | ✅ native        |     ✅      |     ✅       | ✅ algo       |
| Bitget  |  ✅  | —                |     ✅²     |     ✅²      | —             |
| KuCoin  |  ✅  | —                |     ✅      |     ✅³      | ✅ order-list |
| Gate.io |  ✅  | ✅ PATCH         |     ✅      |     ✅       | —             |
| HTX     |  —   | —                |     ✅      |     ✅       | —             |
| Kraken  |  —   | ✅ EditOrder     |     —⁴      |     ✅³      | —             |

1. Self-trade-prevention: the `stp` field on `OrderRequest` maps to the venue's
   native mode (`selfTradePreventionMode` / `smpType` / `stpMode` / `stp` /
   `stp_act`). HTX and Kraken have no spot STP field.
2. Bitget batch is spot-only here; the mix (futures) batch endpoints are not yet
   wired.
3. KuCoin and Kraken have no batch-cancel-by-id, so `cancel_batch` cancels
   sequentially.
4. Kraken's `AddOrderBatch` encodes its order array as indexed form fields, which
   does not fit this binding's form helper — place orders individually.

`place_batch` returns `Vec<Result<Order>>` so a partially-accepted batch keeps
each order's own outcome. Binance is also the reference for WebSocket order
placement ([`WsExecution`]: `place_order_ws` / `cancel_order_ws`).

> The matrix reflects the traits every client implements; object-safety and the
> naming contract are asserted for all ten venues in `tests/conformance.rs`, and
> every REST/WS path above is covered by offline mock-fixture tests.

[`WsExecution`]: ../crates/wickra-exchange-core/src/traits.rs
