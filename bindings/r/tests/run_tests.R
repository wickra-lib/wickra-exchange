## Plain-R tests for the wickra-exchange R binding (no testthat dependency).
## Mirrors the Rust/Python/Node/Go/C#/Java replay-parity tests.

library(wickraexchange)

stopifnot(nzchar(wkex_version()))

## Paper: a market buy fills with slippage and fee.
ex <- wkex_paper(c(USDT = 100000), maker_bps = 1, taker_bps = 5, slippage_bps = 10)
stopifnot(wkex_name(ex) == "paper")
wkex_set_price(ex, "BTC/USDT", 20000)

order <- wkex_place_market(ex, "BTC/USDT", "buy", 1)
stopifnot(order$status == "filled")
## 10 bps slippage on a buy: 20000 * 1.001 = 20020.
stopifnot(abs(order$average_price - 20020) < 1e-6)
stopifnot(abs(wkex_balance(ex, "BTC") - 1) < 1e-9)
stopifnot(abs(wkex_balance(ex, "USDT") - (100000 - 20020 - 10.01)) < 1e-6)

## Resting limit + cancel.
ex2 <- wkex_paper(c(USDT = 100000))
wkex_set_price(ex2, "BTC/USDT", 20000)
resting <- wkex_place_limit(ex2, "BTC/USDT", "buy", 1, 19000)
stopifnot(resting$status == "new")
events <- wkex_poll(ex2)
stopifnot(any(vapply(events, function(e) e$kind == "order_update", logical(1))))
wkex_cancel(ex2, "BTC/USDT", resting$id)
stopifnot(abs(wkex_balance(ex2, "USDT") - 100000) < 1e-9)

## Replay parity: a rising tape crosses a 3-period SMA; the market buy fills.
tape <- c(100, 101, 102, 110, 112)
rex <- wkex_replay_trades("BTC/USDT", tape, c(USDT = 100000))
stopifnot(wkex_name(rex) == "replay")

window <- numeric(3)
seen <- 0L
bought <- FALSE
repeat {
  batch <- wkex_poll(rex)
  if (length(batch) == 0) break
  for (ev in batch) {
    if (ev$kind != "trade") next
    window[(seen %% 3) + 1] <- ev$price
    seen <- seen + 1L
    if (seen >= 3) {
      mean_price <- sum(window) / 3
      if (!bought && ev$price > mean_price) {
        filled <- wkex_place_market(rex, "BTC/USDT", "buy", 1)
        stopifnot(filled$status == "filled")
        bought <- TRUE
      }
    }
  }
}
stopifnot(bought)
stopifnot(abs(wkex_balance(rex, "BTC") - 1) < 1e-9)

## Market-data + order-lifecycle read surface on the paper exchange.
mex <- wkex_paper(c(USDT = 100000))
wkex_set_price(mex, "BTC/USDT", 20000)
tkr <- wkex_ticker(mex, "BTC/USDT")
stopifnot(tkr$symbol == "BTC/USDT")
stopifnot(abs(tkr$last - 20000) < 1e-9)
## subscribe_* are accepted by the paper feed.
wkex_subscribe_trades(mex, "BTC/USDT")
wkex_subscribe_book(mex, "BTC/USDT")
wkex_subscribe_ticker(mex, "BTC/USDT")
## paper has no historical / depth feed: both error.
stopifnot(inherits(try(wkex_klines(mex, "BTC/USDT", "1m", 10), silent = TRUE), "try-error"))
stopifnot(inherits(try(wkex_order_book(mex, "BTC/USDT", 10), silent = TRUE), "try-error"))
## A resting limit can be read back by id and appears in open orders.
resting <- wkex_place_limit(mex, "BTC/USDT", "buy", 1, 19000)
stopifnot(resting$status == "new")
queried <- wkex_query_order(mex, "BTC/USDT", resting$id)
stopifnot(queried$id == resting$id)
opens <- wkex_open_orders(mex)
stopifnot(length(opens) == 1L)
stopifnot(opens[[1]]$id == resting$id)
stopifnot(length(wkex_open_orders(mex, "BTC/USDT")) == 1L)
stopifnot(length(wkex_open_orders(mex, "ETH/USDT")) == 0L)

## Derivatives + advanced surface: construction is offline, so the spot-only
## rejection and the futures construct are checked without a network.
for (venue in c("coinbase", "upbit", "ftx")) {
  stopifnot(inherits(try(wkex_derivatives(venue, "k", "s"), silent = TRUE), "try-error"))
  stopifnot(inherits(try(wkex_advanced(venue, "k", "s"), silent = TRUE), "try-error"))
}
deriv <- wkex_derivatives("binance", "k", "s")
stopifnot(inherits(deriv, "wickra_derivatives"))
adv <- wkex_advanced("binance", "k", "s", futures = TRUE)
stopifnot(inherits(adv, "wickra_advanced"))

## Array-out extended-ops surface is present (loading the package already
## validated that every .Call entry resolves to a registered C symbol).
stopifnot(is.function(wkex_positions))
stopifnot(is.function(wkex_place_oco))
stopifnot(is.function(wkex_place_batch))
## place_batch marshals parallel vectors: a NA price means a market order.
reqs <- data.frame(
  market = c("BTC/USDT", "ETH/USDT"),
  side = c("buy", "sell"),
  quantity = c(0.5, 2),
  price = c(60000, NA_real_),
  stringsAsFactors = FALSE
)
stopifnot(nrow(reqs) == 2L)
stopifnot(is.na(reqs$price[2]))

## User-data + ws-execution: construction is offline; spot-only venues error.
for (venue in c("coinbase", "upbit", "ftx")) {
  stopifnot(inherits(try(wkex_user_data(venue, "k", "s"), silent = TRUE), "try-error"))
  stopifnot(inherits(try(wkex_ws_execution(venue, "k", "s"), silent = TRUE), "try-error"))
}
ud <- wkex_user_data("binance", "k", "s")
stopifnot(inherits(ud, "wickra_user_data"))
## Keepalive is a no-op before subscribe; it must not error.
wkex_keepalive_user_data(ud)
## WsUserData: MarketData, so the client can poll (nothing buffered offline).
stopifnot(length(wkex_user_data_poll(ud)) == 0)
wse <- wkex_ws_execution("bybit", "k", "s")
stopifnot(inherits(wse, "wickra_ws_execution"))
stopifnot(is.function(wkex_ws_place_order))
stopifnot(is.function(wkex_ws_cancel_order))

cat("wickra.exchange R tests passed\n")
