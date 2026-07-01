"use strict";

const test = require("node:test");
const assert = require("node:assert");
const { Exchange, Credentials, OrderRequest, version } = require("../index.js");

test("module surface", () => {
  assert.strictEqual(typeof version(), "string");
  assert.strictEqual(typeof Exchange, "function");
  assert.strictEqual(typeof Credentials, "function");
  assert.strictEqual(typeof OrderRequest, "function");
});

test("credentials construct", () => {
  new Credentials("key", "secret");
  new Credentials("key", "secret", "passphrase");
  new Credentials("key", "secret", null, "-----BEGIN-----");
});

test("paper market buy fills with slippage and fee", () => {
  const ex = Exchange.paper({ USDT: 100000 }, 1, 5, 10);
  assert.strictEqual(ex.name(), "paper");
  ex.setPrice("BTC/USDT", 20000);

  const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
  assert.strictEqual(order.status, "filled");
  assert.strictEqual(order.side, "buy");
  // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
  assert.ok(Math.abs(order.averagePrice - 20020) < 1e-6);

  const balances = ex.balances();
  assert.ok(Math.abs(balances.BTC - 1) < 1e-9);
  // 20020 notional + 5 bps fee (10.01) spent from USDT.
  assert.ok(Math.abs(balances.USDT - (100000 - 20020 - 10.01)) < 1e-6);
});

test("events and ticker", () => {
  const ex = Exchange.paper({ USDT: 50000 });
  ex.setPrice("BTC/USDT", 20000);
  ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));

  const kinds = ex.pollEvents().map((e) => e.kind);
  assert.ok(kinds.includes("order_update"));
  assert.ok(kinds.includes("balance_update"));

  const ticker = ex.ticker("BTC/USDT");
  assert.ok(Math.abs(ticker.last - 20000) < 1e-9);
});

test("resting limit and cancel", () => {
  const ex = Exchange.paper({ USDT: 100000 });
  ex.setPrice("BTC/USDT", 20000);
  const order = ex.placeOrder(OrderRequest.limitBuy("BTC/USDT", 1, 19000));
  assert.strictEqual(order.status, "new");
  ex.cancelOrder("BTC/USDT", order.id);
  assert.ok(Math.abs(ex.balances().USDT - 100000) < 1e-9);
});

test("bad market string throws", () => {
  const ex = Exchange.paper({ USDT: 1 });
  assert.throws(() => ex.setPrice("BTCUSDT", 1));
});
