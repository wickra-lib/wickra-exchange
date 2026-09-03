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
| Bybit    | ✅  | ✅  | ✅  | `PostOnly`⁴ | `smpType` | futures only³ |
| OKX      | ✅  | ✅⁵ | ✅⁵ | `post_only`⁵ | `stpMode` | futures only³ |
| Bitget   | ✅  | ✅  | ✅  | `post_only`⁴ | `stpMode` | futures only³ |
| KuCoin   | ✅  | ✅  | ✅  | `postOnly` | `stp` | futures only³ |
| Gate.io  | ✅  | ✅  | ✅  | `poc`⁴ | `stp_act` | futures only³ |
| HTX      | ✅  | ✅⁶ | ✅¹ ⁶ | `limit-maker`⁶ | — refused | futures only³ |
| Kraken   | ✅  | ✅  | — refused⁷ | `oflags=post` | — refused | futures only³ |
| Coinbase | ✅  | — refused⁸ | ✅¹ | `post_only` | — refused⁹ | — (spot only) |
| Upbit    | ✅  | ✅¹ | ✅¹ | — refused | — refused | — (spot only) |

1. On a limit order. A market order is immediate by construction, so GTC and IOC
   describe what it already does and need no field — but FOK is a different
   instruction ("all of it now, or none"), and where the venue has no market
   fill-or-kill the order is refused rather than left able to fill partially.
2. `LIMIT_MAKER` is Binance's post-only type and accepts no `timeInForce` at all,
   so post-only together with IOC/FOK is refused.
3. `reduce_only` is the one field whose meaning comes from the account rather
   than the venue: it says "close, do not open", and a spot account holds
   balances, not positions, so there is nothing to close. **Every spot client
   refuses it**, on the single-order, batch and WebSocket paths alike. Binance
   says so on the wire too (-1104, "not all sent parameters were read"); the
   others accept the field and never act on it, which reaches the caller as the
   same falsehood from the other direction.
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

`tests/conformance.rs` holds every client to that table without naming it: six
contracts assert that each field is *carried or refused* on the single-order
path, on the batch path, on the WebSocket path, when two fields land in one
venue slot, when a market order asks for fill-or-kill, and — for `reduce_only`,
whose answer depends on the account — carried on a derivatives client and
refused on a spot one. A venue that gains a field later moves from one column to
the other without those tests changing.

The three order paths are three hand-written builders of the same order, so they
drift, and every round of this has found them drifted: the batch path dropping
what the single path carried, and the WebSocket frame dropping what both
carried. Each path is now held to the same table rather than to its own.

**Trigger (stop) orders now reach nine of the ten venues.**
`OrderType::StopMarket` and `StopLimit` rest until the market reaches
`stop_price`, which every venue expresses differently and five of them through a
separate endpoint entirely. A venue
that cannot carry the trigger **refuses the order** with `Error::Exchange` code
`unsupported`, rather than dropping the trigger and placing the plain order
underneath it — a stop-loss without its trigger executes at once, at the price it
existed to protect against.

| Venue | single order | batch | WebSocket |
|-------|--------------|-------|-----------|
| Binance | ✅ `stopPrice` | ✅ | ✅ |
| Bybit | ✅ `triggerPrice` + `triggerDirection` ¹ | — refused | — refused |
| Kraken | ✅ `stop-loss` / `stop-loss-limit` ² | — refused | — refused |
| Coinbase | ✅ `stop_limit_stop_limit_gtc` ³ | n/a | n/a |
| OKX | ✅ `order-algo` + `slTriggerPx` ⁴ | — refused | — refused |
| Bitget | ✅ `place-plan-order` + `triggerType` ⁵ | — refused | — refused |
| KuCoin | ✅ `stop-order` / `stopPriceType` ⁶ | — refused | — refused |
| Gate.io | ✅ `price_orders` + `rule` ⁷ | — refused | — refused |
| HTX | ✅ `algo-orders` / `swap_trigger_order` ⁸ | — refused | — refused |
| Upbit | — refused ⁹ | n/a | n/a |
| `PaperExchange` | refused (`Error::InvalidOrder`) | | |

1. Bybit will not infer which way the market must cross the trigger:
   `triggerDirection` is 1 for "rises to" and 2 for "falls to". The wrong one
   arms the order on the side that never comes, so the stop never fires — with
   no error, because the order was accepted. A **spot** trigger also needs
   `orderFilter: "StopOrder"`: the spot endpoint serves plain and conditional
   orders through one call and defaults to the plain one, so a trigger without
   it is placed immediately.
2. Kraken's trigger types **move what `price` means**: on `stop-loss-limit`,
   `price` is the trigger and `price2` the limit, where on a plain limit `price`
   *is* the limit. On Kraken Futures the trigger is its own `orderType` (`stp`)
   and the order names `triggerSignal=mark` — the price a position is liquidated
   against — rather than inheriting a default that may change.
