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
