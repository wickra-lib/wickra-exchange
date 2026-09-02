package org.wickra.exchange;

/**
 * Which side to cancel when an order would match the account's own resting
 * order.
 */
public enum SelfTradePrevention {
    /** Let the account trade against itself. */
    NONE(Native.STP_NONE),
    /** Cancel the resting order. */
    EXPIRE_MAKER(Native.STP_EXPIRE_MAKER),
    /** Cancel the incoming order. */
    EXPIRE_TAKER(Native.STP_EXPIRE_TAKER),
    /** Cancel both. */
    EXPIRE_BOTH(Native.STP_EXPIRE_BOTH);

    private final int code;

    SelfTradePrevention(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