3. Coinbase has one trigger configuration and it is a **stop-limit**; there is no
   stop-market. A `StopMarket` is refused rather than sent as a stop-limit at an
   invented price, since choosing that price would decide how much slippage the
   caller's stop may take. `stop_direction` is not inferred either.
4. OKX places triggers through `/api/v5/trade/order-algo` as an `ordType` of
   `conditional`, and answers with an `algoId` rather than an `ordId` — a
   different handle, from a different endpoint, which is what has to be given
   back to cancel it. `slOrdPx` of `-1` is OKX's spelling of "take the market",
   so a stop-market sends that sentinel rather than a price.
5. Bitget's plan orders name **which price arms them**, and will not infer it:
   futures watch `mark_price`, the price a position is liquidated against, and
   spot watches `fill_price`, since spot has no mark. `planType` is
   `normal_plan` — the other plan types belong to an already-open position
   rather than to this request.
6. KuCoin's `stop` says which way the market has to cross the trigger: `loss`
   fires when the market moves against the side, which is what a stop-loss
   means, and `entry` fires the other way, which is a breakout entry — a
   different order that happens to use the same price. Spot serves these from
   `/api/v1/stop-order`; futures takes them on the ordinary order path with a
   `stopPriceType` of `MP`, the mark.
7. Gate nests a `trigger` (when to act) inside a `put` (what to place), and
   `rule` is the comparison: `<=` protects a long, `>=` covers a short. Its
   **spot** price order can only place a limit, so a spot stop-market is
   refused rather than sent with an invented limit price — that price is how
   much slippage the caller's stop is allowed to take. Futures has no such
   limit: a price of `0` there means "take the market".
8. HTX splits them across both endpoint *and* envelope: spot posts to
   `/v2/algo-orders` and is addressed by a client order id (so one is always
   sent, since an unnamed algo order could not be cancelled), while the swap
   posts to `swap_trigger_order` with a `trigger_type` of `le`/`ge` and spells
   "take the market" as an order price *type* (`optimal_5`) rather than as a
   price. The v2 endpoints answer `code: 200` and carry no `status` at all —
   a client reading only the v1 envelope reports every placed algo order as a
   failure with an empty code and an empty message.
9. Upbit's order API exposes limit and market order types only.

> **A note on how these were built.** Every other wire shape in this repository
> was read off the venue's own socket or endpoint. An *order body* cannot be:
> placing one needs credentials and real money. The trigger fields above come
> from each venue's API documentation and are pinned by tests against the
> request this client builds — which proves the client sends what was intended,
> not that the venue accepts it. They are the only shapes here carrying that
> weaker guarantee, and they are marked as such in the code.

`tests/conformance.rs` holds all ten clients to the contract: a trigger order is
either sent with its trigger price or refused, never flattened into an immediate
one. It matches on the trigger **value** rather than a list of field names —
Kraken's trigger rides in a field every limit order also has, so no list of
spellings could tell the two apart. A venue that gains native trigger orders
later moves from one branch to the other without the test changing.

### The markets a client refuses

`MarketType` names four markets and no client here routes all four. Until
recently none of them said so: an unrouted market did not fail, it resolved to
whatever that client's URL builder produced, and the venue answered. This was
measured against the live venues, one request per client:

| Venue | asked for coin-margined, answered from | asked for margin |
|-------|----------------------------------------|------------------|
| Binance | `api/v3/ticker/24hr?symbol=BTCUSD` — the **spot** host ¹ | the spot path |
| Bybit | `category=inverse` — correct ² | the spot category |
| OKX | `BTC-USD-SWAP` — correct ² | `BTC-USD` spot |
| Bitget | `productType=USDT-FUTURES` → empty list | the spot path |
| KuCoin | `BTC-USD` on the futures host → 404 ³ | the spot path |
| Gate.io | `/futures/usdt/` → `CONTRACT_NOT_FOUND` ⁴ | the spot path |
| HTX | `linear-swap-ex` → `invalid-parameter` ⁵ | the spot path |
| Kraken | `PF_XBTUSD` — its **linear** perpetual ⁶ | the spot path |
| Coinbase | — (no coin-margined product) | the spot path |
| Upbit | `USD-BTC` → 404 (Upbit is spot-only) | the spot path |

1. The worst of them, because it *works*: `BTCUSD` is a real Binance **spot**
   pair, so a coin-margined request returned real spot prices with no error and
   nothing to notice, and an order would have bought spot BTC with USD instead
   of opening an inverse position. Binance's coin-margined API is a different
   host and prefix entirely (`dapi.binance.com/dapi/v1`).
