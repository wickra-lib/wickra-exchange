"""Surface tests for the user-data + ws-execution clients.

Construction is offline (no socket opens until an RPC is issued), so the class
surface and the spot-only rejection are checked without a network.
"""

import wickra_exchange as wx


def test_module_exposes_ws_classes():
    assert hasattr(wx, "UserData")
    assert hasattr(wx, "WsExecution")


def test_user_data_rejects_spot_only_venue():
    creds = wx.Credentials("key", "secret")
    for name in ("coinbase", "upbit", "ftx"):
        try:
            wx.UserData.connect(name, creds)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{name} must be rejected for user-data streaming")


def test_ws_execution_rejects_spot_only_venue():
    creds = wx.Credentials("key", "secret")
    for name in ("coinbase", "upbit", "ftx"):
        try:
            wx.WsExecution.connect(name, creds)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{name} must be rejected for ws execution")


def test_ws_clients_construct_and_expose_the_surface():
    # A live handle constructs offline; no RPC is issued here.
    creds = wx.Credentials("key", "secret")
    user_data = wx.UserData.connect("binance", creds)
    assert user_data is not None
    # WsUserData: MarketData, so the client can poll (nothing is buffered yet).
    assert user_data.poll_events() == []
    assert hasattr(user_data, "subscribe_user_data")

    execution = wx.WsExecution.connect("bybit", creds)
    assert execution is not None
    assert hasattr(execution, "place_order_ws")
    assert hasattr(execution, "cancel_order_ws")
