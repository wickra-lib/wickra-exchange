# wickra-exchange (Python)

Python bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange):
streaming-native, unified connectivity for the ten largest crypto exchanges, with
offline paper and replay simulators that share the exact same API.

```python
import wickra_exchange as wx

# Offline paper account — deterministic, network-free.
ex = wx.Exchange.paper({"USDT": 100_000.0}, taker_bps=5.0)
ex.set_price("BTC/USDT", 20_000.0)
order = ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))
assert order["status"] == "filled"
print(ex.balances())

# Replay a recorded tape through the same API:
ex = wx.Exchange.replay_trades("BTC/USDT", [100.0, 101.0, 110.0], {"USDT": 10_000.0})
while (events := ex.poll_events()):
    for event in events:
        ...  # drive your strategy

# Live venue (needs API keys):
#   creds = wx.Credentials("key", "secret")
#   ex = wx.Exchange.connect("binance", creds)
```

The same strategy runs **paper, replay and live** by swapping the constructor.

## Build

```bash
maturin develop --release
python -m pytest tests -q
```

Licensed under `MIT OR Apache-2.0`.
