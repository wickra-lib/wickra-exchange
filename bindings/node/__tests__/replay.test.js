"use strict";

// Replay-parity test: a recorded tape drives a signal that fills on the book,
// mirroring the Rust end-to-end test (recorded_tape_drives_indicator_signal_to_a_fill).

const test = require("node:test");
const assert = require("node:assert");
const { Exchange, OrderRequest } = require("../index.js");

function makeSma(window) {
  const values = [];
  return (price) => {
    values.push(price);
    if (values.length < window) return null;
    const recent = values.slice(-window);
    return recent.reduce((a, b) => a + b, 0) / window;
  };
}

test("replay tape drives signal to a fill", () => {
  const tape = [100, 101, 102, 110, 112];
  const ex = Exchange.replayTrades("BTC/USDT", tape, { USDT: 100000 });
  assert.strictEqual(ex.name(), "replay");

  const sma = makeSma(3);
  let bought = false;

  for (;;) {
    const events = ex.pollEvents();
    if (events.length === 0) break;
    for (const event of events) {
      if (event.kind !== "trade") continue;
      const mean = sma(event.price);
      if (mean !== null && !bought && event.price > mean) {
        const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
        assert.strictEqual(order.status, "filled");
        bought = true;
      }
    }
  }

  assert.ok(bought, "the rising tape should have crossed the SMA");
  assert.ok(Math.abs(ex.balances().BTC - 1) < 1e-9);
});

test("replay finishes and stops yielding", () => {
  const ex = Exchange.replayTrades("BTC/USDT", [100, 101], { USDT: 1000 });
  let seen = 0;
  for (;;) {
    const events = ex.pollEvents();
    if (events.length === 0) break;
    seen += events.length;
  }
  assert.ok(seen >= 2);
});
