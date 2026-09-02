# Derivatives & advanced orders

Beyond the uniform `Exchange` surface (market data + spot execution +
streaming), wickra-exchange exposes four optional trait surfaces —
`Derivatives`, `AdvancedOrders`, `WsUserData` and `WsExecution` — for
derivatives trading and richer order control. Each is object-safe and
implemented per venue where the underlying API supports it; per-venue gaps are
documented honestly (see [CAPABILITIES.md](CAPABILITIES.md) for the matrix). All
four are reachable through the facade factory and surfaced through every
language binding (Python, Node.js, the C ABI hub, and the Go / C# / Java / R
wrappers over it).

## Market type selects the futures API

A client is spot or futures at construction, via the `MarketType` in
`ExchangeOptions`:

```rust
use wickra_exchange::{Binance, ExchangeOptions, MarketType};

let opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
let mut binance = Binance::with_credentials(transport, &opts, creds);
```

`MarketType::UsdMFutures` (or `CoinMFutures`) does more than change a host — it
routes **every** endpoint to the venue's futures API and parses the futures
response shapes, which differ from spot. The routing style depends on the venue:

- **Path-based** — a different path or host per market: Binance (`/api/v3` vs
  `/fapi/v1`,`/fapi/v2`), Gate.io (`/api/v4/futures/usdt/*`), HTX
  (`api.hbdm.com` + `/linear-swap-*`), Kraken (Kraken Futures at
  `futures.kraken.com`, a separate product with its own signing).
- **Param-based** — one unified endpoint plus a market parameter: Bybit
  (`category=linear`), OKX (`instType=SWAP`), Bitget (`productType=USDT-FUTURES`).
- **Separate host** — KuCoin Futures lives at `api-futures.kucoin.com` with
  contract symbols (`BTC/USDT` → `XBTUSDTM`).

## `Derivatives` — positions, leverage, margin, close

```rust
use wickra_exchange::{Derivatives, MarginMode, Symbol};

let sym = Symbol::new("BTC", "USDT");
let positions = binance.positions(Some(&sym))?;      // Vec<Position>, flats omitted
binance.set_leverage(&sym, 10)?;
binance.set_margin_mode(&sym, MarginMode::Cross)?;
let flatten = binance.close_position(&sym)?;          // reduce-only market order
```

A `Position` carries `symbol`, `side` (Long/Short), `quantity`, `entry_price`,
`mark_price`, `leverage`, `unrealized_pnl` and `margin_mode`. `close_position`
reads the open position and submits a reduce-only market order on the opposite
side.

**Venue notes.** KuCoin sets leverage per order (recorded locally, applied on the
next order). OKX and Bybit couple leverage with margin mode, so each setter reads
the current value to preserve the other. HTX (cross-margin swap family) and
Kraken Futures (flex account) do not switch margin mode per symbol, so
`set_margin_mode(Isolated)` returns `Error::Exchange`. Kraken `openpositions`
omits mark price and unrealized PnL.

### Margin mode is carried on the order, on two venues

For most venues margin mode is purely an account setting, and `set_margin_mode`
is the whole of it. **OKX and Bitget are not**: every order carries the mode as
a field — OKX as `tdMode`, Bitget as `marginMode` — and the value on the order
is what applies. So both halves have to agree, and the client keeps them in
step from either direction:

```rust
let mut opts = ExchangeOptions::mainnet(MarketType::UsdMFutures);
opts.margin_mode = MarginMode::Isolated;   // every order goes out isolated
let mut okx = Okx::with_credentials(transport, &opts, creds);

okx.set_margin_mode(&sym, MarginMode::Cross)?;  // and later orders follow
```

`ExchangeOptions.margin_mode` seeds it, and `set_margin_mode` updates it, so a
client never sends an order whose margin mode contradicts what was configured
or last set. OKX spot is unaffected — there `tdMode` is always `cash`.

## `AdvancedOrders` — amend, batch, OCO, and STP

```rust
use wickra_exchange::{AdvancedOrders, OcoRequest, OrderRequest, OrderSide, SelfTradePrevention};

// Self-trade prevention is a field on OrderRequest (applied by place_order):
let req = OrderRequest::limit_buy(sym.clone(), qty, price)
    .with_stp(SelfTradePrevention::ExpireMaker);

// Amend a resting order in place (native where supported):
let amended = binance.amend_order(&sym, "123", Some(new_price), Some(new_qty))?;

// Batch place — the outer Result covers transport; each inner Result is one order:
let results = binance.place_batch(&[order_a, order_b])?;
binance.cancel_batch(&sym, &["1".into(), "2".into()])?;

// One-cancels-other bracket (take-profit + stop):
let legs = binance.place_oco(&OcoRequest::new(sym, OrderSide::Sell, qty, tp, stop))?;
```

