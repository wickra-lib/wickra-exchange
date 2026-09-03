"use strict";

// An order number arrives as the number that was written.
//
// JavaScript has one number type and it is a double: about fifteen significant
// digits. The core holds every order number in an exact decimal, so a string is
// the only spelling Node has that reaches it intact -- which is why every
// exchange's own API takes its order numbers as strings too.
//
// `describe()` is the exact read-back: every other number this binding reports
// is a JS number, so it is the only place the difference can be seen.

const test = require("node:test");
const assert = require("node:assert");
const { Exchange, OrderRequest } = require("../index.js");

// Wider than a double: the last digits are the ones it cannot hold.
const WIDE = "12345678.90123456789";
const TINY = "0.000000012345678901234567";

test("a wide price written as a string survives", () => {
  const request = OrderRequest.limitBuy("BTC/USDT", "1", WIDE);
  assert.ok(request.describe().includes(`price=${WIDE}`), request.describe());
});

test("the same number written as a JS number does not", () => {
  const request = OrderRequest.limitBuy("BTC/USDT", 1, Number(WIDE));
  const described = request.describe();
  assert.ok(!described.includes(`price=${WIDE}`), described);
  assert.ok(described.includes("price=12345678.90123457"), described);
});

test("an ordinary number still works and is still exact", () => {
  const described = OrderRequest.limitSell("BTC/USDT", 1.5, 19000.5).describe();
  assert.ok(described.includes("quantity=1.5"), described);
  assert.ok(described.includes("price=19000.5"), described);
});

test("a tiny exact quantity is not rounded away", () => {
  const described = OrderRequest.marketSell("BTC/USDT", TINY).describe();
  assert.ok(described.includes(`quantity=${TINY}`), described);
});

test("a stop price takes the same two spellings", () => {
  const exact = OrderRequest.marketSell("BTC/USDT", "1").withStopPrice(WIDE);
  assert.ok(exact.describe().includes(`stopPrice=${WIDE}`), exact.describe());
  const plain = OrderRequest.marketSell("BTC/USDT", 1).withStopPrice(19000);
  assert.ok(plain.describe().includes("stopPrice=19000"), plain.describe());
});

test("a string that is not a number is a refused order", () => {
  // Not an order at some other price: refused, and it says why.
  assert.throws(() => OrderRequest.limitBuy("BTC/USDT", 1, "nineteen thousand"));
  assert.throws(() => OrderRequest.marketBuy("BTC/USDT", ""));
});

test("an exact order places like any other", () => {
  const ex = Exchange.paper({ USDT: 100000, BTC: 10 });
  ex.setPrice("BTC/USDT", 20000);
  const order = ex.placeOrder(OrderRequest.limitSell("BTC/USDT", "1.5", "21000.5"));
  assert.strictEqual(order.status, "new");
  assert.ok(Math.abs(order.price - 21000.5) < 1e-9);
});
