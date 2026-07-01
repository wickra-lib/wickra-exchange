package org.wickra.exchange;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

class ExchangeTest {

    @Test
    void versionIsExposed() {
        assertFalse(Exchange.version().isEmpty());
    }

    @Test
    void paperMarketBuyFills() {
        try (Exchange ex = Exchange.paper(Map.of("USDT", 100_000.0), 1.0, 5.0, 10.0)) {
            assertEquals("paper", ex.name());
            ex.setPrice("BTC/USDT", 20_000.0);

            Exchange.OrderInfo order = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0);
            assertTrue(order.isFilled());
            // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
            assertTrue(Math.abs(order.averagePrice() - 20_020.0) < 1e-6);

            assertTrue(Math.abs(ex.balance("BTC") - 1.0) < 1e-9);
            assertTrue(Math.abs(ex.balance("USDT") - (100_000.0 - 20_020.0 - 10.01)) < 1e-6);
        }
    }

    @Test
    void restingLimitAndCancel() {
        try (Exchange ex = Exchange.paper(Map.of("USDT", 100_000.0), 0.0, 0.0, 0.0)) {
            ex.setPrice("BTC/USDT", 20_000.0);
            Exchange.OrderInfo resting = ex.placeLimit("BTC/USDT", Exchange.Side.BUY, 1.0, 19_000.0);
            assertEquals(Exchange.Status.NEW, resting.status());
            List<Exchange.Event> events = ex.poll(16);
            assertTrue(events.stream().anyMatch(e -> e.kind() == Exchange.Kind.ORDER_UPDATE));
            ex.cancel("BTC/USDT", resting.id());
            assertTrue(Math.abs(ex.balance("USDT") - 100_000.0) < 1e-9);
        }
    }

    @Test
    void replayParity() {
        double[] tape = {100.0, 101.0, 102.0, 110.0, 112.0};
        try (Exchange ex = Exchange.replayTrades("BTC/USDT", tape, Map.of("USDT", 100_000.0), 0.0, 0.0, 0.0)) {
            assertEquals("replay", ex.name());

            double[] window = new double[3];
            int seen = 0;
            boolean bought = false;

            while (true) {
                List<Exchange.Event> events = ex.poll(16);
                if (events.isEmpty()) {
                    break;
                }
                for (Exchange.Event ev : events) {
                    if (!ev.isTrade()) {
                        continue;
                    }
                    double price = ev.price();
                    window[seen % 3] = price;
                    seen++;
                    if (seen >= 3) {
                        double mean = (window[0] + window[1] + window[2]) / 3.0;
                        if (!bought && price > mean) {
                            Exchange.OrderInfo order = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0);
                            assertTrue(order.isFilled());
                            bought = true;
                        }
                    }
                }
            }

            assertTrue(bought);
            assertTrue(Math.abs(ex.balance("BTC") - 1.0) < 1e-9);
        }
    }
}
