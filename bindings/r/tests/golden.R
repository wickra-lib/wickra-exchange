## Golden-fixture parity for the wickra-exchange R binding.
##
## Lives outside run_tests.R and outside the built tarball (see .Rbuildignore).
## The corpus is at the repository root, above this package, so these cases only
## resolve when run from there -- which CI does explicitly. `R CMD check` runs
## everything under tests/ from the built tarball, where the corpus does not
## exist, and that is what r-universe runs: shipping this file failed every one
## of its thirteen platform builds with
##
##     Error in golden_dir() : no golden/ directory found above ...
##
## The other assertions in run_tests.R build their own data and travel fine.

library(wickraexchange)

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

cat("wickra.exchange R golden-fixture parity passed
")
