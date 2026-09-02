package org.wickra.exchange;

/**
 * How long an order may live.
 *
 * <p>This is not decoration: an {@link #IOC} that reaches a venue as a
 * {@link #GTC} rests in the book the caller asked it never to rest in.
 */
public enum TimeInForce {
    /** Rest until cancelled. */
    GTC(Native.TIF_GTC),
    /** Fill what is possible now, cancel the rest. */
    IOC(Native.TIF_IOC),
    /** Fill entirely now or not at all. */
    FOK(Native.TIF_FOK);

    private final int code;

    TimeInForce(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
