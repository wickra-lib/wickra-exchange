"""Market-data + order-lifecycle read surface on the paper exchange."""

import pytest

import wickra_exchange as wx


def _seeded():
    ex = wx.Exchange.paper({"USDT": 100_000.0})
    ex.set_price("BTC/USDT", 20_000.0)
    return ex


def test_subscribe_streams_are_accepted():
    ex = _seeded()
    # The paper feed accepts every subscription without raising.
    ex.subscribe_trades("BTC/USDT")
    ex.subscribe_book("BTC/USDT")
    ex.subscribe_ticker("BTC/USDT")


def test_klines_and_order_book_report_unsupported_on_paper():
    ex = _seeded()
    # Paper has no historical / depth feed: both raise rather than fabricate.
    with pytest.raises(Exception):
        ex.klines("BTC/USDT", "1m", 10)
    with pytest.raises(Exception):
        ex.order_book("BTC/USDT", 10)


def test_query_order_and_open_orders():
    ex = _seeded()
    resting = ex.place_order(wx.OrderRequest.limit_buy("BTC/USDT", 1.0, 19_000.0))
    assert resting["status"] == "new"

    queried = ex.query_order("BTC/USDT", resting["id"])
    assert queried["id"] == resting["id"]

    opens = ex.open_orders()
    assert len(opens) == 1
    assert opens[0]["id"] == resting["id"]

    # Scoped to the same symbol still returns it; unknown symbol returns none.
    assert len(ex.open_orders("BTC/USDT")) == 1
    assert ex.open_orders("ETH/USDT") == []
