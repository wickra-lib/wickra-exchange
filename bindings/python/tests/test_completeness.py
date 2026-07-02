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
