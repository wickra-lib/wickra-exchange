package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.util.List;

/**
 * A live advanced-orders client: amend and batch cancel. Construct with
 * {@link #connect}; close it when done. Available on the eight trading venues.
 */
public final class AdvancedOrders implements AutoCloseable {

    private MemorySegment handle;

    private AdvancedOrders(MemorySegment handle) {
        this.handle = handle;
    }

    /** Connect an advanced-orders client; {@code futures} selects USD-M futures. */
    public static AdvancedOrders connect(String name, String apiKey, String apiSecret,
                                         String passphrase, String privateKey,
                                         boolean testnet, boolean futures) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pass = passphrase == null ? MemorySegment.NULL : arena.allocateFrom(passphrase);
            MemorySegment priv = privateKey == null ? MemorySegment.NULL : arena.allocateFrom(privateKey);
            MemorySegment h = (MemorySegment) Native.CONNECT_ADVANCED.invokeExact(
                    arena.allocateFrom(name), arena.allocateFrom(apiKey), arena.allocateFrom(apiSecret),
                    pass, priv, (byte) (testnet ? 1 : 0), (byte) (futures ? 1 : 0));
            if (h == null || h.equals(MemorySegment.NULL)) {
                throw new RuntimeException("failed to connect advanced-orders client for " + name);
            }
            return new AdvancedOrders(h);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Amend a resting order in place; pass {@code Double.NaN} to leave a field unchanged. */
    public Exchange.OrderInfo amendOrder(String market, String orderId, double newPrice, double newQuantity) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            Exchange.check((int) Native.ADVANCED_AMEND_ORDER.invokeExact(
                    handle, arena.allocateFrom(market), arena.allocateFrom(orderId),
                    newPrice, newQuantity, out));
            return Exchange.readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Cancel several orders on {@code market} in one request. */
    public void cancelBatch(String market, List<String> orderIds) {
        try (Arena arena = Arena.ofConfined()) {
            int n = orderIds.size();
            MemorySegment ids = arena.allocate(Native.C_PTR.byteSize() * Math.max(n, 1));
            for (int i = 0; i < n; i++) {
                ids.setAtIndex(Native.C_PTR, i, arena.allocateFrom(orderIds.get(i)));
            }
            Exchange.check((int) Native.ADVANCED_CANCEL_BATCH.invokeExact(
                    handle, arena.allocateFrom(market), ids, (long) n));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    @Override
    public void close() {
        if (handle != null && !handle.equals(MemorySegment.NULL)) {
            try {
                Native.ADVANCED_FREE.invokeExact(handle);
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
            handle = null;
        }
    }
}
