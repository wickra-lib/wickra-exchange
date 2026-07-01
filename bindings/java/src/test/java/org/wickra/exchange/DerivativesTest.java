package org.wickra.exchange;

import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

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
}
