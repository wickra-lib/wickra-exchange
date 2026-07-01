"use strict";

const test = require("node:test");
const assert = require("node:assert");
const { Derivatives, AdvancedOrders, Credentials } = require("../index.js");

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
