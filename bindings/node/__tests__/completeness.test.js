"use strict";

// Parity guard: assert every binding class exposes the full canonical verb set
// of the Rust core traits, so a method dropped in a refactor fails loudly here
// (mirrors the completeness check in the main wickra repo).

const test = require("node:test");
const assert = require("node:assert");
const {
  Exchange,
  Derivatives,
  AdvancedOrders,
  UserData,
  WsExecution,
  OrderRequest,
} = require("../index.js");

// MarketData (7) + Execution (5) + Exchange (1). placeOrder is the unified
// entry; setPrice is the paper-only helper.
const EXCHANGE_VERBS = [
  "ticker",
  "klines",
  "orderBook",
  "subscribeTrades",
  "subscribeBook",
  "subscribeTicker",
  "pollEvents",
  "placeOrder",
  "cancelOrder",
  "queryOrder",
  "openOrders",
  "balances",
  "name",
];

const DERIVATIVES_VERBS = ["positions", "setLeverage", "setMarginMode", "closePosition"];
const ADVANCED_VERBS = ["amendOrder", "placeBatch", "cancelBatch", "placeOco"];
const USER_DATA_VERBS = ["subscribeUserData", "keepaliveUserData", "poll"];
const WS_EXECUTION_VERBS = ["placeOrderWs", "cancelOrderWs"];

function assertVerbs(cls, verbs, label) {
  for (const verb of verbs) {
    assert.strictEqual(
      typeof cls.prototype[verb],
      "function",
      `${label} is missing method ${verb}`,
    );
  }
}

test("Exchange exposes the full MarketData + Execution surface", () => {
  assertVerbs(Exchange, EXCHANGE_VERBS, "Exchange");
});

test("Derivatives exposes the full surface", () => {
  assertVerbs(Derivatives, DERIVATIVES_VERBS, "Derivatives");
});

test("AdvancedOrders exposes the full surface", () => {
  assertVerbs(AdvancedOrders, ADVANCED_VERBS, "AdvancedOrders");
});

test("UserData exposes the full surface", () => {
  assertVerbs(UserData, USER_DATA_VERBS, "UserData");
});

test("WsExecution exposes the full surface", () => {
  assertVerbs(WsExecution, WS_EXECUTION_VERBS, "WsExecution");
});

// The four factories plus every builder that decides what the order *is*.
// Without these an order was a market/limit with a quantity and a price and
// nothing else, so a stop-loss, an IOC, a post-only or an idempotent retry could
// not be expressed from Node at all -- however much of it the Rust core
// supported.
const ORDER_REQUEST_VERBS = [
  "withStopPrice",
  "withTimeInForce",
  "withClientOrderId",
  "reduceOnly",
  "postOnly",
  "withStp",
];

test("OrderRequest exposes every field builder", () => {
  for (const verb of ORDER_REQUEST_VERBS) {
    assert.strictEqual(
      typeof OrderRequest.prototype[verb],
      "function",
      `OrderRequest is missing ${verb}`,
    );
  }
  for (const factory of ["marketBuy", "marketSell", "limitBuy", "limitSell"]) {
    assert.strictEqual(
      typeof OrderRequest[factory],
      "function",
      `OrderRequest is missing ${factory}`,
    );
  }
});

test("OrderRequest builders chain and return a new request", () => {
  const base = OrderRequest.limitSell("BTC/USDT", 1.0, 19000.0);
  const built = base
    .withStopPrice(19500.0)
    .withTimeInForce("IOC")
    .withClientOrderId("retry-safe-1")
    .reduceOnly();
  assert.notStrictEqual(built, base);
});

test("an unknown time-in-force throws rather than defaulting to GTC", () => {
  const request = OrderRequest.limitBuy("BTC/USDT", 1.0, 19000.0);
  for (const bad of ["", "gtd", "immediate"]) {
    assert.throws(() => request.withTimeInForce(bad), `withTimeInForce(${bad}) was accepted`);
  }
  for (const bad of ["", "cancel_maker"]) {
    assert.throws(() => request.withStp(bad), `withStp(${bad}) was accepted`);
  }
});

test("time-in-force and stp are case-insensitive", () => {
  const request = OrderRequest.limitBuy("BTC/USDT", 1.0, 19000.0);
  assert.ok(request.withTimeInForce("ioc"));
  assert.ok(request.withTimeInForce("IOC"));
  assert.ok(request.withStp("EXPIRE_MAKER"));
  assert.ok(request.withStp("expire_both"));
});
