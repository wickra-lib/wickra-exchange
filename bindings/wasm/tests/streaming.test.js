// The paper backend and the streaming (poll-driven) replay loop.
//
// `golden.test.js` pins the numbers against committed fixtures. This covers the
// surface around them: what each constructor accepts, what the poll loop yields
// frame by frame, and which calls are supposed to fail -- an error path that
// silently succeeds is the one that reaches a user.
//
//   wasm-pack build bindings/wasm --target nodejs --out-dir pkg
//   node --test bindings/wasm/tests/

const test = require('node:test');
const assert = require('node:assert/strict');

const W = require('../pkg/wickra_exchange_wasm.js');

test('version is the crate version', () => {
  assert.match(W.version(), /^\d+\.\d+\.\d+$/);
});

test('paper: a market buy fills at the mark plus slippage', () => {
  // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
  const ex = W.Exchange.paper({ USDT: 100000 }, 1, 5, 10);
  assert.equal(ex.name(), 'paper');

  ex.setPrice('BTC/USDT', 20000);
  const order = ex.placeOrder(W.OrderRequest.marketBuy('BTC/USDT', 1));

  assert.equal(order.status, 'filled');
  assert.equal(order.side, 'buy');
  assert.ok(Math.abs(order.averagePrice - 20020) < 1e-6, order.averagePrice);

  const balances = ex.balances();
  assert.ok(Math.abs(balances.BTC - 1) < 1e-9);
});

test('paper: the ticker reads back the mark', () => {
  const ex = W.Exchange.paper({ USDT: 1000 });
  ex.setPrice('BTC/USDT', 500);

  const ticker = ex.ticker('BTC/USDT');
  assert.equal(ticker.symbol, 'BTC/USDT');
  assert.ok(Math.abs(ticker.last - 500) < 1e-9);
  // A paper book has one price, so best bid and best ask are both the mark.
  assert.ok(Math.abs(ticker.bid - 500) < 1e-9);
  assert.ok(Math.abs(ticker.ask - 500) < 1e-9);
});

test('there is no depth method to call', () => {
  // Neither backend reachable from WASM has a depth feed -- paper answers
  // `unsupported` and replay delegates to paper -- so the binding does not
  // expose one. This pins that: a future `orderBook` here would be a method
  // that always throws.
  const ex = W.Exchange.paper({ USDT: 1000 });
  assert.equal(typeof ex.orderBook, 'undefined');
});

test('replay: each poll advances the tape by exactly one frame', () => {
  const tape = [100, 101, 102, 110, 112];
  const ex = W.Exchange.replayTrades(
    'BTC/USDT',
    Float64Array.from(tape),
    { USDT: 100000 },
  );
  assert.equal(ex.name(), 'replay');

  const prices = [];
  for (;;) {
    const events = ex.pollEvents();
    if (events.length === 0) break;
    for (const event of events) {
      if (event.kind === 'trade') prices.push(event.price);
    }
  }

  // Every recorded frame is surfaced once, in order, and then the tape stops
  // yielding -- which is what the golden loop relies on to terminate.
  assert.deepEqual(prices, tape);
  assert.equal(ex.pollEvents().length, 0);
});

test('a limit order rests and shows up in open orders', () => {
  const ex = W.Exchange.paper({ USDT: 100000 });
  ex.setPrice('BTC/USDT', 20000);

  const resting = ex.placeOrder(W.OrderRequest.limitBuy('BTC/USDT', 1, 10000));
  assert.equal(resting.status, 'new');

  const open = ex.openOrders('BTC/USDT');
  assert.equal(open.length, 1);
  assert.equal(open[0].id, resting.id);

  assert.equal(ex.queryOrder('BTC/USDT', resting.id).id, resting.id);

  ex.cancelOrder('BTC/USDT', resting.id);
  assert.equal(ex.openOrders(undefined).length, 0);
});

test('a malformed market is rejected, not silently accepted', () => {
  assert.throws(() => W.OrderRequest.marketBuy('BTCUSDT', 1), /BASE\/QUOTE/);
  assert.throws(() => W.Exchange.paper({}).setPrice('BTC', 1), /BASE\/QUOTE/);
});

test('setPrice is refused on a replay backend', () => {
  const ex = W.Exchange.replayTrades(
    'BTC/USDT',
    Float64Array.from([100]),
    { USDT: 1 },
  );
  assert.throws(() => ex.setPrice('BTC/USDT', 1), /paper exchange/);
});
