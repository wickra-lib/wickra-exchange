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
## The full-request forms: a batched or socket-sent order from R could carry
## only market, side, quantity and price, so a stop-loss was unplaceable on
## either path however carefully the venue clients carried the trigger.
stopifnot(is.function(wkex_place_batch_full))
stopifnot(is.function(wkex_ws_place_order_full))
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

## Completeness guard: every canonical verb is exported as a function, so a
## dropped wrapper fails loudly here (mirrors the main wickra repo's check).
for (verb in c(
  "wkex_ticker", "wkex_klines", "wkex_order_book", "wkex_subscribe_trades",
  "wkex_subscribe_book", "wkex_subscribe_ticker", "wkex_poll", "wkex_place_market",
  "wkex_place_limit", "wkex_cancel", "wkex_query_order", "wkex_open_orders",
  "wkex_balance", "wkex_name",
  "wkex_positions", "wkex_set_leverage", "wkex_set_margin_mode", "wkex_close_position",
  "wkex_amend_order", "wkex_place_batch", "wkex_cancel_batch", "wkex_place_oco",
  "wkex_subscribe_user_data", "wkex_keepalive_user_data", "wkex_user_data_poll",
  "wkex_ws_place_order", "wkex_ws_cancel_order",
  "wkex_place_batch_full", "wkex_ws_place_order_full"
)) {
  stopifnot(is.function(get(verb, envir = asNamespace("wickraexchange"))))
}

## ---------------------------------------------------------------------------
## Golden-fixture parity.
##
## The Rust suite (crates/wickra-exchange-core/tests/golden.rs) drives the
## committed replay tapes in golden/ through a ReplayExchange running a fixed
## SMA strategy, and pins the fill price and the resulting balances. The replay
## test above proves a tape reaches a fill; it does not check the numbers. A
## lost decimal, a dropped fee or slippage on the wrong side would still fill,
## and still pass. These assert the exact values the Rust suite pins.
##
## The fixtures are read with a small field reader rather than jsonlite: this
## package declares no dependencies at all, and taking one on so a test can read
## four numbers and one array out of a file whose shape is fixed and committed
## would be a poor trade. The reader handles that shape and nothing else, which
## is why it lives here rather than in R/.
## ---------------------------------------------------------------------------

golden_dir <- function() {
  ## Run from bindings/r under R CMD check, or from the repository root
  ## directly; search upwards rather than counting "..".
  dir <- normalizePath(".", mustWork = FALSE)
  repeat {
    candidate <- file.path(dir, "golden")
    if (dir.exists(candidate)) return(candidate)
    parent <- dirname(dir)
    if (identical(parent, dir)) stop("no golden/ directory found above ", getwd())
    dir <- parent
  }
}

golden_text <- function(kind, name) {
  path <- file.path(golden_dir(), kind, paste0(name, ".json"))
  paste(readLines(path, warn = FALSE), collapse = " ")
}

## The scalar value of "<key>": <number>.
golden_num <- function(text, key) {
  pattern <- paste0('"', key, '"[[:space:]]*:[[:space:]]*-?[0-9.]+')
  hit <- regmatches(text, regexpr(pattern, text))
  stopifnot(length(hit) == 1)
  as.numeric(sub(".*:[[:space:]]*", "", hit))
}

## The array of numbers at "<key>": [ ... ].
golden_nums <- function(text, key) {
  pattern <- paste0('"', key, '"[[:space:]]*:[[:space:]]*\\[[^]]*\\]')
  hit <- regmatches(text, regexpr(pattern, text))
  stopifnot(length(hit) == 1)
  inner <- sub(".*\\[", "", sub("\\].*", "", hit))
  as.numeric(strsplit(inner, ",")[[1]])
}

## The boolean value of "<key>": true|false.
golden_bool <- function(text, key) {
  pattern <- paste0('"', key, '"[[:space:]]*:[[:space:]]*(true|false)')
  hit <- regmatches(text, regexpr(pattern, text))
  stopifnot(length(hit) == 1)
  grepl("true", hit)
}

