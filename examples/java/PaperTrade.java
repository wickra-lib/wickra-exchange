// Paper-trade differentiator demo.
//
// Run (with the binding classes + native library on the paths):
//   java --enable-native-access=ALL-UNNAMED -Dnative.lib.dir=<dir> \
//        -cp <wickra-exchange-classes> PaperTrade.java
import java.util.Map;
import org.wickra.exchange.Exchange;

public final class PaperTrade {
    public static void main(String[] args) {
        try (Exchange ex = Exchange.paper(Map.of("USDT", 100_000.0), 1, 5, 10)) {
            System.out.println("venue: " + ex.name());
            ex.setPrice("BTC/USDT", 20_000.0);

            Exchange.OrderInfo order = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0);
            System.out.println("filled at " + order.averagePrice() + " (status " + order.status() + ")");

            System.out.println("  BTC free: " + ex.balance("BTC"));
            System.out.println("  USDT free: " + ex.balance("USDT"));
        }
    }
}
