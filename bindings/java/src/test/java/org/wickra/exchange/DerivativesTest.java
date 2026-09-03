package org.wickra.exchange;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import org.junit.jupiter.api.Test;

// Construction is offline (no socket opens until an RPC is issued), so the
// surface and the spot-only rejection are checked without a network.
class DerivativesTest {

    @Test
    void derivativesRejectsSpotOnlyAndUnknown() {
        for (String name : new String[] {"coinbase", "upbit", "ftx"}) {
            assertThrows(RuntimeException.class,
                    () -> Derivatives.connect(name, "k", "s", null, null, false));
        }
    }

    @Test
    void advancedRejectsSpotOnlyAndUnknown() {
        for (String name : new String[] {"coinbase", "upbit", "ftx"}) {
            assertThrows(RuntimeException.class,
                    () -> AdvancedOrders.connect(name, "k", "s", null, null, false, false));
        }
    }

    @Test
    void derivativesAndAdvancedConstructForFuturesVenue() {
        try (Derivatives d = Derivatives.connect("binance", "k", "s", null, null, false);
                AdvancedOrders a = AdvancedOrders.connect("binance", "k", "s", null, null, false, true)) {
            assertNotNull(d);
            assertNotNull(a);
        }
    }

    @Test
    void placeBatchEmptyIsNoop() {
        // An empty batch returns without opening a socket.
        try (AdvancedOrders a = AdvancedOrders.connect("binance", "k", "s", null, null, false, false)) {
            List<AdvancedOrders.BatchResult> results = a.placeBatch(List.of());
            assertTrue(results.isEmpty());
        }
    }

    @Test
    void placeBatchFullEmptyIsNoop() {
        // The full-request batch, which is the one that can carry a stop-loss.
        // Empty returns without opening a socket, like its narrow sibling.
        try (AdvancedOrders a = AdvancedOrders.connect("binance", "k", "s", null, null, false, false)) {
            List<AdvancedOrders.BatchResult> results = a.placeBatchFull(List.of());
            assertTrue(results.isEmpty());
        }
    }

    @Test
    void aBatchedOrderCanCarryEveryField() {
        // The shape BatchOrderRequest has no room for. Nothing is sent here --
        // what this pins is that the binding can express it at all, which it
        // could not: a batched order from Java was a market or a limit and
        // nothing else, whatever the venue clients supported.
        OrderRequest request = OrderRequest.stopLimit("BTC/USDT", Exchange.Side.BUY, 1.0, 100.0, 95.0)
                .withTimeInForce(TimeInForce.IOC)
                .withClientOrderId("batch-1")
                .withReduceOnly()
                .withPostOnly();
        assertEquals(95.0, request.stopPrice());
        assertEquals(TimeInForce.IOC, request.timeInForce());
        assertEquals("batch-1", request.clientOrderId());
        assertTrue(request.reduceOnly() && request.postOnly());
    }

    @Test
    void userDataAndWsExecutionRejectSpotOnly() {
        for (String name : new String[] {"coinbase", "upbit", "ftx"}) {
            assertThrows(RuntimeException.class,
                    () -> UserData.connect(name, "k", "s", null, null, false, false));
            assertThrows(RuntimeException.class,
                    () -> WsExecution.connect(name, "k", "s", null, null, false, false));
        }
    }

    @Test
    void userDataConstructsAndPolls() {
        try (UserData userData = UserData.connect("binance", "k", "s", null, null, false, false)) {
            // Keepalive is a no-op before subscribeUserData; it must not throw.
            userData.keepaliveUserData();
            // WsUserData: MarketData, so the client can poll (nothing buffered offline).
            assertTrue(userData.poll(4).isEmpty());
        }
    }

    @Test
    void wsExecutionConstructsForATradingVenue() {
        try (WsExecution exec = WsExecution.connect("bybit", "k", "s", null, null, false, false)) {
            assertNotNull(exec);
        }
    }

    @Test
    void batchRequestShapeRoundTrips() {
        var requests = List.of(
                new AdvancedOrders.BatchOrderRequest("BTC/USDT", Exchange.Side.BUY, 0.5, 60000),
                new AdvancedOrders.BatchOrderRequest("ETH/USDT", Exchange.Side.SELL, 2, Double.NaN));
        assertEquals(2, requests.size());
        assertEquals(Exchange.Side.BUY, requests.get(0).side());
        assertTrue(Double.isNaN(requests.get(1).price()));
        assertNull(new AdvancedOrders.BatchResult(null, "x").order());
    }
}
