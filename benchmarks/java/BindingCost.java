import org.wickra.exchange.Exchange;
import org.wickra.exchange.OrderRequest;
import java.util.Map;

/**
 * What it costs to reach the library from Java.
 *
 * <p>Same two operations, same offline paper account, same iteration count as
 * every other program in this directory and as the Rust baseline. The
 * difference from the baseline is this binding's overhead.
 */
public final class BindingCost {
    private static final int ITERATIONS = 20_000;
    private static final int WARMUP = 1_000;

    private BindingCost() {
    }

    private static void report(String operation, long nanos) {
        double perCall = (double) nanos / ITERATIONS;
        System.out.printf("%-12s %10.0f ns/op   %12.0f ops/s%n", operation, perCall, 1e9 / perCall);
    }

    private static long measure(int iterations, Runnable work) {
        long started = System.nanoTime();
        for (int i = 0; i < iterations; i++) {
            work.run();
        }
        return System.nanoTime() - started;
    }

    public static void main(String[] args) {
        try (Exchange ex = Exchange.paper(Map.of("USDT", 1e9), 0, 0, 0)) {
            ex.setPrice("BTC/USDT", 20_000.0);

            // The first call through any boundary pays for one-time setup, and
            // the JVM also needs the loop warm before it means anything.
            measure(WARMUP, () -> ex.ticker("BTC/USDT"));
            report("ticker", measure(ITERATIONS, () -> ex.ticker("BTC/USDT")));

            OrderRequest request = OrderRequest.market("BTC/USDT", Exchange.Side.BUY, 0.0001);
            measure(WARMUP, () -> ex.placeOrder(request));
            report("place_order", measure(ITERATIONS, () -> ex.placeOrder(request)));
        }
    }
}
