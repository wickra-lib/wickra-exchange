"""Surface tests for the derivatives + advanced-orders clients.

Construction is offline (no socket opens until an RPC is issued), so the class
surface and the spot-only rejection are checked without a network.
"""

import wickra_exchange as wx


def test_module_exposes_new_classes():
    assert hasattr(wx, "Derivatives")
    assert hasattr(wx, "AdvancedOrders")


def test_derivatives_rejects_spot_only_venue():
    creds = wx.Credentials("key", "secret")
    for name in ("coinbase", "upbit", "ftx"):
        try:
            wx.Derivatives.connect(name, creds)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{name} must be rejected for derivatives")


def test_advanced_rejects_spot_only_venue():
    creds = wx.Credentials("key", "secret")
    for name in ("coinbase", "upbit", "ftx"):
        try:
            wx.AdvancedOrders.connect(name, creds)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{name} must be rejected for advanced orders")


def test_derivatives_and_advanced_construct_for_a_futures_venue():
    # A live handle constructs offline; no RPC is issued here.
    creds = wx.Credentials("key", "secret")
    assert wx.Derivatives.connect("binance", creds) is not None
    assert wx.AdvancedOrders.connect("binance", creds, futures=True) is not None
