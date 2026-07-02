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

#' The current ticker for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @return A list with `symbol`, `last`, `bid`, `ask`, `volume`.
#' @export
wkex_ticker <- function(ex, market) {
  .Call(C_wkex_exchange_ticker, ex$handle, market)
}

#' Historical candles for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @param interval Candle interval, e.g. "1m".
#' @param limit Maximum candles to return.
#' @return A list of OHLCV lists (`open`/`high`/`low`/`close`/`volume`/`timestamp`).
#' @export
wkex_klines <- function(ex, market, interval, limit) {
  .Call(C_wkex_exchange_klines, ex$handle, market, interval, as.integer(limit))
}

#' Order-book depth snapshot for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @param depth Maximum levels per side.
#' @return A list with `symbol` and `bids`/`asks` lists of `{price, quantity}`.
#' @export
wkex_order_book <- function(ex, market, depth) {
  .Call(C_wkex_exchange_order_book, ex$handle, market, as.integer(depth))
}

#' Subscribe to the public trade stream for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @return Invisibly, `ex`.
#' @export
wkex_subscribe_trades <- function(ex, market) {
  invisible(.Call(C_wkex_exchange_subscribe_trades, ex$handle, market))
}

#' Subscribe to the order-book stream for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @return Invisibly, `ex`.
#' @export
wkex_subscribe_book <- function(ex, market) {
  invisible(.Call(C_wkex_exchange_subscribe_book, ex$handle, market))
}

#' Subscribe to the ticker stream for a market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @return Invisibly, `ex`.
#' @export
wkex_subscribe_ticker <- function(ex, market) {
  invisible(.Call(C_wkex_exchange_subscribe_ticker, ex$handle, market))
}

#' Look up a single order by venue id.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @param order_id Venue order id.
#' @return The order as a list.
#' @export
wkex_query_order <- function(ex, market, order_id) {
  .wkex_order(.Call(C_wkex_exchange_query_order, ex$handle, market, order_id))
}

#' Open orders, optionally filtered to one market.
#' @param ex A `wickra_exchange` object.
#' @param market Market symbol, or `NULL` for all markets.
#' @return A list of order lists.
#' @export
wkex_open_orders <- function(ex, market = NULL) {
  lapply(.Call(C_wkex_exchange_open_orders, ex$handle, market), .wkex_order)
}

.wkex_position <- function(raw) {
  raw$side <- if (raw$side == 1L) "short" else "long"
  raw$margin_mode <- if (raw$margin_mode == 1L) "isolated" else "cross"
  raw
}

.wkex_margin_code <- function(mode) {
  if (identical(mode, "isolated") || identical(mode, 1L) || identical(mode, 1)) {
    return(1L)
  }
  if (identical(mode, "cross") || identical(mode, 0L) || identical(mode, 0)) {
    return(0L)
  }
  stop("margin mode must be 'cross' or 'isolated'")
}

#' Connect a live derivatives (USD-M futures) client.
#'
#' Positions, leverage, margin mode and reduce-only close. Fails for a spot-only
#' venue (coinbase, upbit).
#' @param name,api_key,api_secret Venue and API credentials.
#' @param passphrase,private_key Optional extra credentials (NULL if unused).
#' @param testnet Use the venue testnet.
#' @return A `wickra_derivatives` object.
#' @export
wkex_derivatives <- function(name, api_key, api_secret,
                             passphrase = NULL, private_key = NULL, testnet = FALSE) {
  handle <- .Call(C_wkex_connect_derivatives, name, api_key, api_secret,
                  passphrase, private_key, as.logical(testnet))
  structure(list(handle = handle), class = "wickra_derivatives")
}

#' The open position in a market.
#' @param deriv A `wickra_derivatives` object.
#' @param market Market symbol, e.g. "BTC/USDT".
#' @return A position list (errors if flat).
#' @export
wkex_position <- function(deriv, market) {
  .wkex_position(.Call(C_wkex_derivatives_position, deriv$handle, market))
}

#' Every open position (list-all).
#'
#' Pass a `market` to scope to one symbol, or `NULL` for all.
#' @param deriv A `wickra_derivatives` object.
#' @param market Optional market symbol, or NULL for all positions.
#' @return A list of position lists.
#' @export
wkex_positions <- function(deriv, market = NULL) {
  lapply(.Call(C_wkex_derivatives_positions, deriv$handle, market), .wkex_position)
}

