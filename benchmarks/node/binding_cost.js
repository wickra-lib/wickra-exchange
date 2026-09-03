"use strict";

// What it costs to reach the library from Node.
//
// Same two operations, same offline paper account, same iteration count as
// every other program in this directory and as the Rust baseline. The
// difference from the baseline is this binding's overhead.

const { Exchange, OrderRequest } = require("../../bindings/node/index.js");

const ITERATIONS = 20000;
const WARMUP = 1000;

function report(operation, nanos) {
  const perCall = nanos / ITERATIONS;
  const opsPerSecond = 1e9 / perCall;
  console.log(
    `${operation.padEnd(12)} ${perCall.toFixed(0).padStart(10)} ns/op   ` +
      `${opsPerSecond.toFixed(0).padStart(12)} ops/s`
  );
}

function time(iterations, work) {
  const started = process.hrtime.bigint();
  for (let i = 0; i < iterations; i++) {
    work();
  }
  return Number(process.hrtime.bigint() - started);
}

const ex = Exchange.paper({ USDT: 1e9 });
ex.setPrice("BTC/USDT", 20000);

// The first call through any boundary pays for one-time setup, which is not
// what is being measured.
time(WARMUP, () => ex.ticker("BTC/USDT"));
report("ticker", time(ITERATIONS, () => ex.ticker("BTC/USDT")));

const request = OrderRequest.marketBuy("BTC/USDT", 0.0001);
time(WARMUP, () => ex.placeOrder(request));
report("place_order", time(ITERATIONS, () => ex.placeOrder(request)));
