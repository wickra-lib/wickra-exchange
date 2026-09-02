package org.wickra.exchange;

/**
 * One net position per symbol, or a long and a short at once.
 *
 * <p>Hedge mode is not a later setting: on a hedged account every order names
 * the side of the account it acts on, so an order placed before the mode is
 * known carries the wrong one.
 */
public enum PositionMode {
    /** A single net position per symbol. */
    ONE_WAY(Native.POSITION_ONE_WAY),
    /** Separate long and short positions per symbol. */
    HEDGE(Native.POSITION_HEDGE);

    private final int code;

    PositionMode(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
