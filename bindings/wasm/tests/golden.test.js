// Golden-fixture parity for the WASM binding.
//
// The Rust suite (`crates/wickra-exchange-core/tests/golden.rs`) drives the
// committed replay tapes through a `ReplayExchange` running a fixed SMA
// strategy and pins the fill price and resulting balances. This runs the same
// fixtures through the same pipeline from JavaScript, so a binding that
// silently changed the numbers -- a lost decimal, a dropped fee, a slippage
// applied on the wrong side -- fails here rather than in someone's backtest.
//
// The strategy is reimplemented rather than imported: pulling in the indicator
// package to compute a three-period mean would make this a test of two
// libraries agreeing, when the thing under test is the replay-to-paper-fill
// pipeline.
//
//   wasm-pack build bindings/wasm --target nodejs --out-dir pkg
//   node --test bindings/wasm/tests/

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const W = require('../pkg/wickra_exchange_wasm.js');

const GOLDEN = path.join(__dirname, '..', '..', '..', 'golden');

function readJson(kind, name) {
  return JSON.parse(
    fs.readFileSync(path.join(GOLDEN, kind, `${name}.json`), 'utf8'),
  );
}

/// A simple-moving-average over the last `period` values; `null` until full.
function sma(period) {
  const window = [];
  return (value) => {
    window.push(value);
    if (window.length > period) window.shift();
    if (window.length < period) return null;
    return window.reduce((a, b) => a + b, 0) / period;
  };
}

function runCase(name) {
  const input = readJson('replay', name);
  const expected = readJson('expected', name);

  const exchange = W.Exchange.replayTrades(
    input.market,
    Float64Array.from(input.tape),
    input.balances,
    input.maker_bps,
    input.taker_bps,
    input.slippage_bps,
  );

  const mean = sma(input.sma_period);
  let fillPrice = null;

  // Each poll advances the recording by exactly one frame and returns it; an
  // empty batch is how an exhausted tape reports itself.
  for (;;) {
    const events = exchange.pollEvents();
    if (events.length === 0) break;
    for (const event of events) {
      if (event.kind !== 'trade') continue;
      const average = mean(event.price);
      if (average !== null && fillPrice === null && event.price > average) {
        const order = exchange.placeOrder(
          W.OrderRequest.marketBuy(input.market, 1),
        );
        fillPrice = order.averagePrice;
      }
    }
  }

  const balances = exchange.balances();
  const tol = 1e-6;

  assert.equal(fillPrice !== null && fillPrice !== undefined, expected.filled);
  assert.ok(
    Math.abs(fillPrice - expected.average_price) < tol,
    `${name}: average price ${fillPrice} != ${expected.average_price}`,
  );
  assert.ok(
    Math.abs(balances.BTC - expected.btc) < tol,
    `${name}: BTC ${balances.BTC} != ${expected.btc}`,
  );
  assert.ok(
    Math.abs(balances.USDT - expected.usdt) < tol,
    `${name}: USDT ${balances.USDT} != ${expected.usdt}`,
  );
}

test('wasm golden: sma_cross (frictionless)', () => {
  runCase('sma_cross');
});

test('wasm golden: sma_cross_with_costs (fees + slippage)', () => {
  runCase('sma_cross_with_costs');
});
