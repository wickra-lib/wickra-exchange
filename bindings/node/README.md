# wickra-exchange (Node.js)

Node.js bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange):
streaming-native, unified connectivity for the ten largest crypto exchanges, with
offline paper and replay simulators that share the exact same API.

```js
const { Exchange, Credentials, OrderRequest } = require("wickra-exchange");

// Offline paper account — deterministic, network-free.
const ex = Exchange.paper({ USDT: 100000 }, 1, 5); // maker/taker bps
ex.setPrice("BTC/USDT", 20000);
const order = ex.placeOrder(OrderRequest.marketBuy("BTC/USDT", 1));
console.log(order.status); // "filled"
console.log(ex.balances());

// Replay a recorded tape through the same API:
const rex = Exchange.replayTrades("BTC/USDT", [100, 101, 110], { USDT: 10000 });
let events;
while ((events = rex.pollEvents()).length) {
  for (const event of events) {
    // drive your strategy
  }
}

// Live venue (needs API keys):
//   const creds = new Credentials("key", "secret");
//   const live = Exchange.connect("binance", creds);
```

The same strategy runs **paper, replay and live** by swapping the constructor.

## Build

```bash
npm install
npm run build   # regenerates index.js, index.d.ts and the native .node
npm test
```

Licensed under `MIT OR Apache-2.0`.