run_golden_case <- function(name) {
  spec <- golden_text("replay", name)
  expected <- golden_text("expected", name)

  period <- golden_num(spec, "sma_period")
  gex <- wkex_replay_trades(
    "BTC/USDT", golden_nums(spec, "tape"), c(USDT = golden_num(spec, "USDT")),
    maker_bps = golden_num(spec, "maker_bps"),
    taker_bps = golden_num(spec, "taker_bps"),
    slippage_bps = golden_num(spec, "slippage_bps")
  )

  window <- numeric(0)
  fill_price <- NA_real_
  ## Each poll advances the recording by exactly one frame; an empty batch is
  ## how an exhausted tape reports itself.
  repeat {
    batch <- wkex_poll(gex)
    if (length(batch) == 0) break
    for (event in batch) {
      if (event$kind != "trade") next
      window <- c(window, event$price)
      if (length(window) < period) next
      mean_price <- mean(window[(length(window) - period + 1):length(window)])
      if (is.na(fill_price) && event$price > mean_price) {
        order <- wkex_place_market(gex, "BTC/USDT", "buy", 1)
        fill_price <- order$average_price
      }
    }
  }

  stopifnot((!is.na(fill_price)) == golden_bool(expected, "filled"))
  stopifnot(abs(fill_price - golden_num(expected, "average_price")) < 1e-6)
  stopifnot(abs(wkex_balance(gex, "BTC") - golden_num(expected, "btc")) < 1e-6)
  stopifnot(abs(wkex_balance(gex, "USDT") - golden_num(expected, "usdt")) < 1e-6)
}

run_golden_case("sma_cross")
run_golden_case("sma_cross_with_costs")

## ---------------------------------------------------------------- place_order
##
## wkex_place_market() and wkex_place_limit() take a market, a side, a quantity
## and a price, which is all an order could ever be from R. The trigger price,
## the time-in-force, post-only, reduce-only, self-trade prevention and the
## client order id all existed in the Rust core and had no way through.

ex <- wkex_paper(c(USDT = 100000, BTC = 5), maker_bps = 1, taker_bps = 5, slippage_bps = 10)
wkex_set_price(ex, "BTC/USDT", 20000)

## A plain request places the same order the narrow call does.
order <- wkex_place_order(ex, "BTC/USDT", "buy", 1)
stopifnot(order$status == "filled")
stopifnot(abs(order$average_price - 20020) < 1e-6)

## A resting order carries the flags that decide what it is.
resting <- wkex_place_order(
  ex, "BTC/USDT", "buy", 1,
  price = 19000, time_in_force = "gtc",
  client_order_id = "retry-safe-1", post_only = TRUE, stp = "expire_maker"
)
stopifnot(resting$status == "new")

## A trigger order reaches the venue with its trigger. The paper backend refuses
## triggers, and that refusal is the proof it arrived: a request with the field
## dropped would have been placed as a plain market sell instead, at the price
## the stop existed to protect against.
stopifnot(inherits(
  try(wkex_place_order(ex, "BTC/USDT", "sell", 1, stop_price = 19000), silent = TRUE),
  "try-error"
))

## The order type is derived from which prices are set, so a caller never names
## one that contradicts the prices given.
stopifnot(wickraexchange:::.wkex_order_type(NA_real_, NA_real_) == 0L)
stopifnot(wickraexchange:::.wkex_order_type(19000, NA_real_) == 1L)
stopifnot(wickraexchange:::.wkex_order_type(NA_real_, 19000) == 2L)
stopifnot(wickraexchange:::.wkex_order_type(18900, 19000) == 3L)

## An unknown time-in-force is an error, not a silent fall back to "gtc": a
## resting order where the caller asked for one that must not rest is the defect
## the core stopped shipping, and a lenient parse here would put it back.
stopifnot(inherits(try(wickraexchange:::.wkex_tif("gtd"), silent = TRUE), "try-error"))
stopifnot(inherits(try(wickraexchange:::.wkex_stp("cancel_maker"), silent = TRUE), "try-error"))
stopifnot(wickraexchange:::.wkex_tif("IOC") == 1L)
stopifnot(wickraexchange:::.wkex_stp("EXPIRE_BOTH") == 3L)

cat("wickra.exchange R tests passed\n")
