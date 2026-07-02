"use strict";

const test = require("node:test");
const assert = require("node:assert");
const { Exchange, OrderRequest } = require("../index.js");

function seeded() {
  const ex = Exchange.paper({ USDT: 100000 });
  ex.setPrice("BTC/USDT", 20000);
  return ex;
}

test("subscribe streams are accepted", () => {
  const ex = seeded();
  ex.subscribeTrades("BTC/USDT");
  ex.subscribeBook("BTC/USDT");
  ex.subscribeTicker("BTC/USDT");
});

test("klines and orderBook report unsupported on paper", () => {
  const ex = seeded();
  assert.throws(() => ex.klines("BTC/USDT", "1m", 10));
  assert.throws(() => ex.orderBook("BTC/USDT", 10));
});

test("queryOrder and openOrders", () => {
  const ex = seeded();
  const resting = ex.placeOrder(OrderRequest.limitBuy("BTC/USDT", 1, 19000));
  assert.strictEqual(resting.status, "new");

  const queried = ex.queryOrder("BTC/USDT", resting.id);
  assert.strictEqual(queried.id, resting.id);

  const opens = ex.openOrders();
  assert.strictEqual(opens.length, 1);
  assert.strictEqual(opens[0].id, resting.id);

  assert.strictEqual(ex.openOrders("BTC/USDT").length, 1);
  assert.strictEqual(ex.openOrders("ETH/USDT").length, 0);
});
