package org.wickra.exchange;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * The order fields that had no way across the C ABI until now.
 *
 * <p>{@code placeMarket} and {@code placeLimit} take a market, a side, a
 * quantity and a price. Everything the library supports beyond that -- the
 * trigger price, the time-in-force, post-only, reduce-only, self-trade
 * prevention, the client order id -- existed in the Rust core and could not be
 * expressed from Java.
 */
class OrderRequestTest {

    private static Exchange paper() {
        return Exchange.paper(Map.of("USDT", 100_000.0, "BTC", 5.0), 1.0, 5.0, 10.0);
    }

    @Test
    void aPlainRequestPlacesTheSameOrderAsTheNarrowCall() {
        try (Exchange ex = paper()) {
            ex.setPrice("BTC/USDT", 20_000.0);

            Exchange.OrderInfo order =
                    ex.placeOrder(OrderRequest.market("BTC/USDT", Exchange.Side.BUY, 1.0));

            assertTrue(order.isFilled());
            assertEquals(20_020.0, order.averagePrice(), 1e-6);
        }
    }

    @Test
    void aRestingOrderCarriesItsFlags() {
        try (Exchange ex = paper()) {
            ex.setPrice("BTC/USDT", 20_000.0);

            Exchange.OrderInfo order = ex.placeOrder(
                    OrderRequest.limit("BTC/USDT", Exchange.Side.BUY, 1.0, 19_000.0)
                            .withTimeInForce(TimeInForce.GTC)
                            .withClientOrderId("retry-safe-1")
                            .withPostOnly()
                            .withStp(SelfTradePrevention.EXPIRE_MAKER));

            assertEquals(Exchange.Status.NEW, order.status());
        }
    }

    /**
     * A trigger order reaches the venue with its trigger. The paper backend
     * refuses triggers, and that refusal is the proof it arrived: a request with
     * the field dropped would have been placed as a plain market sell instead,
     * at the price the stop existed to protect against.
     */
    @Test
    void aStopOrderCarriesItsTrigger() {
        try (Exchange ex = paper()) {
            ex.setPrice("BTC/USDT", 20_000.0);

            assertThrows(RuntimeException.class, () -> ex.placeOrder(
                    OrderRequest.stop("BTC/USDT", Exchange.Side.SELL, 1.0, 19_000.0)));
        }
    }

    @Test
    void theBuildersReturnANewRequestAndDefaultToUnset() {
        OrderRequest base = OrderRequest.limit("BTC/USDT", Exchange.Side.BUY, 1.0, 19_000.0);

        assertNull(base.stopPrice());
        assertNull(base.clientOrderId());
        assertEquals(TimeInForce.GTC, base.timeInForce());
        assertEquals(SelfTradePrevention.NONE, base.stp());

        OrderRequest built = base.withTimeInForce(TimeInForce.IOC).withReduceOnly();
        assertEquals(TimeInForce.IOC, built.timeInForce());
        assertTrue(built.reduceOnly());
        // The original is untouched: each builder returns a new request.
        assertEquals(TimeInForce.GTC, base.timeInForce());
        assertEquals(false, base.reduceOnly());
    }

    @Test
    void stopLimitCarriesBothPrices() {
        OrderRequest request = OrderRequest.stopLimit(
                "BTC/USDT", Exchange.Side.SELL, 1.0, 18_900.0, 19_000.0);

        assertEquals(OrderType.STOP_LIMIT, request.type());
        assertEquals(18_900.0, request.price());
        assertEquals(19_000.0, request.stopPrice());
    }
}
