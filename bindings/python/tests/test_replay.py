"""Replay-parity test: a recorded tape drives a signal that fills on the book.

This mirrors the Rust end-to-end test (`recorded_tape_drives_indicator_signal_to_a_fill`):
a rising price tape breaks above a 3-period moving average, and the resulting
market buy fills on the paper book — proving the replay path behaves identically
from Python.
"""

import wickra_exchange as wx


def _sma(window):
    """A tiny streaming simple moving average, so the test needs no wickra dep."""
    values = []

    def update(price):
        values.append(price)
        if len(values) < window:
            return None
        return sum(values[-window:]) / window

    return update


def test_replay_tape_drives_signal_to_fill():
    tape = [100.0, 101.0, 102.0, 110.0, 112.0]
    ex = wx.Exchange.replay_trades("BTC/USDT", tape, {"USDT": 100_000.0})
    assert ex.name() == "replay"

    sma = _sma(3)
    bought = False

    while True:
        events = ex.poll_events()
        if not events:
            break
        for event in events:
            if event["type"] != "trade":
                continue
            mean = sma(event["price"])
            if mean is not None and not bought and event["price"] > mean:
                order = ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))
                assert order["status"] == "filled"
                bought = True

    assert bought, "the rising tape should have crossed the SMA"
    assert abs(ex.balances()["BTC"] - 1.0) < 1e-9


def test_replay_finishes_and_stops_yielding():
    ex = wx.Exchange.replay_trades("BTC/USDT", [100.0, 101.0], {"USDT": 1_000.0})
    seen = 0
    while True:
        events = ex.poll_events()
        if not events:
            break
        seen += len(events)
    assert seen >= 2
