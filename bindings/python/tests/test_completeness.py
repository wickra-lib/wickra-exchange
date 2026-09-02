"""Parity guard: every binding class exposes the full canonical verb set of the
Rust core traits, so a method dropped in a refactor fails loudly here (mirrors
the completeness check in the main wickra repo)."""

import wickra_exchange as wx

# MarketData (7) + Execution (5) + Exchange (1). place_order is the unified
# entry; set_price is the paper-only helper.
EXCHANGE_VERBS = [
    "ticker",
    "klines",
    "order_book",
    "subscribe_trades",
    "subscribe_book",
    "subscribe_ticker",
    "poll_events",
    "place_order",
    "cancel_order",
    "query_order",
    "open_orders",
    "balances",
    "name",
]

DERIVATIVES_VERBS = ["positions", "set_leverage", "set_margin_mode", "close_position"]

# The four constructors plus every builder that decides what the order *is*.
# Without these an order was a market/limit with a quantity and a price and
# nothing else, so a stop-loss, an IOC, a post-only or an idempotent retry
# could not be expressed from Python at all -- however much of it the Rust
# core supported.
ORDER_REQUEST_VERBS = [
    "market_buy",
    "market_sell",
    "limit_buy",
    "limit_sell",
    "with_stop_price",
    "with_time_in_force",
    "with_client_order_id",
    "reduce_only",
    "post_only",
    "with_stp",
]
ADVANCED_VERBS = ["amend_order", "place_batch", "cancel_batch", "place_oco"]
USER_DATA_VERBS = ["subscribe_user_data", "keepalive_user_data", "poll_events"]
WS_EXECUTION_VERBS = ["place_order_ws", "cancel_order_ws"]


def _assert_verbs(cls, verbs):
    for verb in verbs:
        assert callable(getattr(cls, verb, None)), f"{cls.__name__} is missing {verb}"


def test_exchange_surface_complete():
    _assert_verbs(wx.Exchange, EXCHANGE_VERBS)


def test_derivatives_surface_complete():
    _assert_verbs(wx.Derivatives, DERIVATIVES_VERBS)


def test_advanced_surface_complete():
    _assert_verbs(wx.AdvancedOrders, ADVANCED_VERBS)


def test_user_data_surface_complete():
    _assert_verbs(wx.UserData, USER_DATA_VERBS)


def test_ws_execution_surface_complete():
    _assert_verbs(wx.WsExecution, WS_EXECUTION_VERBS)


def test_order_request_surface_complete():
    _assert_verbs(wx.OrderRequest, ORDER_REQUEST_VERBS)


def test_order_request_builders_compose():
    """The builders chain, and each returns a request rather than mutating."""
    base = wx.OrderRequest.limit_sell("BTC/USDT", 1.0, 19000.0)
    built = (
        base.with_stop_price(19500.0)
        .with_time_in_force("IOC")
        .with_client_order_id("retry-safe-1")
        .reduce_only()
    )
    assert built is not base
    assert repr(built) != repr(base)


def test_unknown_time_in_force_raises_rather_than_defaulting():
    """A typo must not quietly become GTC: that is the defect #195 fixed in the
    core, and it would be reintroduced here by a lenient parse."""
    request = wx.OrderRequest.limit_buy("BTC/USDT", 1.0, 19000.0)
    for bad in ["", "gtd", "immediate"]:
        try:
            request.with_time_in_force(bad)
        except ValueError:
            continue
        raise AssertionError(f"with_time_in_force({bad!r}) was accepted")

    for bad in ["", "cancel_maker"]:
        try:
            request.with_stp(bad)
        except ValueError:
            continue
        raise AssertionError(f"with_stp({bad!r}) was accepted")


def test_time_in_force_and_stp_are_case_insensitive():
    request = wx.OrderRequest.limit_buy("BTC/USDT", 1.0, 19000.0)
    assert request.with_time_in_force("ioc") is not None
    assert request.with_time_in_force("IOC") is not None
    assert request.with_stp("EXPIRE_MAKER") is not None
    assert request.with_stp("expire_both") is not None
