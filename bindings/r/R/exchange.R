## Idiomatic R interface to the wickra-exchange C ABI hub.

.wkex_side <- function(side) {
  if (identical(side, "buy") || identical(side, 0L) || identical(side, 0)) {
    return(0L)
  }
  if (identical(side, "sell") || identical(side, 1L) || identical(side, 1)) {
    return(1L)
  }
  stop("side must be 'buy' or 'sell'")
}

.wkex_status <- c("new", "partially_filled", "filled", "canceled", "rejected", "expired")
.wkex_kind <- c("trade", "ticker", "order_update", "balance_update", "subscribed", "other")

.wkex_order <- function(raw) {
  raw$side <- if (raw$side == 1L) "sell" else "buy"
  raw$status <- .wkex_status[raw$status + 1L]
  raw
}

.wkex_event <- function(raw) {
  raw$kind <- .wkex_kind[raw$kind + 1L]
  if (raw$side >= 0L) {
    raw$side <- if (raw$side == 1L) "sell" else "buy"
  } else {
    raw$side <- NA_character_
  }
  if (!is.null(raw$order)) {
    raw$order <- .wkex_order(raw$order)
  }
  raw
}

#' The wickra-exchange library version.
#' @return A version string.
#' @export
wkex_version <- function() {
  .Call(C_wkex_version)
}

#' Open an offline paper account.
#' @param balances Named numeric vector of starting balances (asset -> amount).
#' @param maker_bps,taker_bps,slippage_bps Costs in basis points.
#' @return A `wickra_exchange` object.
#' @export
wkex_paper <- function(balances, maker_bps = 0, taker_bps = 0, slippage_bps = 0) {
  handle <- .Call(
    C_wkex_paper_new, names(balances), as.numeric(balances),
    maker_bps, taker_bps, slippage_bps
  )
  structure(list(handle = handle), class = "wickra_exchange")
}

#' Open a replay account driven by a recorded tape of trades.
#' @param market Market string, e.g. "BTC/USDT".
#' @param tape Numeric vector of trade prices.
#' @param balances Named numeric vector of starting balances.
#' @param maker_bps,taker_bps,slippage_bps Costs in basis points.
#' @return A `wickra_exchange` object.
#' @export
wkex_replay_trades <- function(market, tape, balances, maker_bps = 0, taker_bps = 0, slippage_bps = 0) {
  handle <- .Call(
    C_wkex_replay_new, market, as.numeric(tape), names(balances), as.numeric(balances),
    maker_bps, taker_bps, slippage_bps
  )
  structure(list(handle = handle), class = "wickra_exchange")
}

#' The venue identifier of an exchange.
#' @param ex A `wickra_exchange` object.
#' @return The venue name ("paper", "replay", "binance", ...).
#' @export
wkex_name <- function(ex) {
  .Call(C_wkex_name, ex$handle)
}

#' Set the mark price a paper account fills against (paper backend only).
#' @param ex A `wickra_exchange` object.
#' @param market Market string.
#' @param price Mark price.
#' @return Invisibly, `ex`.
#' @export
wkex_set_price <- function(ex, market, price) {
  code <- .Call(C_wkex_set_price, ex$handle, market, as.numeric(price))
  if (code != 0L) {
    stop(sprintf("wickra: set_price failed with code %d", code))
  }
  invisible(ex)
}

#' Place a market order.
#' @param ex A `wickra_exchange` object.
#' @param market Market string.
#' @param side "buy" or "sell".
#' @param quantity Order quantity.
#' @return The resulting order as a list.
#' @export
wkex_place_market <- function(ex, market, side, quantity) {
  .wkex_order(.Call(C_wkex_place, ex$handle, market, .wkex_side(side), as.numeric(quantity), NA_real_))
}

#' Place a limit order.
#' @param ex A `wickra_exchange` object.
#' @param market Market string.
#' @param side "buy" or "sell".
#' @param quantity Order quantity.
#' @param price Limit price.
#' @return The resulting order as a list.
#' @export
wkex_place_limit <- function(ex, market, side, quantity, price) {
  .wkex_order(.Call(C_wkex_place, ex$handle, market, .wkex_side(side), as.numeric(quantity), as.numeric(price)))
}

#' Cancel an open order by venue id.
#' @param ex A `wickra_exchange` object.
#' @param market Market string.
#' @param order_id The venue order id.
#' @return Invisibly, `ex`.
#' @export
wkex_cancel <- function(ex, market, order_id) {
  .Call(C_wkex_cancel, ex$handle, market, order_id)
  invisible(ex)
}

#' The free balance of an asset.
#' @param ex A `wickra_exchange` object.
#' @param asset Asset symbol, e.g. "BTC".
#' @return The free balance as a number.
#' @export
wkex_balance <- function(ex, asset) {
  .Call(C_wkex_balance, ex$handle, asset)
}

#' Drain buffered events.
#' @param ex A `wickra_exchange` object.
#' @param capacity Maximum events to return per call.
#' @return A list of event lists.
#' @export
wkex_poll <- function(ex, capacity = 16L) {
  lapply(.Call(C_wkex_poll, ex$handle, as.integer(capacity)), .wkex_event)
}
