package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/**
 * A live derivatives (futures/perpetual) client: positions, leverage, margin
 * mode and reduce-only close. Construct with {@link #connect}; close it when
 * done. Available on the eight venues with futures markets.
 */
public final class Derivatives implements AutoCloseable {

    /** The margin mode of a position. */
    public enum MarginMode { CROSS, ISOLATED }

    /** The direction of a position. */
    public enum PositionSide { LONG, SHORT }

    /** An open derivatives position. */
    public record PositionInfo(String symbol, PositionSide side, double quantity,
                               double entryPrice, double markPrice, double leverage,
                               double unrealizedPnl, MarginMode marginMode) {}

    private MemorySegment handle;

    private Derivatives(MemorySegment handle) {
        this.handle = handle;
    }

    /** Connect a USD-M futures client for {@code name}. Fails for a spot-only venue. */
    public static Derivatives connect(String name, String apiKey, String apiSecret,
                                      String passphrase, String privateKey, boolean testnet) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pass = passphrase == null ? MemorySegment.NULL : arena.allocateFrom(passphrase);
            MemorySegment priv = privateKey == null ? MemorySegment.NULL : arena.allocateFrom(privateKey);
            MemorySegment h = (MemorySegment) Native.CONNECT_DERIVATIVES.invokeExact(
                    arena.allocateFrom(name), arena.allocateFrom(apiKey), arena.allocateFrom(apiSecret),
                    pass, priv, (byte) (testnet ? 1 : 0));
            if (h == null || h.equals(MemorySegment.NULL)) {
                throw new RuntimeException("failed to connect derivatives client for " + name);
            }
            return new Derivatives(h);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** The open position in {@code market} (throws if flat). */
    public PositionInfo position(String market) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.POSITION_SIZE, 8);
            Exchange.check((int) Native.DERIVATIVES_POSITION.invokeExact(
                    handle, arena.allocateFrom(market), out));
            return readPosition(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /**
     * Every open position. Pass a {@code market} to scope to one symbol, or
     * {@code null} for all. Grows its buffer and re-queries if the venue reports
     * more positions than fit.
     */
    public java.util.List<PositionInfo> positions(String market) {
        int cap = 16;
        while (true) {
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment marketSeg = market == null ? MemorySegment.NULL : arena.allocateFrom(market);
                MemorySegment out = arena.allocate(Native.POSITION_SIZE * cap, 8);
                int count = (int) Native.DERIVATIVES_POSITIONS.invokeExact(handle, marketSeg, out, (long) cap);
                if (count < 0) {
                    throw new RuntimeException("positions failed with code " + count);
                }
                if (count > cap) {
                    cap = count;
                    continue;
                }
                java.util.List<PositionInfo> result = new java.util.ArrayList<>(count);
                for (int i = 0; i < count; i++) {
                    MemorySegment slot = out.asSlice((long) i * Native.POSITION_SIZE, Native.POSITION_SIZE);
                    result.add(readPosition(slot));
                }
                return result;
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
        }
    }

    /** Set the leverage for {@code market}. */
    public void setLeverage(String market, int leverage) {
        try (Arena arena = Arena.ofConfined()) {
            Exchange.check((int) Native.DERIVATIVES_SET_LEVERAGE.invokeExact(
                    handle, arena.allocateFrom(market), leverage));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Set the margin mode for {@code market}. */
    public void setMarginMode(String market, MarginMode mode) {
        int code = mode == MarginMode.ISOLATED ? Native.MARGIN_ISOLATED : Native.MARGIN_CROSS;
        try (Arena arena = Arena.ofConfined()) {
            Exchange.check((int) Native.DERIVATIVES_SET_MARGIN_MODE.invokeExact(
                    handle, arena.allocateFrom(market), code));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Flatten the open position in {@code market} with a reduce-only market order. */
    public Exchange.OrderInfo closePosition(String market) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            Exchange.check((int) Native.DERIVATIVES_CLOSE_POSITION.invokeExact(
                    handle, arena.allocateFrom(market), out));
            return Exchange.readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    @Override
    public void close() {
        if (handle != null && !handle.equals(MemorySegment.NULL)) {
            try {
                Native.DERIVATIVES_FREE.invokeExact(handle);
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
            handle = null;
        }
    }

    private static PositionInfo readPosition(MemorySegment p) {
        String symbol = Native.readCString(p, Native.P_SYMBOL, Native.STR_CAP);
        PositionSide side = p.get(ValueLayout.JAVA_INT, Native.P_SIDE) == Native.POSITION_SHORT
                ? PositionSide.SHORT : PositionSide.LONG;
        double quantity = p.get(ValueLayout.JAVA_DOUBLE, Native.P_QUANTITY);
        double entry = p.get(ValueLayout.JAVA_DOUBLE, Native.P_ENTRY);
        double mark = p.get(ValueLayout.JAVA_DOUBLE, Native.P_MARK);
        double leverage = p.get(ValueLayout.JAVA_DOUBLE, Native.P_LEVERAGE);
        double upnl = p.get(ValueLayout.JAVA_DOUBLE, Native.P_UPNL);
        MarginMode mode = p.get(ValueLayout.JAVA_INT, Native.P_MARGIN_MODE) == Native.MARGIN_ISOLATED
                ? MarginMode.ISOLATED : MarginMode.CROSS;
        return new PositionInfo(symbol, side, quantity, entry, mark, leverage, upnl, mode);
    }
}
