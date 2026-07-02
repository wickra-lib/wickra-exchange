# wickra-exchange (Java)

JVM bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI via the Java FFM (Panama) API — no JNI. One synchronous,
pull-based API over the ten largest crypto exchanges, plus offline paper and
replay simulators that share the same API.

```java
import org.wickra.exchange.Exchange;
import java.util.Map;

try (Exchange ex = Exchange.paper(Map.of("USDT", 100_000.0), 0, 5, 0)) {
    ex.setPrice("BTC/USDT", 20_000.0);
    var order = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0);
    System.out.println(order.status());        // FILLED
    System.out.println(ex.balance("BTC"));      // 1.0
}
```

Requires Java 22+ (FFM). The native library path is set via the `native.lib.dir`
system property. The same strategy runs **paper, replay and live** by swapping the
constructor. Licensed under `MIT OR Apache-2.0`.