2. Bybit and OKX are the two that would have routed the *data* correctly, both
   verified against the live venues. They are refused with the rest anyway: an
   inverse order's size is denominated differently — Bybit's inverse `qty` is in
   USD, not in the base coin — so `quantity` would silently mean something else
   on those two clients than on every other one. Half a market is the defect
   this refusal exists to prevent, not a smaller version of the feature.
3. KuCoin's inverse contracts are named `XBTUSDM`, not `BTC-USD`.
4. Gate settles inverse contracts under `/futures/btc/`, not `/futures/usdt/`.
5. HTX serves coin-margined swaps from `swap-ex`, not `linear-swap-ex`.
6. `PF_XBTUSD` and `PI_XBTUSD` both exist and both answer; `PF_` is the linear
   multi-collateral perpetual and `PI_` the coin-margined one.

**Margin was routed nowhere at all** — every client sent it down its plain spot
path, where a margin order becomes an ordinary spot order and the borrow the
caller asked for simply does not happen.

The guard sits at the seams that reach a venue: the HTTP helpers and each
WebSocket connect. That is a handful of places per client rather than forty
public methods, and it is where "nothing was sent" can actually be asserted.
`tests/conformance.rs` holds every client to it — market data, an order and a
stream, per unrouted market, each refused with `Error::Exchange` code
`unsupported` and **no request recorded**. Removing any single guard fails it.

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

### The path a binding can build it on

A field the caller can *name* is not yet a field that reaches the venue: it also
has to reach it on the path the caller chose. Three of them send an order, and
they were not equal.

| | single | batch | WebSocket |
|---|:---:|:---:|:---:|
| Rust | ✅ | ✅ | ✅ |
| Python, Node | ✅ | ✅ | ✅ |
| Go | ✅ | ✅ | ✅ |
| C#, Java, R | ✅ | ✅ | ✅ |
| WASM | ✅ | — ¹ | — ¹ |
| C / C++ | ✅ | ✅ | ✅ |

1. WASM targets `wasm32-unknown-unknown`, which has no sockets, so it carries
   the offline subset on purpose and has neither path.

The C ABI has carried `wickra_advanced_place_batch_full` and
`wickra_ws_place_order_full` since the `WickraOrderRequest` struct arrived —
and until now **only Go called them**. C#, Java and R each reached the narrow
four-argument forms on both paths, so a batched or socket-sent order from those
three could be a market or a limit and nothing else, while their single-order
path carried all six fields and the surface check reported the contract whole.

That is the same shape twice over: the check counted whether a field appeared
*anywhere* in a binding, the way it once counted whether a verb appeared without
asking what the verb could express. It now checks per path, so a binding that
reaches a field on one path and not another fails rather than averaging out.

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
> `CoinMFutures` and `Margin`, and no client routes either. **Every venue client
> now refuses an unrouted market before it sends anything**, rather than
> answering from whichever market its URL builder happened to name — see
> [the markets a client refuses](#the-markets-a-client-refuses) below for what
> each one answered instead. Coinbase and Upbit serve spot only and refuse
> linear futures on the same grounds.
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
> subscribe, HTX v2 auth, Kraken token) so `poll_events` surfaces the user's own
> `OrderUpdate` / `BalanceUpdate` events. Available on the eight trading venues;
> Coinbase and Upbit are spot-only and do not implement it.
>
> **A futures client watches the futures account**, which on four venues is a
> different socket with different topics, and on three of those was refused
> until recently rather than pointed at spot — a futures order never appears in
> the spot account, so watching it is waiting for a fill that cannot arrive.
> KuCoin negotiates its bullet token against the futures host and takes
> `/contractMarket/tradeOrders` + `/contractAccount/wallet`; Gate takes
> `futures.orders` + `futures.balances` on `fx-ws.gateio.ws`, addressed to a
> **user id** the spot channels do not need; HTX takes `orders_cross.*` +
> `accounts_cross.*` on the same notification socket its public derivatives
> channels use, separated by authentication rather than by host; Kraken uses the
> separate `futures.kraken.com` challenge/response feed. Each frame shape differs
> from its spot counterpart in a way that fails quietly if read as the spot one —
> KuCoin's `done` covers a fill *and* a cancel, Gate carries a signed size and
> what is *left*, HTX numbers the lifecycle 1..11 — so each is parsed on its own
> terms.
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
> That the frame carries what the REST body carries is now a test rather than an
> intention: `the_websocket_path_carries_every_field_too` holds all five sockets
> to the same field table as the REST and batch paths. It was written because the
> claim was not true — Binance's frame named the hedged `positionSide` and never
> `reduceOnly`, so a one-way futures close sent over the socket opened a position
> instead of closing one.
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
