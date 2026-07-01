# wickra-exchange (Go)

Go bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI (cgo): one synchronous, pull-based API over the ten largest
crypto exchanges, plus offline paper and replay simulators that share the same API.

```go
ex, _ := wickraexchange.Paper(map[string]float64{"USDT": 100000}, 0, 5, 0)
defer ex.Close()
ex.SetPrice("BTC/USDT", 20000)
order, _ := ex.PlaceMarket("BTC/USDT", wickraexchange.Buy, 1)
fmt.Println(order.IsFilled())      // true
```

The C ABI header is vendored under `include/`; the prebuilt library is staged per
platform under `lib/<goos>_<goarch>/`. The same strategy runs **paper, replay and
live** by swapping the constructor. Licensed under `MIT OR Apache-2.0`.
