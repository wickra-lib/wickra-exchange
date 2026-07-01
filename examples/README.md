# Examples

Runnable examples in every supported language. Each `paper_trade` example is the
differentiator demo — an offline paper account fills orders deterministically
through the same API a live venue uses.

| Language | Path                       | Run                                            |
|----------|----------------------------|------------------------------------------------|
| Rust     | `rust/`                    | `cargo run -p wickra-exchange-examples --bin paper_trade` |
| Python   | `python/paper_trade.py`    | `python paper_trade.py`                         |
| Node.js  | `node/paper_trade.js`      | `node paper_trade.js`                           |
| C        | `c/replay.c`, `c/paper.cpp`| build via `c/CMakeLists.txt`                    |
| C#       | `csharp/`                  | `dotnet run`                                    |
| Go       | `go/paper_trade.go`        | `go run paper_trade.go`                         |
| Java     | `java/PaperTrade.java`     | `java --enable-native-access=ALL-UNNAMED ...`   |
| R        | `r/paper_trade.R`          | `Rscript paper_trade.R`                         |

The `rust/ticker` binary fetches a live public ticker:
`cargo run -p wickra-exchange-examples --bin ticker -- binance BTC/USDT`.

Each example requires the corresponding binding to be built/installed first (see
each `bindings/<lang>/README.md`).
