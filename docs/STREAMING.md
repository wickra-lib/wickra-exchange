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

## Health

`Health` is the same shape of thing as reconciliation: a snapshot the caller
folds, not a subscription the client fills. The pull model already hands you
every input — `Disconnected` / `Reconnected` say whether the stream is up and
how often it has come back, `TradePrint` carries the venue's timestamp,
`sync_time` returns the clock offset, and the rate budget lives in the
`ThrottledTransport` you wrapped the client with.

```rust
use wickra_exchange::Health;

let mut health = Health { clock_offset_ms: exchange.sync_time()?, ..Health::default() };
for event in exchange.poll_events() {
    match event {
        Event::Disconnected => health.connected = false,
        Event::Reconnected => { health.connected = true; health.reconnects += 1; }
        Event::Trade(ref print) => { health.last_message_ms = Some(print.timestamp); }
        _ => {}
    }
}
```

Ask it two questions, both relative to *now* rather than to the last message:
`staleness_ms(now_ms)` is the silence so far, and `is_healthy(now_ms,
max_staleness_ms)` combines it with `connected`. The pair matters because a
stream that has stopped delivering is still connected — the socket is open and
nothing is arriving, which `connected` alone cannot tell you.

Only `TradePrint` carries a venue timestamp; tickers and book updates are
identified by update id, so for those the caller's own clock stands in.

Anything you then log about a request must not carry the credential that signed
it. `redact(text, secret)` replaces every occurrence with `<redacted>`, and
ignores an empty secret so it is safe to call unconditionally:

```rust
log(&redact(&line, &api_secret));
```

A runnable version of both is
[`examples/rust/src/health_and_redaction.rs`](../examples/rust/src/health_and_redaction.rs).

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
