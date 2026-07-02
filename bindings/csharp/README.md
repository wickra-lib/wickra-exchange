# wickra-exchange (C#)

.NET bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI via P/Invoke. One synchronous, pull-based API over the ten
largest crypto exchanges, plus offline paper and replay simulators that share the
same API.

```csharp
using WickraExchange;
using System.Collections.Generic;

using var ex = Exchange.Paper(new Dictionary<string, double> { ["USDT"] = 100_000.0 },
                              makerBps: 0, takerBps: 5, slippageBps: 0);
ex.SetPrice("BTC/USDT", 20_000.0);
var order = ex.PlaceMarket("BTC/USDT", Side.Buy, 1.0);
Console.WriteLine(order.Status);        // Filled
Console.WriteLine(ex.Balance("BTC"));    // 1.0
```

Requires .NET 8+. The native library (`wickra_exchange`) must be resolvable on the
loader path (`PATH` on Windows, `LD_LIBRARY_PATH` on Linux, `DYLD_LIBRARY_PATH` on
macOS). The same strategy runs **paper, replay and live** by swapping the
constructor. Licensed under `MIT OR Apache-2.0`.
