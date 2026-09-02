package org.wickra.exchange;

/**
 * Which of a venue's markets a client trades.
 *
 * <p>A single venue such as Binance is several APIs behind one name, with
 * different hosts, endpoints and symbol filters. This is the choice between
 * them, made at construction because it decides where every later call is
 * routed. Only spot and USDⓈ-margined futures are offered: no client routes
 * coin-margined or margin consistently, and Binance treats coin-margined as
 * spot outright.
 */
public enum Market {
    /** Spot. */
    SPOT(Native.MARKET_SPOT),
    /** USDⓈ-margined linear perpetual / futures. */
    USDM_FUTURES(Native.MARKET_USDM_FUTURES);

    private final int code;

    Market(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
