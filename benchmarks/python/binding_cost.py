"""What it costs to reach the library from Python.

Same two operations, same offline paper account, same iteration count as every
other program in this directory and as the Rust baseline. The difference from
the baseline is this binding's overhead.

    python binding_cost.py
"""

import time

import wickra_exchange as wx

ITERATIONS = 20_000
WARMUP = 1_000


def report(operation: str, nanos: float) -> None:
    per_call = nanos / ITERATIONS
    print(f"{operation:<12} {per_call:>10.0f} ns/op   {1e9 / per_call:>12.0f} ops/s")


def measure(iterations: int, work) -> float:
    started = time.perf_counter_ns()
    for _ in range(iterations):
        work()
    return time.perf_counter_ns() - started


ex = wx.Exchange.paper({"USDT": 1e9})
ex.set_price("BTC/USDT", 20_000.0)

# The first call through any boundary pays for one-time setup, which is not what
# is being measured.
measure(WARMUP, lambda: ex.ticker("BTC/USDT"))
report("ticker", measure(ITERATIONS, lambda: ex.ticker("BTC/USDT")))

request = wx.OrderRequest.market_buy("BTC/USDT", 0.0001)
measure(WARMUP, lambda: ex.place_order(request))
report("place_order", measure(ITERATIONS, lambda: ex.place_order(request)))