#' Set the leverage for a market.
#' @param deriv A `wickra_derivatives` object.
#' @param market Market symbol.
#' @param leverage Integer leverage.
#' @export
wkex_set_leverage <- function(deriv, market, leverage) {
  invisible(.Call(C_wkex_derivatives_set_leverage, deriv$handle, market, as.integer(leverage)))
}

#' Set the margin mode ("cross" or "isolated") for a market.
#' @param deriv A `wickra_derivatives` object.
#' @param market Market symbol.
#' @param mode "cross" or "isolated".
#' @export
wkex_set_margin_mode <- function(deriv, market, mode) {
  invisible(.Call(C_wkex_derivatives_set_margin_mode, deriv$handle, market, .wkex_margin_code(mode)))
}

#' Flatten the open position in a market with a reduce-only market order.
#' @param deriv A `wickra_derivatives` object.
#' @param market Market symbol.
#' @return The resulting order list.
#' @export
wkex_close_position <- function(deriv, market) {
  .wkex_order(.Call(C_wkex_derivatives_close_position, deriv$handle, market))
}

#' Connect a live advanced-orders client (amend, batch cancel).
#'
#' Fails for a venue without an advanced-order surface (coinbase, upbit).
#' @param name,api_key,api_secret Venue and API credentials.
#' @param passphrase,private_key Optional extra credentials (NULL if unused).
#' @param testnet Use the venue testnet.
#' @param futures Select the USD-M futures market.
#' @return A `wickra_advanced` object.
#' @export
wkex_advanced <- function(name, api_key, api_secret,
                          passphrase = NULL, private_key = NULL, testnet = FALSE, futures = FALSE) {
  handle <- .Call(C_wkex_connect_advanced, name, api_key, api_secret,
                  passphrase, private_key, as.logical(testnet), as.logical(futures))
  structure(list(handle = handle), class = "wickra_advanced")
}

#' Amend a resting order's price and/or quantity in place.
#'
#' Pass `NA` for `new_price` or `new_quantity` to leave that field unchanged.
#' @param adv A `wickra_advanced` object.
#' @param market Market symbol.
#' @param order_id Venue order id.
#' @param new_price,new_quantity New values, or NA to leave unchanged.
#' @return The refreshed order list.
#' @export
wkex_amend_order <- function(adv, market, order_id, new_price = NA_real_, new_quantity = NA_real_) {
  .wkex_order(.Call(C_wkex_advanced_amend_order, adv$handle, market, order_id,
                    as.numeric(new_price), as.numeric(new_quantity)))
}

#' Cancel several orders on a market in one request.
#' @param adv A `wickra_advanced` object.
#' @param market Market symbol.
#' @param order_ids Character vector of venue order ids.
#' @export
wkex_cancel_batch <- function(adv, market, order_ids) {
  invisible(.Call(C_wkex_advanced_cancel_batch, adv$handle, market, as.character(order_ids)))
}

#' Place a one-cancels-other bracket.
#'
#' A take-profit limit leg at `price` paired with a stop leg triggered at
#' `stop_price`. A finite `stop_limit_price` makes the stop leg a stop-limit;
#' `NA` leaves it a stop-market.
#' @param adv A `wickra_advanced` object.
#' @param market Market symbol.
#' @param side "buy" or "sell".
#' @param quantity Order quantity.
#' @param price Take-profit limit price.
#' @param stop_price Stop trigger price.
#' @param stop_limit_price Stop-leg limit price, or NA for a stop-market.
#' @return A list of the resulting order legs.
#' @export
wkex_place_oco <- function(adv, market, side, quantity, price, stop_price, stop_limit_price = NA_real_) {
  legs <- .Call(C_wkex_advanced_place_oco, adv$handle, market, .wkex_side(side),
                as.numeric(quantity), as.numeric(price), as.numeric(stop_price),
                as.numeric(stop_limit_price))
  lapply(legs, .wkex_order)
}

#' Place several orders in one request.
#'
#' The orders are described by parallel vectors. `prices` uses `NA` for a market
#' order and a finite value for a limit order.
#' @param adv A `wickra_advanced` object.
#' @param markets Character vector of market symbols.
#' @param sides Character or integer vector of sides ("buy"/"sell").
#' @param quantities Numeric vector of quantities.
#' @param prices Numeric vector of prices (NA for a market order).
#' @return A list of results, each `list(order = , error = )`: `order` on success
#'   or `error` (an integer status code) on a per-order rejection.
#' @export
wkex_place_batch <- function(adv, markets, sides, quantities, prices) {
  sides_int <- vapply(sides, .wkex_side, integer(1), USE.NAMES = FALSE)
  results <- .Call(C_wkex_advanced_place_batch, adv$handle, as.character(markets),
                   as.integer(sides_int), as.numeric(quantities), as.numeric(prices))
  lapply(results, function(r) {
    if (!is.null(r$order)) {
      r$order <- .wkex_order(r$order)
    }
    r
  })
}

