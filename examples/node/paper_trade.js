// Paper-trade differentiator demo. Run: node paper_trade.js
"use strict";

const { Exchange, OrderRequest } = require("wickra-exchange");

const ex = Exchange.paper({ USDT: 100000 }, 1, 5, 10);
console.log("venue:", ex.name());
ex.setPrice("BTC/USDT", 20000);

const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
console.log(`filled at ${order.averagePrice} (status ${order.status})`);

for (const [asset, free] of Object.entries(ex.balances())) {
  console.log(`  ${asset} free: ${free}`);
}

for (const event of ex.pollEvents()) {
  console.log("event:", event.kind);
}
