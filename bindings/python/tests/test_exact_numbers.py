"""An order number arrives as the number that was written.

A `float` holds about fifteen significant digits, and the core holds every order
number in an exact decimal. Sent as a float, `12345678.90123456789` becomes
`12345678.90123457` -- a different order, placed without a word. Python has
`decimal.Decimal` in its standard library, and it was going through a float on
the way in.

`repr()` is the exact read-back: every other number this binding reports is a
float, so it is the only place the difference can be seen.
"""

from decimal import Decimal

import pytest

import wickra_exchange as wx

# Wider than a double: the last digits are the ones a float cannot hold.
WIDE = "12345678.90123456789"
TINY = "0.000000012345678901234567"
BIG_INT = 123456789012345678


@pytest.mark.parametrize("written", [Decimal(WIDE), WIDE])
def test_a_wide_price_survives_as_decimal_or_str(written):
    """Both exact spellings reach the order intact."""
    request = wx.OrderRequest.limit_buy("BTC/USDT", 1, written)
    assert f"price={WIDE}" in repr(request)


def test_the_same_number_through_a_float_does_not():
    """What the float path does to it, measured rather than asserted from docs."""
    request = wx.OrderRequest.limit_buy("BTC/USDT", 1, float(WIDE))
    assert f"price={WIDE}" not in repr(request)
    assert "price=12345678.90123457" in repr(request)


def test_a_float_still_works_and_is_still_exact_for_ordinary_numbers():
    """The ordinary spelling keeps working; nothing about it changed."""
    request = wx.OrderRequest.limit_buy("BTC/USDT", 1.5, 19_000.5)
    assert "quantity=1.5" in repr(request)
    assert "price=19000.5" in repr(request)


def test_an_int_is_exact_where_a_float_stops_being_one():
    """123456789012345678 is not representable as a double; the nearest is ...680."""
    assert f"price={BIG_INT}" in repr(wx.OrderRequest.limit_buy("BTC/USDT", 1, BIG_INT))
    assert f"price={BIG_INT}" not in repr(
        wx.OrderRequest.limit_buy("BTC/USDT", 1, float(BIG_INT))
    )


def test_a_tiny_exact_quantity_is_not_rounded_away():
    request = wx.OrderRequest.market_sell("BTC/USDT", Decimal(TINY))
    assert f"quantity={TINY}" in repr(request)


def test_a_stop_price_takes_the_same_spellings():
    for written in (Decimal(WIDE), WIDE):
        request = wx.OrderRequest.market_sell("BTC/USDT", 1).with_stop_price(written)
        assert f"stop_price={WIDE}" in repr(request)


def test_a_number_that_is_not_a_number_is_refused():
    """An unparsable price is a refused order, not an order at some other price."""
    with pytest.raises(ValueError):
        wx.OrderRequest.limit_buy("BTC/USDT", 1, "nineteen thousand")


def test_a_bool_is_not_a_quantity():
    """`True` is an `int` in Python. A quantity of `True` is a mistake, not one."""
    with pytest.raises(ValueError):
        wx.OrderRequest.market_buy("BTC/USDT", True)


def test_an_exact_order_still_places():
    """The exact spelling is not a separate path: it places like any other."""
    ex = wx.Exchange.paper({"USDT": 100_000.0})
    ex.set_price("BTC/USDT", 20_000.0)
    order = ex.place_order(wx.OrderRequest.limit_sell("BTC/USDT", "1.5", "21000.5"))
    assert order["status"] == "new"
    assert order["price"] == pytest.approx(21_000.5)
