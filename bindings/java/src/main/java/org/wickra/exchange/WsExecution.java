package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;

/**
 * A live WebSocket order-API client: place and cancel orders over the venue's
 * WebSocket order API. Native on Binance/Bybit/OKX/Gate/Kraken; on Bitget,
 * KuCoin and HTX the methods throw (no WebSocket order-entry API — use REST).
 * Construct with {@link #connect}; close it when done.
 */
public final class WsExecution implements AutoCloseable {

    private MemorySegment handle;

    private WsExecution(MemorySegment handle) {
        this.handle = handle;
    }

    /** Connect a WebSocket order-API client; {@code futures} selects USD-M futures. */
    public static WsExecution connect(String name, String apiKey, String apiSecret,
                                      String passphrase, String privateKey,
                                      boolean testnet, boolean futures) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pass = passphrase == null ? MemorySegment.NULL : arena.allocateFrom(passphrase);
            MemorySegment priv = privateKey == null ? MemorySegment.NULL : arena.allocateFrom(privateKey);
            MemorySegment h = (MemorySegment) Native.CONNECT_WS_EXECUTION.invokeExact(
                    arena.allocateFrom(name), arena.allocateFrom(apiKey), arena.allocateFrom(apiSecret),
                    pass, priv, (byte) (testnet ? 1 : 0), (byte) (futures ? 1 : 0));
            if (h == null || h.equals(MemorySegment.NULL)) {
                throw new RuntimeException("failed to connect ws-execution client for " + name);
            }
            return new WsExecution(h);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Place an order over the WebSocket order API. {@code Double.NaN} price = market order. */
    public Exchange.OrderInfo placeOrderWs(String market, Exchange.Side side, double quantity, double price) {
        int sideCode = side == Exchange.Side.BUY ? Native.SIDE_BUY : Native.SIDE_SELL;
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            Exchange.check((int) Native.WS_PLACE_ORDER.invokeExact(
                    handle, arena.allocateFrom(market), sideCode, quantity, price, out));
            return Exchange.readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Cancel an order over the WebSocket order API by venue id. */
    public void cancelOrderWs(String market, String orderId) {
        try (Arena arena = Arena.ofConfined()) {
            Exchange.check((int) Native.WS_CANCEL_ORDER.invokeExact(
                    handle, arena.allocateFrom(market), arena.allocateFrom(orderId)));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    @Override
    public void close() {
        if (handle != null && !handle.equals(MemorySegment.NULL)) {
            try {
                Native.WS_EXECUTION_FREE.invokeExact(handle);
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
            handle = null;
        }
    }
}
