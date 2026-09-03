## What it costs to reach the library from R.
##
## Same two operations, same offline paper account, same iteration count as
## every other program in this directory and as the Rust baseline. The
## difference from the baseline is this binding's overhead.
##
##   Rscript binding_cost.R

library(wickraexchange)

ITERATIONS <- 20000L
WARMUP <- 1000L

report <- function(operation, nanos) {
  per_call <- nanos / ITERATIONS
  cat(sprintf("%-12s %10.0f ns/op   %12.0f ops/s\n", operation, per_call, 1e9 / per_call))
}

measure <- function(iterations, work) {
  started <- Sys.time()
  for (i in seq_len(iterations)) {
    work()
  }
  as.numeric(difftime(Sys.time(), started, units = "secs")) * 1e9
}

ex <- wkex_paper(c(USDT = 1e9))
wkex_set_price(ex, "BTC/USDT", 20000)

## The first call through any boundary pays for one-time setup, which is not
## what is being measured.
invisible(measure(WARMUP, function() wkex_ticker(ex, "BTC/USDT")))
report("ticker", measure(ITERATIONS, function() wkex_ticker(ex, "BTC/USDT")))

invisible(measure(WARMUP, function() wkex_place_market(ex, "BTC/USDT", "buy", 0.0001)))
report("place_order",
       measure(ITERATIONS, function() wkex_place_market(ex, "BTC/USDT", "buy", 0.0001)))
