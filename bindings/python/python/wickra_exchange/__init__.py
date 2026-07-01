"""wickra-exchange: streaming-native, unified crypto-exchange connectivity.

One typed, synchronous, pull-based API over the ten largest exchanges — plus
offline paper and replay simulators that implement the exact same API, so the
same strategy runs paper, replay and live by swapping the constructor::

    import wickra_exchange as wx

    # Offline paper account, deterministic and network-free.
    ex = wx.Exchange.paper({"USDT": 100_000.0}, taker_bps=5.0)
    ex.set_price("BTC/USDT", 20_000.0)
    order = ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))
    assert order["status"] == "filled"

    # Live venue (needs API keys):
    #   creds = wx.Credentials("key", "secret")
    #   ex = wx.Exchange.connect("binance", creds)
"""

from __future__ import annotations

from ._wickra_exchange import (
    AdvancedOrders,
    Credentials,
    Derivatives,
    Exchange,
    OrderRequest,
    UserData,
    WsExecution,
    __version__,
)

__all__ = [
    "AdvancedOrders",
    "Credentials",
    "Derivatives",
    "Exchange",
    "OrderRequest",
    "UserData",
    "WsExecution",
    "__version__",
]
