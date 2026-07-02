package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * A unified exchange client over the synchronous, pull-based API. Construct with
 * {@link #paper}, {@link #replayTrades} or {@link #connect}; the methods are
 * identical whichever backend was chosen. Not thread-safe; close it when done.
 */
public final class Exchange implements AutoCloseable {

    /** The side of an order. */
    public enum Side { BUY, SELL }

    /** The lifecycle state of an order. */
    public enum Status { NEW, PARTIALLY_FILLED, FILLED, CANCELED, REJECTED, EXPIRED }

    /** The kind of a stream event. */
    public enum Kind { TRADE, TICKER, ORDER_UPDATE, BALANCE_UPDATE, SUBSCRIBED, OTHER }

    /** An order as reported by the exchange. */
    public record OrderInfo(String id, Side side, Status status, double quantity,
                            double filledQuantity, Double price, Double averagePrice) {
        public boolean isFilled() {
            return status == Status.FILLED;
        }
    }

    /** A single stream event. */
    public record Event(Kind kind, String symbol, Double price, Double quantity,
                        Side side, OrderInfo order) {
        public boolean isTrade() {
            return kind == Kind.TRADE;
        }
    }

    /** A point-in-time ticker snapshot. */
    public record TickerInfo(String symbol, double last, double bid, double ask, double volume) {}

    /** A single OHLCV candle. */
    public record CandleInfo(double open, double high, double low, double close,
                             double volume, long timestamp) {}

    /** A single order-book level: price and resting quantity. */
    public record BookLevelInfo(double price, double quantity) {}

    /**
     * A depth snapshot, best-first on each side. {@code symbol} echoes the
     * requested market; the venue sequence id is available on the native bindings.
     */
    public record OrderBookInfo(String symbol, List<BookLevelInfo> bids, List<BookLevelInfo> asks) {}

    private MemorySegment handle;

    private Exchange(MemorySegment handle) {
        this.handle = handle;
    }

    /** The library version. */
    public static String version() {
        try {
            MemorySegment ptr = (MemorySegment) Native.VERSION.invokeExact();
            return ptr.reinterpret(Long.MAX_VALUE).getString(0);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** An offline paper account seeded from {@code balances} (asset -&gt; amount). */
    public static Exchange paper(Map<String, Double> balances,
                                 double makerBps, double takerBps, double slippageBps) {
        try (Arena arena = Arena.ofConfined()) {
            Balances b = marshalBalances(arena, balances);
            MemorySegment h = (MemorySegment) Native.PAPER_NEW.invokeExact(
                    b.assets, b.amounts, (long) balances.size(), makerBps, takerBps, slippageBps);
            return fromHandle(h, "paper");
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** A replay account driven by a recorded {@code tape} of trades for {@code market}. */
    public static Exchange replayTrades(String market, double[] tape, Map<String, Double> balances,
                                        double makerBps, double takerBps, double slippageBps) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment marketSeg = arena.allocateFrom(market);
            MemorySegment tapeSeg = arena.allocateFrom(ValueLayout.JAVA_DOUBLE, tape);
            Balances b = marshalBalances(arena, balances);
            MemorySegment h = (MemorySegment) Native.REPLAY_NEW.invokeExact(
                    marketSeg, tapeSeg, (long) tape.length,
                    b.assets, b.amounts, (long) balances.size(),
                    makerBps, takerBps, slippageBps);
            return fromHandle(h, "replay");
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** A live client for {@code name}, authenticated with API keys. */
    public static Exchange connect(String name, String apiKey, String apiSecret,
                                   String passphrase, String privateKey, boolean testnet) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pass = passphrase == null ? MemorySegment.NULL : arena.allocateFrom(passphrase);
            MemorySegment priv = privateKey == null ? MemorySegment.NULL : arena.allocateFrom(privateKey);
            MemorySegment h = (MemorySegment) Native.CONNECT.invokeExact(
                    arena.allocateFrom(name), arena.allocateFrom(apiKey), arena.allocateFrom(apiSecret),
                    pass, priv, (byte) (testnet ? 1 : 0));
            return fromHandle(h, name);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** The venue identifier ("paper", "replay", "binance", ...). */
    public String name() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment buf = arena.allocate(32);
            int rc = (int) Native.NAME.invokeExact(handle, buf, 32L);
            check(rc);
            return Native.readCString(buf, 0, 32);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Set the mark price a paper account fills against (paper backend only). */
    public void setPrice(String market, double price) {
        try (Arena arena = Arena.ofConfined()) {
            check((int) Native.SET_PRICE.invokeExact(handle, arena.allocateFrom(market), price));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Place a market order. */
    public OrderInfo placeMarket(String market, Side side, double quantity) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            int rc = (int) Native.PLACE_MARKET.invokeExact(
                    handle, arena.allocateFrom(market), sideCode(side), quantity, out);
            check(rc);
            return readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Place a limit order. */
    public OrderInfo placeLimit(String market, Side side, double quantity, double price) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            int rc = (int) Native.PLACE_LIMIT.invokeExact(
                    handle, arena.allocateFrom(market), sideCode(side), quantity, price, out);
            check(rc);
            return readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Cancel an open order by venue id. */
    public void cancel(String market, String orderId) {
        try (Arena arena = Arena.ofConfined()) {
            check((int) Native.CANCEL.invokeExact(
                    handle, arena.allocateFrom(market), arena.allocateFrom(orderId)));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** The free balance of {@code asset}. */
    public double balance(String asset) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(ValueLayout.JAVA_DOUBLE);
            check((int) Native.BALANCE.invokeExact(handle, arena.allocateFrom(asset), out));
            return out.get(ValueLayout.JAVA_DOUBLE, 0);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** The current ticker for {@code market}. */
    public TickerInfo ticker(String market) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.TICKER_SIZE, 8);
            check((int) Native.EXCHANGE_TICKER.invokeExact(handle, arena.allocateFrom(market), out));
            return readTicker(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Up to {@code limit} historical candles for {@code market} at {@code interval}. */
    public List<CandleInfo> klines(String market, String interval, int limit) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment marketSeg = arena.allocateFrom(market);
            MemorySegment intervalSeg = arena.allocateFrom(interval);
            int cap = 128;
            while (true) {
                MemorySegment buf = arena.allocate(Native.CANDLE_SIZE * cap, 8);
                int count = (int) Native.EXCHANGE_KLINES.invokeExact(
                        handle, marketSeg, intervalSeg, limit, buf, (long) cap);
                if (count < 0) {
                    throw new RuntimeException("klines failed with code " + count);
                }
                if (count > cap) {
                    cap = count;
                    continue;
                }
                List<CandleInfo> candles = new ArrayList<>(count);
                for (int i = 0; i < count; i++) {
                    candles.add(readCandle(buf.asSlice(i * Native.CANDLE_SIZE, Native.CANDLE_SIZE)));
                }
                return candles;
            }
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Depth snapshot for {@code market} (up to {@code depth} levels per side). */
    public OrderBookInfo orderBook(String market, int depth) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment marketSeg = arena.allocateFrom(market);
            int cap = 64;
            while (true) {
                MemorySegment bids = arena.allocate(Native.BOOK_LEVEL_SIZE * cap, 8);
                MemorySegment asks = arena.allocate(Native.BOOK_LEVEL_SIZE * cap, 8);
                MemorySegment bidCount = arena.allocate(ValueLayout.JAVA_LONG);
                MemorySegment askCount = arena.allocate(ValueLayout.JAVA_LONG);
                check((int) Native.EXCHANGE_ORDER_BOOK.invokeExact(
                        handle, marketSeg, depth, bids, (long) cap, asks, (long) cap, bidCount, askCount));
                int nb = (int) bidCount.get(ValueLayout.JAVA_LONG, 0);
                int na = (int) askCount.get(ValueLayout.JAVA_LONG, 0);
                if (nb > cap || na > cap) {
                    cap = Math.max(nb, na);
                    continue;
                }
                List<BookLevelInfo> bidList = new ArrayList<>(nb);
                for (int i = 0; i < nb; i++) {
                    bidList.add(readBookLevel(bids.asSlice(i * Native.BOOK_LEVEL_SIZE, Native.BOOK_LEVEL_SIZE)));
                }
                List<BookLevelInfo> askList = new ArrayList<>(na);
                for (int i = 0; i < na; i++) {
                    askList.add(readBookLevel(asks.asSlice(i * Native.BOOK_LEVEL_SIZE, Native.BOOK_LEVEL_SIZE)));
                }
                return new OrderBookInfo(market, bidList, askList);
            }
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Subscribe to the public trade stream for {@code market}. */
    public void subscribeTrades(String market) {
        try (Arena arena = Arena.ofConfined()) {
            check((int) Native.EXCHANGE_SUBSCRIBE_TRADES.invokeExact(handle, arena.allocateFrom(market)));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Subscribe to the order-book stream for {@code market}. */
    public void subscribeBook(String market) {
        try (Arena arena = Arena.ofConfined()) {
            check((int) Native.EXCHANGE_SUBSCRIBE_BOOK.invokeExact(handle, arena.allocateFrom(market)));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Subscribe to the ticker stream for {@code market}. */
    public void subscribeTicker(String market) {
        try (Arena arena = Arena.ofConfined()) {
            check((int) Native.EXCHANGE_SUBSCRIBE_TICKER.invokeExact(handle, arena.allocateFrom(market)));
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Look up a single order by venue id. */
    public OrderInfo queryOrder(String market, String orderId) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment out = arena.allocate(Native.ORDER_SIZE, 8);
            check((int) Native.EXCHANGE_QUERY_ORDER.invokeExact(
                    handle, arena.allocateFrom(market), arena.allocateFrom(orderId), out));
            return readOrder(out);
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Open orders, optionally filtered to one {@code market} (null for all). */
    public List<OrderInfo> openOrders(String market) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment marketSeg = market == null ? MemorySegment.NULL : arena.allocateFrom(market);
            int cap = 16;
            while (true) {
                MemorySegment buf = arena.allocate(Native.ORDER_SIZE * cap, 8);
                int count = (int) Native.EXCHANGE_OPEN_ORDERS.invokeExact(handle, marketSeg, buf, (long) cap);
                if (count < 0) {
                    throw new RuntimeException("open_orders failed with code " + count);
                }
                if (count > cap) {
                    cap = count;
                    continue;
                }
                List<OrderInfo> orders = new ArrayList<>(count);
                for (int i = 0; i < count; i++) {
                    orders.add(readOrder(buf.asSlice(i * Native.ORDER_SIZE, Native.ORDER_SIZE)));
                }
                return orders;
            }
        } catch (Throwable t) {
            throw new RuntimeException(t);
        }
    }

    /** Drain buffered events (up to {@code capacity} per call). */
    public List<Event> poll(int capacity) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment buf = arena.allocate(Native.EVENT_SIZE * capacity, 8);
            int count = (int) Native.POLL.invokeExact(handle, buf, (long) capacity);
            if (count < 0) {
                throw new RuntimeException("poll failed with code " + count);
            }
            List<Event> events = new ArrayList<>(count);
            for (int i = 0; i < count; i++) {
                events.add(readEvent(buf.asSlice(i * Native.EVENT_SIZE, Native.EVENT_SIZE)));
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
                Native.FREE.invokeExact(handle);
            } catch (Throwable t) {
                throw new RuntimeException(t);
            }
            handle = null;
        }
    }

    // ---------------------------- helpers ------------------------------------

    private record Balances(MemorySegment assets, MemorySegment amounts) {}

    private static Balances marshalBalances(Arena arena, Map<String, Double> balances) {
        int n = balances.size();
        MemorySegment assets = arena.allocate(Native.C_PTR.byteSize() * Math.max(n, 1));
        MemorySegment amounts = arena.allocate(ValueLayout.JAVA_DOUBLE, Math.max(n, 1));
        int i = 0;
        for (Map.Entry<String, Double> entry : balances.entrySet()) {
            assets.setAtIndex(Native.C_PTR, i, arena.allocateFrom(entry.getKey()));
            amounts.setAtIndex(ValueLayout.JAVA_DOUBLE, i, entry.getValue());
            i++;
        }
        return new Balances(assets, amounts);
    }

    private static Exchange fromHandle(MemorySegment handle, String what) {
        if (handle == null || handle.equals(MemorySegment.NULL)) {
            throw new RuntimeException("failed to construct " + what + " exchange");
        }
        return new Exchange(handle);
    }

    private static int sideCode(Side side) {
        return side == Side.BUY ? Native.SIDE_BUY : Native.SIDE_SELL;
    }

    static OrderInfo readOrder(MemorySegment order) {
        String id = Native.readCString(order, Native.O_ID, Native.STR_CAP);
        Side side = order.get(ValueLayout.JAVA_INT, Native.O_SIDE) == Native.SIDE_SELL ? Side.SELL : Side.BUY;
        Status status = Status.values()[order.get(ValueLayout.JAVA_INT, Native.O_STATUS)];
        double quantity = order.get(ValueLayout.JAVA_DOUBLE, Native.O_QUANTITY);
        double filled = order.get(ValueLayout.JAVA_DOUBLE, Native.O_FILLED);
        Double price = nanToNull(order.get(ValueLayout.JAVA_DOUBLE, Native.O_PRICE));
        Double avg = nanToNull(order.get(ValueLayout.JAVA_DOUBLE, Native.O_AVG));
        return new OrderInfo(id, side, status, quantity, filled, price, avg);
    }

    static TickerInfo readTicker(MemorySegment ticker) {
        String symbol = Native.readCString(ticker, Native.T_SYMBOL, Native.STR_CAP);
        double last = ticker.get(ValueLayout.JAVA_DOUBLE, Native.T_LAST);
        double bid = ticker.get(ValueLayout.JAVA_DOUBLE, Native.T_BID);
        double ask = ticker.get(ValueLayout.JAVA_DOUBLE, Native.T_ASK);
        double volume = ticker.get(ValueLayout.JAVA_DOUBLE, Native.T_VOLUME);
        return new TickerInfo(symbol, last, bid, ask, volume);
    }

    static CandleInfo readCandle(MemorySegment candle) {
        return new CandleInfo(
                candle.get(ValueLayout.JAVA_DOUBLE, Native.C_OPEN),
                candle.get(ValueLayout.JAVA_DOUBLE, Native.C_HIGH),
                candle.get(ValueLayout.JAVA_DOUBLE, Native.C_LOW),
                candle.get(ValueLayout.JAVA_DOUBLE, Native.C_CLOSE),
                candle.get(ValueLayout.JAVA_DOUBLE, Native.C_VOLUME),
                candle.get(ValueLayout.JAVA_LONG, Native.C_TIMESTAMP));
    }

    static BookLevelInfo readBookLevel(MemorySegment level) {
        return new BookLevelInfo(
                level.get(ValueLayout.JAVA_DOUBLE, Native.BL_PRICE),
                level.get(ValueLayout.JAVA_DOUBLE, Native.BL_QUANTITY));
    }

    static Event readEvent(MemorySegment event) {
        Kind kind = Kind.values()[event.get(ValueLayout.JAVA_INT, Native.E_KIND)];
        String symbol = Native.readCString(event, Native.E_SYMBOL, Native.STR_CAP);
        if (symbol.isEmpty()) {
            symbol = null;
        }
        Double price = nanToNull(event.get(ValueLayout.JAVA_DOUBLE, Native.E_PRICE));
        Double quantity = nanToNull(event.get(ValueLayout.JAVA_DOUBLE, Native.E_QUANTITY));
        int sideCode = event.get(ValueLayout.JAVA_INT, Native.E_SIDE);
        Side side = sideCode < 0 ? null : (sideCode == Native.SIDE_SELL ? Side.SELL : Side.BUY);
        OrderInfo order = kind == Kind.ORDER_UPDATE
                ? readOrder(event.asSlice(Native.E_ORDER, Native.ORDER_SIZE))
                : null;
        return new Event(kind, symbol, price, quantity, side, order);
    }

    private static Double nanToNull(double value) {
        return Double.isNaN(value) ? null : value;
    }

    static void check(int code) {
        if (code != Native.OK) {
            throw new RuntimeException("exchange call failed with code " + code);
        }
    }
}
