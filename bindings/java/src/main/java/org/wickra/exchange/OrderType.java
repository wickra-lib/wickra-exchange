package org.wickra.exchange;

/**
 * What kind of order this is: whether it takes the market now, rests at a
 * price, or waits for a trigger.
 */
public enum OrderType {
    /** Take the best available price now. */
    MARKET(Native.ORDER_MARKET),
    /** Rest at the limit price until filled or cancelled. */
    LIMIT(Native.ORDER_LIMIT),
    /** Rest until the market reaches the stop price, then take the market. */
    STOP_MARKET(Native.ORDER_STOP_MARKET),
    /** Rest until the market reaches the stop price, then rest at the limit price. */
    STOP_LIMIT(Native.ORDER_STOP_LIMIT);

    private final int code;

    OrderType(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
