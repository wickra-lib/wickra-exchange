# Capability matrix

Every venue implements the full `Exchange` surface (market data + execution +
streaming). The trait is uniform by design; this document records the axes that
legitimately differ per venue, and — for derivatives and advanced orders — the
**real** per-venue support, including honestly-documented gaps.

## Core

| Venue    | Spot | Derivatives | Passphrase | Signing      | WS market data | WS user data | WS order placement |
|----------|:----:|:-----------:|:----------:|--------------|:--------------:|:------------:|:------------------:|
| Binance  |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |         ✅         |
| Bybit    |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |         ✅         |
| OKX      |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |         ✅         |
| Bitget   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |        —¹          |
| KuCoin   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |        —¹          |
| Gate.io  |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |      ✅      |         ✅         |
| HTX      |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |        —¹          |
| Kraken   |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |      ✅      |         ✅         |
| Coinbase |  ✅  |     —       |     —      | ES256 JWT    |       ✅       |      —       |         —          |
| Upbit    |  ✅  |     —       |     —      | HS512 JWT    |       ✅       |      —       |         —          |

1. Bitget, KuCoin and HTX expose no WebSocket order-entry API (their WebSocket
   surface is subscription-only). `WsExecution::place_order_ws` /
   `cancel_order_ws` return a documented `Error::Exchange` pointing to REST.

Market and limit orders are common across venues, with time-in-force
GTC / IOC / FOK and the `reduce_only` and `post_only` flags. Per-symbol filters
(lot step, price tick, min-notional) are enforced through `InstrumentFilters`
before an order is sent.

**Trigger (stop) orders are the exception, and only Binance carries them.**
`OrderType::StopMarket` and `StopLimit` rest until the market reaches
`stop_price`, which every venue expresses through a different field and several
through a separate endpoint entirely. Only the Binance client sends the trigger
(`stopPrice`, on all three of its order paths). **Every other venue refuses the
order** with `Error::Exchange` code `unsupported`, rather than dropping the
trigger and placing the plain order underneath it — a stop-loss without its
trigger executes at once, at the price it existed to protect against.

| Venue | trigger orders |
|-------|----------------|
| Binance | ✅ `stopPrice` on REST, batch and the WebSocket API |
| every other venue | refused (`Error::Exchange`, code `unsupported`) |
| `PaperExchange` | refused (`Error::InvalidOrder`) |

`tests/conformance.rs` holds all ten clients to that contract: a trigger order
is either sent with its trigger price or refused, never flattened into an
immediate one. A venue that gains native trigger orders later moves from one
branch to the other without the test changing.

> **Full read/execution surface in every binding.** The complete `MarketData`
> surface (`ticker`, `klines`, `order_book`, `subscribe_trades` /
> `subscribe_book` / `subscribe_ticker`, `poll_events`) and `Execution` surface
> (`place_order`, `cancel_order`, `query_order`, `open_orders`, `balances`) are
> reachable from **all nine languages** — Rust, Python, Node.js, and C / C++ /
> C# / Go / Java / R over the C ABI hub. What pins that is
> `scripts/check_binding_surface.py`, which reads the canonical verb set out of
> `traits.rs`, holds every binding's source to it, and runs in CI as its own job:
> a dropped method fails the build. Python and Node additionally assert it at
> run time. (The C-ABI `order_book` projects the bid/ask levels; the venue
> sequence id stays on the native Rust/Python/Node path.)
>
> The **WASM** binding is deliberately outside this claim. It targets
> `wasm32-unknown-unknown`, which has no sockets, so it carries the offline
> paper and replay simulators and no live venue client at all — see
> [bindings/wasm](../bindings/wasm/README.md).

> **WS user-data streams** ([`WsUserData`]) push the account's own order and
> balance updates: `subscribe_user_data` opens a private stream (Binance listen
> key, Bybit/OKX/Bitget signed login, KuCoin bullet-private token, Gate signed
> subscribe, HTX v2 auth, Kraken token; the Kraken **futures** client uses the
> separate `futures.kraken.com` challenge/response feed) so `poll_events` surfaces
> the user's own `OrderUpdate` / `BalanceUpdate` events. Available on the eight
> trading venues; Coinbase and Upbit are spot-only and do not implement it.
> `keepalive_user_data` keeps the stream alive (Binance listen-key `PUT`, KuCoin
> bullet-token refresh via re-subscribe, per-venue ping frame); a dropped stream
> is also recovered automatically on the next `poll_events`, which re-subscribes
> with fresh signed auth and emits `Event::Disconnected` then `Event::Reconnected`.
>
> **WS order placement** ([`WsExecution`]: `place_order_ws` / `cancel_order_ws`)
> is native on Binance, Bybit, OKX, Gate.io and Kraken over each venue's
> WebSocket order API; Bitget, KuCoin and HTX have no such API and return a
> documented `Error::Exchange`. Coinbase and Upbit do not implement it.
>
> A WebSocket order carries the same flags its own REST sibling does. On four of
> the five the frame uses the venue's REST field names, so the mapping is the
> same one — Binance `selfTradePreventionMode`, Bybit `smpType`, OKX `stpMode`
> and `reduceOnly`, Gate `stp_act`. **Kraken is the exception**: its v2 frame
> names every field differently (`order_qty`, `limit_price`, `cl_ord_id`), so
> the REST spelling of post-only (`oflags=post`) proves nothing about the
> WebSocket one. A post-only order over Kraken's WebSocket returns
> `Error::Exchange` and points at REST, rather than being placed as a limit that
> may take liquidity.
>
> All three surfaces are reachable through the facade factory
> (`connect`, `connect_derivatives`, `connect_advanced`, `connect_user_data`,
> `connect_ws_execution`) **and through all nine language bindings** — Python,
> Node.js, the C ABI hub, and the Go / C# / Java / R wrappers over it.

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

