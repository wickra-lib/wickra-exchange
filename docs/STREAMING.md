# Streaming model

wickra-exchange is **pull-based**. The public API is synchronous — a real client
drives an async socket internally, but every call (and every language binding)
blocks, so the consumer owns its loop. This is what lets the C ABI carry
streaming to every binding, including single-threaded R, as a plain call.

## Subscribe, then poll

```rust
use wickra_exchange::{MarketData, Symbol};

exchange.subscribe_trades(&Symbol::new("BTC", "USDT"))?;
loop {
    for event in exchange.poll_events() {
        // handle Trade / BookSnapshot / BookDelta / OrderUpdate / BalanceUpdate
    }
}
```

- `subscribe_trades` / `subscribe_book` / `subscribe_ticker` open a subscription
  that fills an internal buffer.
- `poll_events` drains everything buffered since the last call and returns an
  empty vector when nothing is pending (never blocks).
- Order-book streams maintain a local ladder via `OrderBookBuilder`, which
  detects sequence gaps and signals a resync (`BookUpdate::Gap`).

## Events

`Event` is a tagged enum: `Trade`, `Ticker`, `BookSnapshot`, `BookDelta`,
`OrderUpdate`, `BalanceUpdate`, `Subscribed`, `Disconnected`, `Reconnected`.
Execution events (order/balance updates) flow through the same `poll_events`
drain as market data.

## Reconnect and the dead-man's-switch

When the peer closes a stream, the client transparently **reconnects and replays
every subscription** — the consumer only sees a `Disconnected` followed by a
`Reconnected` event, and the buffer keeps filling.

A reconnect is also the moment to check what the account did while you were not
watching. The client cannot do this for you: it knows what the venue lists right
now, not what *you* believed was open. So the two halves are yours to join —
`Event::Reconnected` tells you when, and `reconcile_orders` compares your list of
open order ids against `open_orders`:

```rust
if matches!(event, Event::Reconnected) {
    let diff = reconcile_orders(&believed_open, &exchange.open_orders(None)?);
    for id in &diff.vanished {
        // Believed open here, not open at the venue: it filled or cancelled
        // unseen. Fetch its final state before trusting any position or risk
        // figure derived from it.
        let order = exchange.query_order(&market, id)?;
    }
}
```

`vanished` is the half that matters. `appeared` means an order exists that this
client did not place — another session, or an acknowledgement that was lost.

A runnable version, offline and deterministic, is
[`examples/rust/src/reconcile_after_reconnect.rs`](../examples/rust/src/reconcile_after_reconnect.rs):
a replay tape can carry `Disconnected` / `Reconnected` exactly as a live stream
emits them, so the control flow there is the one you would write against a
venue.

For live trading, pair that with a **dead-man's-switch** (`DeadMansSwitch`): arm
it and feed it a heartbeat on every message; if the deadline passes without one,
`is_expired` fires and you cancel every resting order (via the venue's cancel-all
endpoint, or `PaperExchange::cancel_all` in simulation) so nothing works
unattended after a disconnect.

```rust
use wickra_exchange::DeadMansSwitch;
use std::time::Duration;

let mut guard = DeadMansSwitch::new(Duration::from_secs(10));
guard.heartbeat(now_ms);            // on every successful message
if guard.is_expired(now_ms) {
    exchange_cancel_all();          // heartbeat lost -> pull all orders
}
```
