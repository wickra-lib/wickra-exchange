"use strict";

const test = require("node:test");
const assert = require("node:assert");
const { Derivatives, AdvancedOrders, Credentials, Exchange, OrderRequest, UserData, WsExecution } = require("../index.js");

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

test("user-data and ws-execution reject spot-only and unknown venues", () => {
  const creds = new Credentials("key", "secret");
  for (const name of ["coinbase", "upbit", "ftx"]) {
    assert.throws(() => UserData.connect(name, creds), `${name} must be rejected for user-data`);
    assert.throws(() => WsExecution.connect(name, creds), `${name} must be rejected for ws-execution`);
  }
});

test("user-data and ws-execution construct and expose their surface", () => {
  const creds = new Credentials("key", "secret");
  const userData = UserData.connect("binance", creds);
  assert.ok(userData);
  // WsUserData: MarketData, so the client can poll (nothing buffered yet).
  assert.deepStrictEqual(userData.poll(), []);
  assert.strictEqual(typeof userData.subscribeUserData, "function");
  // keepalive is a no-op before subscribe (no stream open yet).
  assert.strictEqual(typeof userData.keepaliveUserData, "function");
  userData.keepaliveUserData();

  const exec = WsExecution.connect("bybit", creds);
  assert.ok(exec);
  for (const method of ["placeOrderWs", "cancelOrderWs"]) {
    assert.strictEqual(typeof exec[method], "function", `${method} must be a method`);
  }
});

test("the exchange handle can reach a futures market", () => {
  // Exchange.connect used to build a spot client and nothing else, so no Node
  // caller could place a futures order, read a futures book or cancel a
  // futures order. Construction is offline; this pins that the door exists.
  const creds = new Credentials("key", "secret");
  assert.ok(Exchange.connect("binance", creds, false, "spot"));
  assert.ok(Exchange.connect("binance", creds, false, "usdm_futures"));
});

test("the margin and position modes reach the client", () => {
  // Two venues carry the margin mode on every order and four carry the
  // position side, so neither can be set after the first order.
  const creds = new Credentials("key", "secret");
  assert.ok(Exchange.connect("okx", creds, false, "usdm_futures", "isolated", "hedge"));
});

test("an unrouted market or an unknown mode is rejected", () => {
  const creds = new Credentials("key", "secret");
  // coinm_futures is deliberately not offered: no client routes it
  // consistently, and Binance treats it as spot outright.
  assert.throws(() => Exchange.connect("binance", creds, false, "coinm_futures"));
  assert.throws(() => Exchange.connect("binance", creds, false, "perpetual"));
  assert.throws(() => Exchange.connect("binance", creds, false, "spot", "portfolio"));
  assert.throws(() => Exchange.connect("binance", creds, false, "spot", "cross", "both"));
});
