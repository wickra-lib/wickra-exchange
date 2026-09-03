package org.wickra.exchange;

import java.math.BigDecimal;

/**
 * A full order, as the caller wants it placed.
 *
 * <p>{@link Exchange#placeMarket} and {@link Exchange#placeLimit} take a market,
 * a side, a quantity and a price, which is all an order could ever be from this
 * binding. Everything else the library supports had no way through: the trigger
 * price that makes a stop-loss a stop-loss, the time-in-force that says an order
 * must not rest, post-only, reduce-only, self-trade prevention, and the client
 * order id that makes a retried placement idempotent.
 *
 * <p>Build one with {@link #market}, {@link #limit} or {@link #stop} and add the
 * optional fields with the {@code with*} methods, each of which returns a new
 * request:
 *
 * <pre>{@code
 * OrderRequest stopLoss = OrderRequest.stop("BTC/USDT", Exchange.Side.SELL, 1.0, 19_000.0)
 *         .withClientOrderId("protect-position-1")
 *         .withReduceOnly();
 * }</pre>
 *
 * <p>A {@code null} price is unset, which is how the C ABI is told a field
 * carries no value.
 */
public record OrderRequest(
        String market,
        Exchange.Side side,
        OrderType type,
        double quantity,
        Double price,
        Double stopPrice,
        TimeInForce timeInForce,
        String clientOrderId,
        boolean reduceOnly,
        boolean postOnly,
        SelfTradePrevention stp,
        BigDecimal exactQuantity,
        BigDecimal exactPrice,
        BigDecimal exactStopPrice) {

    /**
     * The eleven-field form, with no exact numbers set.
     *
     * <p>Kept so that everything built before the exact fields existed still
     * compiles and behaves identically.
     */
    public OrderRequest(String market, Exchange.Side side, OrderType type, double quantity,
                        Double price, Double stopPrice, TimeInForce timeInForce,
                        String clientOrderId, boolean reduceOnly, boolean postOnly,
                        SelfTradePrevention stp) {
        this(market, side, type, quantity, price, stopPrice, timeInForce, clientOrderId,
                reduceOnly, postOnly, stp, null, null, null);
    }

    /**
     * Place exactly this quantity, whatever {@link #quantity()} says.
     *
     * <p>A {@code double} holds about fifteen significant digits; the core holds
     * every order number in an exact decimal. Sent as a double,
     * {@code 12345678.90123456789} arrives as {@code 12345678.90123457} — a
     * different order, placed without a word. A {@link BigDecimal} set here
     * reaches the venue as written.
     */
    public OrderRequest withExactQuantity(BigDecimal exactQuantity) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice,
                exactStopPrice);
    }

    /** Rest at exactly this price, whatever {@link #price()} says. */
    public OrderRequest withExactPrice(BigDecimal exactPrice) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice,
                exactStopPrice);
    }

    /** Trigger at exactly this price, whatever {@link #stopPrice()} says. */
    public OrderRequest withExactStopPrice(BigDecimal exactStopPrice) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice,
                exactStopPrice);
    }

    /** A market order: take the best available price now. */
    public static OrderRequest market(String market, Exchange.Side side, double quantity) {
        return new OrderRequest(market, side, OrderType.MARKET, quantity, null, null,
                TimeInForce.GTC, null, false, false, SelfTradePrevention.NONE);
    }

    /** A limit order: rest at {@code price} until filled or cancelled. */
    public static OrderRequest limit(String market, Exchange.Side side, double quantity,
                                     double price) {
        return new OrderRequest(market, side, OrderType.LIMIT, quantity, price, null,
                TimeInForce.GTC, null, false, false, SelfTradePrevention.NONE);
    }

    /**
     * A stop-market order: rest until the market reaches {@code stopPrice}, then
     * take the market.
     *
     * <p>This is the order no binding could place. The core has carried the
     * trigger price since the venue clients stopped flattening it, and nothing
     * outside Rust could set one.
     */
    public static OrderRequest stop(String market, Exchange.Side side, double quantity,
                                    double stopPrice) {
        return new OrderRequest(market, side, OrderType.STOP_MARKET, quantity, null, stopPrice,
                TimeInForce.GTC, null, false, false, SelfTradePrevention.NONE);
    }

    /**
     * A stop-limit order: rest until the market reaches {@code stopPrice}, then
     * rest at {@code price}.
     */
    public static OrderRequest stopLimit(String market, Exchange.Side side, double quantity,
                                         double price, double stopPrice) {
        return new OrderRequest(market, side, OrderType.STOP_LIMIT, quantity, price, stopPrice,
                TimeInForce.GTC, null, false, false, SelfTradePrevention.NONE);
    }

    /** How long the order may live. */
    public OrderRequest withTimeInForce(TimeInForce timeInForce) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice, exactStopPrice);
    }

    /**
     * An id of the caller's choosing, so a retried placement is recognised by
     * the venue as the same order rather than placed twice.
     */
    public OrderRequest withClientOrderId(String clientOrderId) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice, exactStopPrice);
    }

    /** Close-only: the order may not increase a position. */
    public OrderRequest withReduceOnly() {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, true, postOnly, stp, exactQuantity, exactPrice, exactStopPrice);
    }

    /** Maker-only: the order is cancelled rather than crossing the spread. */
    public OrderRequest withPostOnly() {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, true, stp, exactQuantity, exactPrice, exactStopPrice);
    }

    /** Which side to cancel when the order would match the account's own. */
    public OrderRequest withStp(SelfTradePrevention stp) {
        return new OrderRequest(market, side, type, quantity, price, stopPrice, timeInForce,
                clientOrderId, reduceOnly, postOnly, stp, exactQuantity, exactPrice, exactStopPrice);
    }
}
