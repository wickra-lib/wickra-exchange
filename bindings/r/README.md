# wickra.exchange (R)

R bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI (`.Call`): one synchronous, pull-based API over the ten
largest crypto exchanges, plus offline paper and replay simulators that share the
same API.

```r
library(wickraexchange)

ex <- wkex_paper(c(USDT = 100000), taker_bps = 5)
wkex_set_price(ex, "BTC/USDT", 20000)
order <- wkex_place_market(ex, "BTC/USDT", "buy", 1)
order$status          # "filled"
wkex_balance(ex, "BTC")  # 1
```

Build against the C ABI via the `WKEX_INC` (header) and `WKEX_LIB` (library)
environment variables. The same strategy runs **paper, replay and live** by
swapping the constructor. Licensed under `MIT OR Apache-2.0`.
