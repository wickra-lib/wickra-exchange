"""Smoke tests for the paper exchange over the pull API."""

import wickra_exchange as wx


def test_module_surface():
    assert isinstance(wx.__version__, str)
    assert hasattr(wx, "Credentials")
    assert hasattr(wx, "Exchange")
    assert hasattr(wx, "OrderRequest")


def test_credentials_construct():
    # Construction must not raise for the two- and four-argument forms.
    wx.Credentials("key", "secret")
    wx.Credentials("key", "secret", passphrase="pass")
    wx.Credentials("key", "secret", private_key="-----BEGIN-----")


def test_paper_market_buy_fills():
    ex = wx.Exchange.paper({"USDT": 100_000.0}, taker_bps=5.0, slippage_bps=10.0)
    assert ex.name() == "paper"
    ex.set_price("BTC/USDT", 20_000.0)

    order = ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))
    assert order["status"] == "filled"
    assert order["side"] == "buy"
    # 10 bps slippage on a buy: 20000 * 1.001 = 20020.
    assert abs(order["average_price"] - 20020.0) < 1e-6

    balances = ex.balances()
    assert abs(balances["BTC"] - 1.0) < 1e-9
    # 20020 notional + 5 bps fee (10.01) spent from USDT.
    assert abs(balances["USDT"] - (100_000.0 - 20_020.0 - 10.01)) < 1e-6


def test_paper_events_and_ticker():
    ex = wx.Exchange.paper({"USDT": 50_000.0})
    ex.set_price("BTC/USDT", 20_000.0)
    ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))

    events = ex.poll_events()
    kinds = [e["type"] for e in events]
    assert "order_update" in kinds
    assert "balance_update" in kinds

    ticker = ex.ticker("BTC/USDT")
    assert abs(ticker["last"] - 20_000.0) < 1e-9


def test_resting_limit_and_cancel():
    ex = wx.Exchange.paper({"USDT": 100_000.0})
    ex.set_price("BTC/USDT", 20_000.0)
    order = ex.place_order(wx.OrderRequest.limit_buy("BTC/USDT", 1.0, 19_000.0))
    assert order["status"] == "new"
    ex.cancel_order("BTC/USDT", order["id"])
    assert abs(ex.balances()["USDT"] - 100_000.0) < 1e-9


def test_bad_market_string_raises():
    ex = wx.Exchange.paper({"USDT": 1.0})
    try:
        ex.set_price("BTCUSDT", 1.0)
    except ValueError:
        pass
    else:
        raise AssertionError("expected ValueError for a market without '/'")
