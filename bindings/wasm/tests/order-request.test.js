// Order-field parity for the WASM binding.
//
// `OrderRequest` used to expose four factories -- market/limit, buy/sell -- and
// nothing else, so an order from the browser was a quantity and a price and no
// more. The trigger price that makes a stop-loss a stop-loss, the time-in-force
// that says an order must not rest, post-only, reduce-only, self-trade
// prevention and the client order id that makes a retry idempotent all existed
// in the Rust core and had no spelling here.
//
// This holds the builders to that surface, and holds the string-shaped ones to
// rejecting a value they do not know: a lenient parse that quietly fell back to
// GTC would reintroduce, at the binding edge, exactly the defect #195 fixed
// inside the core.
//
//   wasm-pack build bindings/wasm --target nodejs --out-dir pkg
//   node --test bindings/wasm/tests/

const test = require('node:test');
const assert = require('node:assert/strict');

const { OrderRequest } = require('../pkg/wickra_exchange_wasm.js');

const BUILDERS = [
  'withStopPrice',
  'withTimeInForce',
  'withClientOrderId',
  'reduceOnly',
  'postOnly',
  'withStp',
];

test('OrderRequest exposes every field builder', () => {
  for (const verb of BUILDERS) {
    assert.equal(
      typeof OrderRequest.prototype[verb],
      'function',
      `OrderRequest is missing ${verb}`,
    );
  }
  for (const factory of ['marketBuy', 'marketSell', 'limitBuy', 'limitSell']) {
    assert.equal(
      typeof OrderRequest[factory],
      'function',
      `OrderRequest is missing ${factory}`,
    );
  }
});

test('the builders chain and return a new request', () => {
  const base = OrderRequest.limitSell('BTC/USDT', 1.0, 19000.0);
  const built = base
    .withStopPrice(19500.0)
    .withTimeInForce('IOC')
    .withClientOrderId('retry-safe-1')
    .reduceOnly()
    .withStp('expire_maker');
  assert.notEqual(built, base);
});

test('an unknown time-in-force throws rather than defaulting to GTC', () => {
  const request = OrderRequest.limitBuy('BTC/USDT', 1.0, 19000.0);
  for (const bad of ['', 'gtd', 'immediate']) {
    assert.throws(() => request.withTimeInForce(bad));
  }
  for (const bad of ['', 'cancel_maker']) {
    assert.throws(() => request.withStp(bad));
  }
});

test('time-in-force and stp are case-insensitive', () => {
  const request = OrderRequest.limitBuy('BTC/USDT', 1.0, 19000.0);
  assert.ok(request.withTimeInForce('ioc'));
  assert.ok(request.withTimeInForce('IOC'));
  assert.ok(request.withStp('EXPIRE_MAKER'));
  assert.ok(request.withStp('expire_both'));
});
