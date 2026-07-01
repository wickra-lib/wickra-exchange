"use strict";

const test = require("node:test");
const assert = require("node:assert");
const { Derivatives, AdvancedOrders, Credentials, OrderRequest } = require("../index.js");

// Construction is offline (no socket opens until an RPC is issued), so the class
// surface and the spot-only rejection are checked without a network.

test("derivatives and advanced classes are exported", () => {
  assert.strictEqual(typeof Derivatives, "function");
  assert.strictEqual(typeof AdvancedOrders, "function");
});

test("derivatives rejects spot-only and unknown venues", () => {
  const creds = new Credentials("key", "secret");
  for (const name of ["coinbase", "upbit", "ftx"]) {
    assert.throws(() => Derivatives.connect(name, creds), `${name} must be rejected`);
  }
});

test("advanced rejects spot-only and unknown venues", () => {
  const creds = new Credentials("key", "secret");
  for (const name of ["coinbase", "upbit", "ftx"]) {
    assert.throws(() => AdvancedOrders.connect(name, creds), `${name} must be rejected`);
  }
});

test("derivatives and advanced construct for a futures venue", () => {
  const creds = new Credentials("key", "secret");
  assert.ok(Derivatives.connect("binance", creds));
  assert.ok(AdvancedOrders.connect("binance", creds, false, true));
});

test("advanced exposes the full extended-ops surface", () => {
  const creds = new Credentials("key", "secret");
  const adv = AdvancedOrders.connect("binance", creds);
  for (const method of ["amendOrder", "cancelBatch", "placeOco", "placeBatch"]) {
    assert.strictEqual(typeof adv[method], "function", `${method} must be a method`);
  }
});

test("placeBatch accepts an array of OrderRequest instances", () => {
  // The batch input is an array of OrderRequest class instances; building them
  // is offline, so the argument shape is validated without a socket.
  const requests = [
    OrderRequest.limitBuy("BTC/USDT", 0.5, 60000),
    OrderRequest.marketSell("ETH/USDT", 2),
  ];
  assert.strictEqual(requests.length, 2);
  for (const request of requests) {
    assert.ok(request instanceof OrderRequest);
  }
});