#' Connect a live private user-data client.
#'
#' After [wkex_subscribe_user_data()], [wkex_user_data_poll()] surfaces the
#' account's own order and balance updates. Fails for a spot-only venue.
#' @param name,api_key,api_secret Venue and API credentials.
#' @param passphrase,private_key Optional extra credentials (NULL if unused).
#' @param testnet Use the venue testnet.
#' @param futures Select the USD-M futures market.
#' @return A `wickra_user_data` object.
#' @export
wkex_user_data <- function(name, api_key, api_secret,
                           passphrase = NULL, private_key = NULL, testnet = FALSE, futures = FALSE) {
  handle <- .Call(C_wkex_connect_user_data, name, api_key, api_secret,
                  passphrase, private_key, as.logical(testnet), as.logical(futures))
  structure(list(handle = handle), class = "wickra_user_data")
}

#' Open the private user-data stream.
#' @param ud A `wickra_user_data` object.
#' @return Invisibly, `ud`.
#' @export
wkex_subscribe_user_data <- function(ud) {
  invisible(.Call(C_wkex_user_data_subscribe, ud$handle))
}

#' Keep the private user-data stream alive.
#'
#' Refreshes the venue session / sends a heartbeat so the stream is not dropped
#' for inactivity; call it periodically. A dropped stream is also recovered
#' automatically on the next [wkex_user_data_poll()]. A no-op before
#' [wkex_subscribe_user_data()].
#' @param ud A `wickra_user_data` object.
#' @return Invisibly, `ud`.
#' @export
wkex_keepalive_user_data <- function(ud) {
  invisible(.Call(C_wkex_user_data_keepalive, ud$handle))
}

#' Drain buffered user-data events.
#' @param ud A `wickra_user_data` object.
#' @param capacity Maximum events to return per call.
#' @return A list of event lists.
#' @export
wkex_user_data_poll <- function(ud, capacity = 16L) {
  lapply(.Call(C_wkex_user_data_poll, ud$handle, as.integer(capacity)), .wkex_event)
}

#' Connect a live WebSocket order-API client (place/cancel over the ws-api).
#'
#' Native on binance/bybit/okx/gateio/kraken; on bitget/kucoin/htx the methods
#' error (no WebSocket order-entry API). Fails for a spot-only venue.
#' @param name,api_key,api_secret Venue and API credentials.
#' @param passphrase,private_key Optional extra credentials (NULL if unused).
#' @param testnet Use the venue testnet.
#' @param futures Select the USD-M futures market.
#' @return A `wickra_ws_execution` object.
#' @export
wkex_ws_execution <- function(name, api_key, api_secret,
                              passphrase = NULL, private_key = NULL, testnet = FALSE, futures = FALSE) {
  handle <- .Call(C_wkex_connect_ws_execution, name, api_key, api_secret,
                  passphrase, private_key, as.logical(testnet), as.logical(futures))
  structure(list(handle = handle), class = "wickra_ws_execution")
}

#' Place an order over the WebSocket order API.
#' @param wse A `wickra_ws_execution` object.
#' @param market Market string.
#' @param side "buy" or "sell".
#' @param quantity Order quantity.
#' @param price Limit price, or NA for a market order.
#' @return The resulting order as a list.
#' @export
wkex_ws_place_order <- function(wse, market, side, quantity, price = NA_real_) {
  .wkex_order(.Call(C_wkex_ws_place_order, wse$handle, market, .wkex_side(side),
                    as.numeric(quantity), as.numeric(price)))
}

#' Cancel an order over the WebSocket order API by venue id.
#' @param wse A `wickra_ws_execution` object.
#' @param market Market string.
#' @param order_id The venue order id.
#' @return Invisibly, `wse`.
#' @export
wkex_ws_cancel_order <- function(wse, market, order_id) {
  .Call(C_wkex_ws_cancel_order, wse$handle, market, order_id)
  invisible(wse)
}
