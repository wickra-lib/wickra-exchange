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


def test_exchange_can_reach_a_futures_market():
    # `Exchange.connect` used to build a spot client and nothing else, so no
    # Python caller could place a futures order, read a futures book or cancel
    # a futures order. Construction is offline; this pins that the door exists.
    creds = wx.Credentials("key", "secret")
    for market in ("spot", "usdm_futures"):
        assert wx.Exchange.connect("binance", creds, market=market) is not None


def test_exchange_carries_the_margin_and_position_modes():
    # Two venues carry the margin mode on every order and four carry the
    # position side, so neither can be set after the first order is placed.
    creds = wx.Credentials("key", "secret")
    assert (
        wx.Exchange.connect(
            "okx",
            creds,
            market="usdm_futures",
            margin_mode="isolated",
            position_mode="hedge",
        )
        is not None
    )


def test_bad_market_and_mode_strings_are_rejected():
    creds = wx.Credentials("key", "secret")
    for kwargs in (
        {"market": "perpetual"},
        # Not offered: no client routes coin-margined consistently, and Binance
        # treats it as spot outright.
        {"market": "coinm_futures"},
        {"margin_mode": "portfolio"},
        {"position_mode": "both"},
    ):
        try:
            wx.Exchange.connect("binance", creds, **kwargs)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{kwargs} must be rejected")
