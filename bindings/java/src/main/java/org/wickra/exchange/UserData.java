package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.util.ArrayList;
import java.util.List;

/**
 * A live private user-data client. After {@link #subscribeUserData}, {@link #poll}
 * surfaces the account's own order and balance updates alongside the public
 * market-data stream. Construct with {@link #connect}; close it when done.
 * Available on the eight trading venues.
 */
public final class UserData implements AutoCloseable {

    private MemorySegment handle;

    private UserData(MemorySegment handle) {
        this.handle = handle;
    }

    /** Connect a user-data client; {@code futures} selects USD-M futures. */
    public static UserData connect(String name, String apiKey, String apiSecret,
                                   String passphrase, String privateKey,
                                   boolean testnet, boolean futures) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pass = passphrase == null ? MemorySegment.NULL : arena.allocateFrom(passphrase);
            MemorySegment priv = privateKey == null ? MemorySegment.NULL : arena.allocateFrom(privateKey);
            MemorySegment h = (MemorySegment) Native.CONNECT_USER_DATA.invokeExact(
                    arena.allocateFrom(name), arena.allocateFrom(apiKey), arena.allocateFrom(apiSecret),
                    pass, priv, (byte) (testnet ? 1 : 0), (byte) (futures ? 1 : 0));
            if (h == null || h.equals(MemorySegment.NULL)) {
                throw new RuntimeException("failed to connect user-data client for " + name);
            }
            return new UserData(h);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Open the private user-data stream. Afterwards {@link #poll} drains the account's events too. */
    public void subscribeUserData() {
        try {
            Exchange.check((int) Native.USER_DATA_SUBSCRIBE.invokeExact(handle));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /**
     * Keep the private stream alive (refresh the venue session / send a heartbeat) so it is not
     * dropped for inactivity; call it periodically. A dropped stream is also recovered
     * automatically on the next {@link #poll}. A no-op before {@link #subscribeUserData}.
     */
    public void keepaliveUserData() {
        try {
            Exchange.check((int) Native.USER_DATA_KEEPALIVE.invokeExact(handle));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Drain buffered events (up to {@code capacity} per call). */
    public List<Exchange.Event> poll(int capacity) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment buf = arena.allocate(Native.EVENT_SIZE * capacity, 8);
            int count = (int) Native.USER_DATA_POLL.invokeExact(handle, buf, (long) capacity);
            if (count < 0) {
                throw new RuntimeException("user-data poll failed with code " + count);
            }
            List<Exchange.Event> events = new ArrayList<>(count);
            for (int i = 0; i < count; i++) {
                events.add(Exchange.readEvent(buf.asSlice(i * Native.EVENT_SIZE, Native.EVENT_SIZE)));
            }
            return events;
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    @Override
    public void close() {
        if (handle != null && !handle.equals(MemorySegment.NULL)) {
            try {
                Native.USER_DATA_FREE.invokeExact(handle);
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
            handle = null;
        }
    }
}
