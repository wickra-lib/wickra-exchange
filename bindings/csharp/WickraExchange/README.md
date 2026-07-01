# WickraExchange (.NET)

.NET bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI: one synchronous, pull-based API over the ten largest crypto
exchanges, plus offline paper and replay simulators that share the same API.

```csharp
using WickraExchange;

using var ex = Exchange.Paper(
    new Dictionary<string, double> { ["USDT"] = 100_000.0 }, takerBps: 5.0);
ex.SetPrice("BTC/USDT", 20_000.0);
var order = ex.PlaceMarket("BTC/USDT", Side.Buy, 1.0);
Console.WriteLine(order.Status);           // Filled
Console.WriteLine(ex.Balance("BTC"));      // 1

// Live venue (needs API keys):
//   using var live = Exchange.Connect("binance", new WickraExchange.Credentials("k","s"));
```

The same strategy runs **paper, replay and live** by swapping the constructor.
Licensed under `MIT OR Apache-2.0`.