`SelfTradePrevention` (`None`/`ExpireMaker`/`ExpireTaker`/`ExpireBoth`) maps to
each venue's native mode. `place_batch` returns `Vec<Result<Order>>` so a
partially-accepted batch still surfaces the successes. Where a venue lacks an
operation natively, the method returns a documented `Error::Exchange` rather than
a fragile emulation — consult the matrix before relying on amend/OCO on a given
venue.

## `WsUserData` — private account/order stream

`subscribe_user_data` opens the account's private WebSocket stream so
`poll_events` also surfaces the user's own `OrderUpdate` / `BalanceUpdate`
events. Implemented on the eight trading venues (Binance listen key,
Bybit/OKX/Bitget signed login, KuCoin bullet-private token, Gate signed
subscribe, HTX v2 auth, Kraken token → `executions`/`balances`); Coinbase and
Upbit are spot-only and do not implement it. The Kraken **futures** client uses
the separate Kraken Futures feed (`wss://futures.kraken.com/ws/v1`) with
challenge/response auth → `open_orders`/`balances`.

```rust
use wickra_exchange::{WsUserData, connect_user_data};

let mut client = connect_user_data("binance", creds, &opts)?;
client.subscribe_user_data()?;
loop {
    client.keepalive_user_data()?;                 // periodic: keep the session alive
    for event in client.poll_events() { /* OrderUpdate / BalanceUpdate ... */ }
}
```

**Keepalive & auto-reconnect.** `keepalive_user_data` keeps the private stream
alive — Binance refreshes its listen key (`PUT`), the others send the venue's
private ping frame — so it is not dropped for inactivity; call it periodically.
If the stream is dropped anyway, the next `poll_events` re-subscribes it with
**fresh** signed auth (re-signed login / re-fetched token, never a stale replay)
and emits `Event::Disconnected` then `Event::Reconnected`, so a consumer that
only polls still recovers transparently.

## `WsExecution` — order placement over the WebSocket API

Lower-latency placement over a venue's WebSocket order API; the request is
exchanged on a dedicated connection opened (and authenticated) lazily on first
use. Native on **Binance, Bybit, OKX, Gate.io and Kraken**. Bitget, KuCoin and
HTX expose no WebSocket order-entry API, so their `place_order_ws` /
`cancel_order_ws` return a documented `Error::Exchange` pointing to REST.

```rust
use wickra_exchange::{WsExecution, connect_ws_execution};

let mut client = connect_ws_execution("bybit", creds, &opts)?;
let order = client.place_order_ws(&req)?;
client.cancel_order_ws(&sym, &order.id)?;
```

`place_order_ws` requires a WebSocket transport (`with_ws`); without one it
returns `Error::NotConnected`.

## Derivatives feeds: typed but not yet subscribed

The `feeds` module carries typed shapes for the derivatives microstructure
channels — `FundingRate`, `OpenInterest`, `Liquidation`, `LongShortRatio` and
`MarkIndex` — and `DerivativesTickBuilder` folds them into the
`wickra_core::DerivativesTick` the perpetual-futures indicator family consumes:

```rust
use wickra_exchange::{DerivativesFeed, DerivativesTickBuilder, FundingRate};

let mut builder = DerivativesTickBuilder::new();
builder.apply(&DerivativesFeed::Funding(funding_rate));  // a frame you supply
let tick = builder.build()?;                             // -> DerivativesTick
```

**No venue client subscribes to these channels.** The trade and depth streams
are wired on all ten venues and emit `TradePrint` / `OrderBookSnapshot`
directly; the derivatives channels are not, so the frames above are ones the
caller obtains and applies. The shapes and the fold are here so that a caller
who has the data does not write the conversion by hand — not because the client
fetches it.

`Derivatives` (above) is a different surface: `positions`, `set_leverage`,
`set_margin_mode` and `close_position` are account operations and **are**
implemented, on all eight futures venues.

## Honest gaps

These two remain because the venue API does not exist — they are documented, not
faked:

- **KuCoin `cancel_batch`** cancels sequentially: KuCoin has no
  batch-cancel-by-id endpoint (order-list cancel is by list id only).
- **Kraken Futures `WsExecution`** stays REST-only: Kraken Futures has no
  WebSocket order-entry API. (Its `WsUserData` **is** wired, over the separate
  `futures.kraken.com` challenge/response feed.)

Everything else in this document is implemented: private-stream keepalive +
automatic reconnect on all eight venues, Kraken native `place_batch` /
`cancel_batch`, and Kraken Futures user-data.
