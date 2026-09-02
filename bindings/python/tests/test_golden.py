"""Golden-fixture parity for the Python binding.

The Rust suite (``crates/wickra-exchange-core/tests/golden.rs``) drives the
committed replay tapes in ``golden/`` through a ``ReplayExchange`` running a
fixed SMA strategy, and pins the fill price and the resulting balances. This
runs the same fixtures through the same pipeline from Python.

``test_replay.py`` already proves a tape reaches a fill. What it does not do is
check the *numbers*: a lost decimal, a dropped fee or slippage applied to the
wrong side would still produce a fill, and still pass. These assert the exact
values the Rust suite pins, so the Python path cannot drift from it silently.

The strategy is reimplemented rather than imported: pulling in the indicator
package to average three numbers would make this a test of two libraries
agreeing, when the thing under test is the replay-to-paper-fill pipeline.
"""

import json
import pathlib

import pytest

import wickra_exchange as wx

GOLDEN = pathlib.Path(__file__).resolve().parents[3] / "golden"

TOL = 1e-6


def _read(kind, name):
    return json.loads((GOLDEN / kind / f"{name}.json").read_text(encoding="utf-8"))


def _sma(window):
    values = []

    def update(price):
        values.append(price)
        if len(values) < window:
            return None
        return sum(values[-window:]) / window

    return update


def _run_case(name):
    spec = _read("replay", name)
    expected = _read("expected", name)

    exchange = wx.Exchange.replay_trades(
        spec["market"],
        spec["tape"],
        spec["balances"],
        spec["maker_bps"],
        spec["taker_bps"],
        spec["slippage_bps"],
    )

    sma = _sma(spec["sma_period"])
    fill_price = None

    # Each poll advances the recording by exactly one frame; an empty batch is
    # how an exhausted tape reports itself.
    while True:
        events = exchange.poll_events()
        if not events:
            break
        for event in events:
            if event["type"] != "trade":
                continue
            mean = sma(event["price"])
            if mean is not None and fill_price is None and event["price"] > mean:
                order = exchange.place_order(
                    wx.OrderRequest.market_buy(spec["market"], 1.0)
                )
                fill_price = order["average_price"]

    balances = exchange.balances()

    assert (fill_price is not None) == expected["filled"]
    assert fill_price == pytest.approx(expected["average_price"], abs=TOL)
    assert balances["BTC"] == pytest.approx(expected["btc"], abs=TOL)
    assert balances["USDT"] == pytest.approx(expected["usdt"], abs=TOL)


def test_golden_sma_cross_frictionless():
    _run_case("sma_cross")


def test_golden_sma_cross_with_costs():
    _run_case("sma_cross_with_costs")
