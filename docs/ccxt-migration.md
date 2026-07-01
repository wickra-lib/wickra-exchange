# Migrating from ccxt

ccxt is the reference multi-exchange library. wickra-exchange covers the same
core surface with a typed, streaming-native API and no per-call `async`.

## Concept mapping

| ccxt                                   | wickra-exchange                              |
|----------------------------------------|----------------------------------------------|
| `ccxt.binance({ apiKey, secret })`     | `Exchange.connect("binance", credentials)`   |
| `exchange.fetch_ticker(symbol)`        | `exchange.ticker(&symbol)`                   |
| `exchange.fetch_ohlcv(symbol, tf)`     | `exchange.klines(&symbol, "1m", 500)`        |
| `exchange.fetch_order_book(symbol)`    | `exchange.order_book(&symbol, depth)`        |
| `exchange.create_order(...)`           | `exchange.place_order(&OrderRequest::...)`   |
| `exchange.cancel_order(id, symbol)`    | `exchange.cancel_order(&symbol, id)`         |
| `exchange.fetch_open_orders(symbol)`   | `exchange.open_orders(Some(&symbol))`        |
| `exchange.fetch_balance()`             | `exchange.balances()`                        |
| `exchange.watch_trades(symbol)` (pro)  | `subscribe_trades` + `poll_events`           |

## Key differences

- **Typed, not dict-based.** `OrderRequest`, `Order`, `Ticker`, `Balance` are
  concrete types with exact `Decimal` fields — no stringly-typed maps.
- **Pull-based streaming is built in**, not a separate `pro` package. Subscribe,
  then drain `poll_events` in your own loop.
- **Symbols are `BASE/QUOTE`** (`"BTC/USDT"`), normalised per venue internally.
- **Paper and replay backends** share the exact same API, so you can validate a
  strategy offline before pointing it at a live venue — see the [Cookbook](Cookbook.md).
