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

Market and limit orders are common across venues, and per-symbol filters (lot
step, price tick, min-notional) are enforced through `InstrumentFilters` before
an order is sent.

### Order fields

The rest of the request is **not** uniform, and this table says exactly where it
differs. Every field is either sent to the venue or the order is refused with
`Error::Exchange` code `unsupported`; none is ever dropped on the way, because an
order missing a field the caller set is a different order, not a smaller one.

| Venue    | GTC | IOC | FOK | `post_only` | `stp` | `reduce_only` |
|----------|:---:|:---:|:---:|:-----------:|:-----:|:-------------:|
| Binance  | ✅  | ✅¹ | ✅¹ | `LIMIT_MAKER`² | `selfTradePreventionMode` | futures only³ |
| Bybit    | ✅  | ✅  | ✅  | `PostOnly`⁴ | `smpType` | ✅ |
| OKX      | ✅  | ✅⁵ | ✅⁵ | `post_only`⁵ | `stpMode` | ✅ |
| Bitget   | ✅  | ✅  | ✅  | `post_only`⁴ | `stpMode` | ✅ |
| KuCoin   | ✅  | ✅  | ✅  | `postOnly` | `stp` | futures only |
| Gate.io  | ✅  | ✅  | ✅  | `poc`⁴ | `stp_act` | futures only |
| HTX      | ✅  | ✅⁶ | ✅¹ ⁶ | `limit-maker`⁶ | — refused | futures only |
| Kraken   | ✅  | ✅  | — refused⁷ | `oflags=post` | — refused | futures only |
| Coinbase | ✅  | — refused⁸ | ✅¹ | `post_only` | — refused⁹ | — (spot only) |
| Upbit    | ✅  | ✅¹ | ✅¹ | — refused | — refused | — (spot only) |

1. On a limit order. A market order is immediate by construction, so GTC and IOC
   describe what it already does and need no field — but FOK is a different
   instruction ("all of it now, or none"), and where the venue has no market
   fill-or-kill the order is refused rather than left able to fill partially.
2. `LIMIT_MAKER` is Binance's post-only type and accepts no `timeInForce` at all,
   so post-only together with IOC/FOK is refused.
3. Spot holds balances, not positions, and rejects `reduceOnly` outright (-1104).
4. The venue spells post-only as a *value* of its time-in-force field, so the two
   share one slot: post-only together with a non-GTC time-in-force is refused.
5. OKX carries all three inside `ordType` (`ioc`, `fok`, `post_only` are order
   types, not modifiers), so any two of them at once are refused.
6. HTX folds the kind, the time-in-force and post-only into one `<side>-<kind>`
   string (`buy-ioc`, `buy-limit-fok`, `buy-limit-maker`).
7. Kraken's `timeinforce` is GTC, IOC or GTD — it has no fill-or-kill.
8. Coinbase's limit configurations are `limit_limit_gtc`, `_gtd` and `_fok`;
   there is no `limit_limit_ioc`.
9. Coinbase sets self-trade prevention per portfolio, not per order.

`tests/conformance.rs` holds every client to that table without naming it: four
contracts assert that each field is *carried or refused* on the single-order
path, on the batch path, when two fields land in one venue slot, and when a
market order asks for fill-or-kill. A venue that gains a field later moves from
one column to the other without those tests changing.

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

### The order a binding can build

Every language can express every field of an `OrderRequest`. This was not true
until recently, and the gap is worth naming because no check caught it: the
`place_order` **verb** was present in all seven bindings the whole time, while
the **order** it could build was a market or limit with a quantity and a price
and nothing else. A stop-loss could not be placed from any language but Rust,
however carefully the venue clients carried the trigger.

| | Rust | Python | Node | WASM | C / C++ | C# | Go | Java | R |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| market / limit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `stop_price` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `time_in_force` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `post_only` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `reduce_only` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `stp` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `client_order_id` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

The three native bindings (Python, Node, WASM) hold `OrderRequest` directly and
carry the core's builders. The C-ABI languages pass a `WickraOrderRequest`
struct, in whatever shape suits them: a widened struct in Go, a record with
`init` properties in C#, a record with `with*` builders in Java, named arguments
in R. `place_market` / `place_limit` remain everywhere as the shortest spelling
of the common case.

`scripts/check_binding_surface.py` reads the field list out of `OrderRequest`
itself and holds every binding to it, as a third axis beside verbs and
configuration. A field added to the core is a field the check demands.

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
> **What that check did not cover, and what it cost.** It read the canonical
> verb set and held every binding to it — so it counted *verbs*, not reachable
> *configurations*. Every binding carried `place_order`, and every binding built
> its exchange client with `MarketType::Spot` hardcoded. The full surface was
> present and half the markets were unreachable: no caller in Python, Node, C,
> C++, C#, Go, Java or R could place a futures order, read a futures book, or
> cancel a futures order. The derivatives, advanced-orders, user-data and
> ws-execution constructors each chose their own market, which is why nothing
> looked missing. Every binding's exchange constructor now takes the market, and
> the two per-order modes with it — `margin_mode` and `position_mode` are
> carried on the order by six venues between them, so neither can be set after
> the first order is placed.
>
> Only **spot** and **USDⓈ-margined futures** are offered. `MarketType` also has
> `CoinMFutures` and `Margin`, and no client routes either consistently: Binance
> treats coin-margined as spot outright (its `is_futures` is USDⓈ-only and its
> base URL falls through to the spot host), five venues route it to their USDT
> futures path, and only Bybit maps it to a genuine inverse category. `Margin`
> is routed nowhere. A binding that offered them would hand a caller a name that
> does not describe where the order goes, which is the defect the parameter
> exists to end — so they are refused, loudly, rather than silently downgraded
> to spot.
>
> The check now covers the axes as well as the verbs: it reads each binding's
> exchange constructor — wherever that language declares it, including Go's
> `Options` struct — and fails if `market`, `margin_mode` or `position_mode` is
> absent from it. The search is confined to the constructor on purpose. A
> whole-file search would have passed on the broken code, because
> `MarketType::Spot` appeared in every binding *because of* the bug; a check
> that passes today and would not have caught yesterday's defect manufactures
> assurance rather than providing it.
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
