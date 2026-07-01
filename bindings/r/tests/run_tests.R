## Plain-R tests for the wickra-exchange R binding (no testthat dependency).
## Mirrors the Rust/Python/Node/Go/C#/Java replay-parity tests.

library(wickra.exchange)

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

cat("wickra.exchange R tests passed\n")
