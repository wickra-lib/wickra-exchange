package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.util.List;

/**
 * A live advanced-orders client: amend and batch cancel. Construct with
 * {@link #connect}; close it when done. Available on the eight trading venues.
 */
public final class AdvancedOrders implements AutoCloseable {

    /**
     * One order in a batch placement. A {@code NaN} {@code price} places a market
     * order; a finite value places a limit order.
     */
    public record BatchOrderRequest(String market, Exchange.Side side, double quantity, double price) {}

    /**
     * One order's outcome in a batch placement: exactly one of {@code order}
     * (success) or {@code error} (per-order rejection) is set.
     */
    public record BatchResult(Exchange.OrderInfo order, String error) {}

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

    /**
     * Place a one-cancels-other bracket: a take-profit limit leg at {@code price}
     * paired with a stop leg triggered at {@code stopPrice}. A finite
     * {@code stopLimitPrice} makes the stop leg a stop-limit; pass {@code Double.NaN}
     * to leave it a stop-market. Returns the resulting order legs.
     */
    public List<Exchange.OrderInfo> placeOco(String market, Exchange.Side side, double quantity,
                                             double price, double stopPrice, double stopLimitPrice) {
        int sideCode = side == Exchange.Side.BUY ? Native.SIDE_BUY : Native.SIDE_SELL;
        int cap = 4;
        while (true) {
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment out = arena.allocate(Native.ORDER_SIZE * cap, 8);
                int count = (int) Native.ADVANCED_PLACE_OCO.invokeExact(
                        handle, arena.allocateFrom(market), sideCode,
                        quantity, price, stopPrice, stopLimitPrice, out, (long) cap);
                if (count < 0) {
                    throw new RuntimeException("place_oco failed with code " + count);
                }
                if (count > cap) {
                    cap = count;
                    continue;
                }
                java.util.List<Exchange.OrderInfo> result = new java.util.ArrayList<>(count);
                for (int i = 0; i < count; i++) {
                    result.add(Exchange.readOrder(out.asSlice((long) i * Native.ORDER_SIZE, Native.ORDER_SIZE)));
                }
                return result;
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
        }
    }

    /**
     * Place several orders in one request. Returns one {@link BatchResult} per
     * request, in order: a per-order rejection surfaces in that result's
     * {@code error}, while a whole-request failure throws.
     */
    public List<BatchResult> placeBatch(List<BatchOrderRequest> requests) {
        int n = requests.size();
        if (n == 0) {
            return List.of();
        }
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment markets = arena.allocate(Native.C_PTR.byteSize() * n, 8);
            MemorySegment sides = arena.allocate((long) Native.C_INT.byteSize() * n, 8);
            MemorySegment quantities = arena.allocate((long) Native.C_DOUBLE.byteSize() * n, 8);
            MemorySegment prices = arena.allocate((long) Native.C_DOUBLE.byteSize() * n, 8);
            for (int i = 0; i < n; i++) {
                BatchOrderRequest r = requests.get(i);
                markets.setAtIndex(Native.C_PTR, i, arena.allocateFrom(r.market()));
                sides.setAtIndex(Native.C_INT, i, r.side() == Exchange.Side.BUY ? Native.SIDE_BUY : Native.SIDE_SELL);
                quantities.setAtIndex(Native.C_DOUBLE, i, r.quantity());
                prices.setAtIndex(Native.C_DOUBLE, i, r.price());
            }
            MemorySegment out = arena.allocate(Native.ORDER_SIZE * n, 8);
            MemorySegment codes = arena.allocate((long) Native.C_INT.byteSize() * n, 8);
            int count = (int) Native.ADVANCED_PLACE_BATCH.invokeExact(
                    handle, markets, sides, quantities, prices, (long) n, out, codes, (long) n);
            if (count < 0) {
                throw new RuntimeException("place_batch failed with code " + count);
            }
            java.util.List<BatchResult> result = new java.util.ArrayList<>(count);
            for (int i = 0; i < count; i++) {
                int code = codes.getAtIndex(Native.C_INT, i);
                if (code == Native.OK) {
                    result.add(new BatchResult(
                            Exchange.readOrder(out.asSlice((long) i * Native.ORDER_SIZE, Native.ORDER_SIZE)), null));
                } else {
                    result.add(new BatchResult(null, "order rejected with code " + code));
                }
            }
            return result;
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
