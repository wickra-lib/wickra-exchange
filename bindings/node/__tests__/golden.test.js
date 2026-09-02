"use strict";

// Golden-fixture parity for the Node binding.
//
// The Rust suite (crates/wickra-exchange-core/tests/golden.rs) drives the
// committed replay tapes in golden/ through a ReplayExchange running a fixed
// SMA strategy, and pins the fill price and the resulting balances. This runs
// the same fixtures through the same pipeline from Node.
//
// replay.test.js already proves a tape reaches a fill. What it does not do is
// check the numbers: a lost decimal, a dropped fee or slippage applied to the
// wrong side would still produce a fill, and still pass. These assert the exact
// values the Rust suite pins.
//
// The strategy is reimplemented rather than imported: pulling in the indicator
// package to average three numbers would make this a test of two libraries
// agreeing, when the thing under test is the replay-to-paper-fill pipeline.

const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");

const { Exchange, OrderRequest } = require("../index.js");

const GOLDEN = path.join(__dirname, "..", "..", "..", "golden");
const TOL = 1e-6;

function readJson(kind, name) {
  return JSON.parse(
    fs.readFileSync(path.join(GOLDEN, kind, `${name}.json`), "utf8"),
  );
}

function makeSma(window) {
  const values = [];
  return (price) => {
    values.push(price);
    if (values.length < window) return null;
    return values.slice(-window).reduce((a, b) => a + b, 0) / window;
  };
}

function runCase(name) {
  const spec = readJson("replay", name);
  const expected = readJson("expected", name);

  const exchange = Exchange.replayTrades(
    spec.market,
    spec.tape,
    spec.balances,
    spec.maker_bps,
    spec.taker_bps,
    spec.slippage_bps,
  );

  const sma = makeSma(spec.sma_period);
  let fillPrice = null;

  // Each poll advances the recording by exactly one frame; an empty batch is
  // how an exhausted tape reports itself.
  for (;;) {
    const events = exchange.pollEvents();
    if (events.length === 0) break;
    for (const event of events) {
      if (event.kind !== "trade") continue;
      const mean = sma(event.price);
      if (mean !== null && fillPrice === null && event.price > mean) {
        const order = exchange.placeOrder(
          OrderRequest.marketBuy(spec.market, 1),
        );
        fillPrice = order.averagePrice;
      }
    }
  }

  const balances = exchange.balances();

  assert.strictEqual(fillPrice !== null, expected.filled);
  assert.ok(
    Math.abs(fillPrice - expected.average_price) < TOL,
    `${name}: average price ${fillPrice} != ${expected.average_price}`,
  );
  assert.ok(
    Math.abs(balances.BTC - expected.btc) < TOL,
    `${name}: BTC ${balances.BTC} != ${expected.btc}`,
  );
  assert.ok(
    Math.abs(balances.USDT - expected.usdt) < TOL,
    `${name}: USDT ${balances.USDT} != ${expected.usdt}`,
  );
}

test("golden: sma_cross (frictionless)", () => {
  runCase("sma_cross");
});

test("golden: sma_cross_with_costs (fees + slippage)", () => {
  runCase("sma_cross_with_costs");
});
