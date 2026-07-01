# Derivatives & advanced orders

Beyond the uniform `Exchange` surface (market data + spot execution +
streaming), wickra-exchange exposes three optional trait surfaces for
derivatives trading and richer order control. Each is object-safe and
implemented per venue where the underlying API supports it; per-venue gaps are
documented honestly (see [CAPABILITIES.md](CAPABILITIES.md) for the matrix).

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

## `WsExecution` — order placement over the WebSocket API

Lower-latency placement over a venue's `ws-api`. Implemented on Binance as the
reference; the request is signed exactly like REST, wrapped in a
`{id, method, params}` frame, and exchanged on a dedicated connection opened
lazily on first use.

```rust
use wickra_exchange::WsExecution;

let order = binance.place_order_ws(&req)?;
binance.cancel_order_ws(&sym, &order.id)?;
```

`place_order_ws` requires a WebSocket transport (`with_ws`); without one it
returns `Error::NotConnected`. Other venues' ws-api placement follows the same
pattern and is a documented follow-up.

## Honest gaps

- **Futures order shape** on Gate, Bitget, HTX and Kraken: `query_order` /
  `cancel_order` / `open_orders` still use the spot order shape on a futures
  client. Market data, `place_order`, `balances`, `positions` and
  `close_position` are futures-correct.
- **WS user-data streams** (private pushes) are not yet implemented on any venue.
- **WS order placement** beyond Binance, and native batch on venues currently
  falling back to sequential (KuCoin/Kraken cancel, Bitget futures batch,
  Kraken `AddOrderBatch`), are follow-ups.