### Position mode

`ExchangeOptions.position_mode` says whether the account holds one net position
per symbol (`OneWay`) or a long and a short at once (`Hedge`). It is not an
account setting this library changes — it is a statement of how the account is
already configured, and what a hedged account changes is **every order**: each
one has to name the side it acts on, under a different field name per venue.

| Venue   | hedge field                       | one-way | hedge |
|---------|-----------------------------------|:-------:|:-----:|
| Binance | `positionSide` (`reduceOnly` out) |   ✅    |  ✅   |
| Bybit   | `positionIdx` 1/2                 |   ✅    |  ✅   |
| OKX     | `posSide` long/short              |   ✅    |  ✅   |
| Bitget  | `tradeSide` open/close            |   ✅    |  ✅   |
| Gate.io | `auto_size` close_long/close_short |  ✅    |  ✅   |
| HTX     | `direction` + `offset`            |   ✅    |  ✅¹  |
| KuCoin  | —                                 |   ✅    |  —²   |
| Kraken  | —                                 |   ✅    |  —²   |

1. HTX needs no branch: its swap orders already carry `direction` (buy/sell)
   **and** `offset` (open/close), which is the hedged encoding, and the venue
   accepts the same shape in one-way mode.
2. KuCoin Futures and Kraken Futures hold one net position per contract and
   have no hedge mode, so no field on the order could carry a side. A futures
   order from a client configured `Hedge` returns `Error::Exchange` rather than
   moving the net position — the same treatment as `set_margin_mode(Isolated)`
   above.

The side is derived, not asked for: `OrderRequest` carries `side` and
`reduce_only`, and buying opens the long side or closes the short one. The
mapping lives in `PositionSide::for_order` and is applied identically on every
venue.

**Futures order lifecycle:** `query_order` / `cancel_order` / `open_orders` now
route to the futures order endpoints on all eight futures venues — including Gate
(`/futures/usdt/orders`), Bitget (mix `/api/v2/mix/order/*`), HTX
(`/linear-swap-api/v1/swap_cross_*`) and Kraken Futures
(`/derivatives/api/v3/*`) — so a futures client reads back, lists and cancels
**futures** orders (previously these four used the spot order shape). Market
data, `place_order`, `balances`, `positions` and `close_position` were already
futures-correct.

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
| Kraken  |  —   | ✅ EditOrder     |     ✅⁴     |     ✅⁴      | —             |

1. Self-trade-prevention: the `stp` field on `OrderRequest` maps to the venue's
   native mode (`selfTradePreventionMode` / `smpType` / `stpMode` / `stp` /
   `stp_act`). HTX and Kraken have no spot STP field.
2. Bitget batch routes to the mix (futures) batch endpoints
   (`/api/v2/mix/order/batch-place-order` / `batch-cancel-orders`) on a futures
   client and to the spot endpoints otherwise.
3. KuCoin has no batch-cancel-by-id endpoint, so `cancel_batch` cancels
   sequentially.
4. Kraken spot batches natively: `place_batch` → `AddOrderBatch` (indexed
   `orders[i][…]` form array), `cancel_batch` → `CancelOrderBatch`. The Kraken
   **futures** client has no batch-cancel endpoint, so its `cancel_batch` is
   sequential.

`place_batch` returns `Vec<Result<Order>>` so a partially-accepted batch keeps
each order's own outcome.

> The matrix reflects the traits every client implements; object-safety and the
> naming contract are asserted for all ten venues in `tests/conformance.rs`, and
> every REST/WS path above is covered by offline mock-fixture tests.

[`WsExecution`]: ../crates/wickra-exchange-core/src/traits.rs
[`WsUserData`]: ../crates/wickra-exchange-core/src/traits.rs
