/* R .Call glue for the wickra-exchange C ABI hub. */
#include <R.h>
#include <Rinternals.h>
#include <R_ext/Rdynload.h>
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include "wickra_exchange.h"

/* --- handle lifetime ----------------------------------------------------- */

static void wkex_finalize(SEXP ext) {
    WickraExchange *h = (WickraExchange *)R_ExternalPtrAddr(ext);
    if (h) {
        wickra_exchange_free(h);
    }
    R_ClearExternalPtr(ext);
}

static SEXP wrap_handle(WickraExchange *h, const char *what) {
    if (!h) {
        Rf_error("wickra: failed to construct %s exchange", what);
    }
    SEXP ext = PROTECT(R_MakeExternalPtr(h, R_NilValue, R_NilValue));
    R_RegisterCFinalizerEx(ext, wkex_finalize, TRUE);
    UNPROTECT(1);
    return ext;
}

static WickraExchange *handle_of(SEXP ext) {
    WickraExchange *h = (WickraExchange *)R_ExternalPtrAddr(ext);
    if (!h) {
        Rf_error("wickra: exchange handle is closed");
    }
    return h;
}

/* Build a C array of `const char*` from an R character vector, using R_alloc so
 * it lives until the enclosing .Call returns. */
static const char **string_vec(SEXP x, R_xlen_t n) {
    const char **out = (const char **)R_alloc(n, sizeof(char *));
    for (R_xlen_t i = 0; i < n; i++) {
        out[i] = CHAR(STRING_ELT(x, i));
    }
    return out;
}

/* An optional C string: NULL for an R NULL, NA, empty vector or empty string. */
static const char *opt_cstr(SEXP x) {
    if (x == R_NilValue || Rf_length(x) == 0 || STRING_ELT(x, 0) == NA_STRING) {
        return NULL;
    }
    const char *s = CHAR(STRING_ELT(x, 0));
    return s[0] == '\0' ? NULL : s;
}

/* --- result builders ----------------------------------------------------- */

static SEXP nan_to_na(double value) {
    return Rf_ScalarReal(value != value ? NA_REAL : value);
}

static SEXP order_to_list(const WickraOrder *order) {
    const char *names[] = {"id", "side", "status", "quantity",
                           "filled_quantity", "price", "average_price", ""};
    SEXP out = PROTECT(Rf_mkNamed(VECSXP, names));
    SET_VECTOR_ELT(out, 0, Rf_mkString(order->id));
    SET_VECTOR_ELT(out, 1, Rf_ScalarInteger(order->side));
    SET_VECTOR_ELT(out, 2, Rf_ScalarInteger(order->status));
    SET_VECTOR_ELT(out, 3, Rf_ScalarReal(order->quantity));
    SET_VECTOR_ELT(out, 4, Rf_ScalarReal(order->filled_quantity));
    SET_VECTOR_ELT(out, 5, nan_to_na(order->price));
    SET_VECTOR_ELT(out, 6, nan_to_na(order->average_price));
    UNPROTECT(1);
    return out;
}

static SEXP position_to_list(const WickraPosition *p) {
    const char *names[] = {"symbol", "side", "quantity", "entry_price",
                           "mark_price", "leverage", "unrealized_pnl", "margin_mode", ""};
    SEXP out = PROTECT(Rf_mkNamed(VECSXP, names));
    SET_VECTOR_ELT(out, 0, Rf_mkString(p->symbol));
    SET_VECTOR_ELT(out, 1, Rf_ScalarInteger(p->side));
    SET_VECTOR_ELT(out, 2, Rf_ScalarReal(p->quantity));
    SET_VECTOR_ELT(out, 3, Rf_ScalarReal(p->entry_price));
    SET_VECTOR_ELT(out, 4, Rf_ScalarReal(p->mark_price));
    SET_VECTOR_ELT(out, 5, Rf_ScalarReal(p->leverage));
    SET_VECTOR_ELT(out, 6, Rf_ScalarReal(p->unrealized_pnl));
    SET_VECTOR_ELT(out, 7, Rf_ScalarInteger(p->margin_mode));
    UNPROTECT(1);
    return out;
}

static SEXP event_to_list(const WickraEvent *event) {
    const char *names[] = {"kind", "symbol", "price", "quantity", "side", "order", ""};
    SEXP out = PROTECT(Rf_mkNamed(VECSXP, names));
    SET_VECTOR_ELT(out, 0, Rf_ScalarInteger(event->kind));
    SET_VECTOR_ELT(out, 1, Rf_mkString(event->symbol));
    SET_VECTOR_ELT(out, 2, nan_to_na(event->price));
    SET_VECTOR_ELT(out, 3, nan_to_na(event->quantity));
    SET_VECTOR_ELT(out, 4, Rf_ScalarInteger(event->side));
    SET_VECTOR_ELT(out, 5, event->kind == WICKRA_EVENT_ORDER_UPDATE
                               ? order_to_list(&event->order)
                               : R_NilValue);
    UNPROTECT(1);
    return out;
}

/* --- exports ------------------------------------------------------------- */

SEXP wkex_version(void) {
    return Rf_mkString(wickra_version());
}

SEXP wkex_paper_new(SEXP assets, SEXP amounts, SEXP maker, SEXP taker, SEXP slippage) {
    R_xlen_t n = Rf_xlength(assets);
    const char **c_assets = string_vec(assets, n);
    WickraExchange *h = wickra_paper_new(
        c_assets, REAL(amounts), (size_t)n,
        Rf_asReal(maker), Rf_asReal(taker), Rf_asReal(slippage));
    return wrap_handle(h, "paper");
}

SEXP wkex_replay_new(SEXP market, SEXP tape, SEXP assets, SEXP amounts,
                     SEXP maker, SEXP taker, SEXP slippage) {
    R_xlen_t n = Rf_xlength(assets);
    R_xlen_t n_tape = Rf_xlength(tape);
    const char **c_assets = string_vec(assets, n);
    WickraExchange *h = wickra_replay_new(
        CHAR(STRING_ELT(market, 0)), REAL(tape), (size_t)n_tape,
        c_assets, REAL(amounts), (size_t)n,
        Rf_asReal(maker), Rf_asReal(taker), Rf_asReal(slippage));
    return wrap_handle(h, "replay");
}

SEXP wkex_name(SEXP ext) {
    char buf[32];
    wickra_exchange_name(handle_of(ext), buf, sizeof(buf));
    return Rf_mkString(buf);
}

SEXP wkex_set_price(SEXP ext, SEXP market, SEXP price) {
    int rc = wickra_exchange_set_price(handle_of(ext), CHAR(STRING_ELT(market, 0)), Rf_asReal(price));
    return Rf_ScalarInteger(rc);
}

SEXP wkex_place(SEXP ext, SEXP market, SEXP side, SEXP quantity, SEXP price) {
    WickraOrder order;
    const char *m = CHAR(STRING_ELT(market, 0));
    int s = Rf_asInteger(side);
    double qty = Rf_asReal(quantity);
    double p = Rf_asReal(price);
    int rc = ISNA(p)
        ? wickra_exchange_place_market(handle_of(ext), m, s, qty, &order)
        : wickra_exchange_place_limit(handle_of(ext), m, s, qty, p, &order);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: order failed with code %d", rc);
    }
    return order_to_list(&order);
}

SEXP wkex_cancel(SEXP ext, SEXP market, SEXP order_id) {
    int rc = wickra_exchange_cancel(handle_of(ext),
                                    CHAR(STRING_ELT(market, 0)),
                                    CHAR(STRING_ELT(order_id, 0)));
    return Rf_ScalarInteger(rc);
}

SEXP wkex_balance(SEXP ext, SEXP asset) {
    double free_amount = 0.0;
    int rc = wickra_exchange_balance(handle_of(ext), CHAR(STRING_ELT(asset, 0)), &free_amount);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: balance failed with code %d", rc);
    }
    return Rf_ScalarReal(free_amount);
}

SEXP wkex_poll(SEXP ext, SEXP capacity) {
    int cap = Rf_asInteger(capacity);
    WickraEvent *buf = (WickraEvent *)R_alloc(cap, sizeof(WickraEvent));
    int count = wickra_exchange_poll(handle_of(ext), buf, (size_t)cap);
    if (count < 0) {
        Rf_error("wickra: poll failed with code %d", count);
    }
    SEXP out = PROTECT(Rf_allocVector(VECSXP, count));
    for (int i = 0; i < count; i++) {
        SET_VECTOR_ELT(out, i, event_to_list(&buf[i]));
    }
    UNPROTECT(1);
    return out;
}

/* --- derivatives --------------------------------------------------------- */

static void wkex_deriv_finalize(SEXP ext) {
    WickraDerivatives *h = (WickraDerivatives *)R_ExternalPtrAddr(ext);
    if (h) {
        wickra_derivatives_free(h);
    }
    R_ClearExternalPtr(ext);
}

static WickraDerivatives *deriv_of(SEXP ext) {
    WickraDerivatives *h = (WickraDerivatives *)R_ExternalPtrAddr(ext);
    if (!h) {
        Rf_error("wickra: derivatives handle is closed");
    }
    return h;
}

SEXP wkex_connect_derivatives(SEXP name, SEXP api_key, SEXP api_secret,
                              SEXP passphrase, SEXP private_key, SEXP testnet) {
    WickraDerivatives *h = wickra_connect_derivatives(
        CHAR(STRING_ELT(name, 0)), CHAR(STRING_ELT(api_key, 0)), CHAR(STRING_ELT(api_secret, 0)),
        opt_cstr(passphrase), opt_cstr(private_key), (bool)Rf_asLogical(testnet));
    if (!h) {
        Rf_error("wickra: failed to connect derivatives client (spot-only or unknown venue?)");
    }
    SEXP ext = PROTECT(R_MakeExternalPtr(h, R_NilValue, R_NilValue));
    R_RegisterCFinalizerEx(ext, wkex_deriv_finalize, TRUE);
    UNPROTECT(1);
    return ext;
}

SEXP wkex_derivatives_position(SEXP ext, SEXP market) {
    WickraPosition pos;
    int rc = wickra_derivatives_position(deriv_of(ext), CHAR(STRING_ELT(market, 0)), &pos);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: position failed with code %d", rc);
    }
    return position_to_list(&pos);
}

SEXP wkex_derivatives_positions(SEXP ext, SEXP market) {
    const char *m = opt_cstr(market);
    int cap = 16;
    for (;;) {
        WickraPosition *buf = (WickraPosition *)R_alloc(cap, sizeof(WickraPosition));
        int count = wickra_derivatives_positions(deriv_of(ext), m, buf, (size_t)cap);
        if (count < 0) {
            Rf_error("wickra: positions failed with code %d", count);
        }
        if (count > cap) {
            cap = count;
            continue;
        }
        SEXP out = PROTECT(Rf_allocVector(VECSXP, count));
        for (int i = 0; i < count; i++) {
            SET_VECTOR_ELT(out, i, position_to_list(&buf[i]));
        }
        UNPROTECT(1);
        return out;
    }
}

SEXP wkex_derivatives_set_leverage(SEXP ext, SEXP market, SEXP leverage) {
    int rc = wickra_derivatives_set_leverage(deriv_of(ext), CHAR(STRING_ELT(market, 0)),
                                             (uint32_t)Rf_asInteger(leverage));
    return Rf_ScalarInteger(rc);
}

SEXP wkex_derivatives_set_margin_mode(SEXP ext, SEXP market, SEXP mode) {
    int rc = wickra_derivatives_set_margin_mode(deriv_of(ext), CHAR(STRING_ELT(market, 0)),
                                                Rf_asInteger(mode));
    return Rf_ScalarInteger(rc);
}

SEXP wkex_derivatives_close_position(SEXP ext, SEXP market) {
    WickraOrder order;
    int rc = wickra_derivatives_close_position(deriv_of(ext), CHAR(STRING_ELT(market, 0)), &order);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: close_position failed with code %d", rc);
    }
    return order_to_list(&order);
}

/* --- advanced orders ----------------------------------------------------- */

static void wkex_adv_finalize(SEXP ext) {
    WickraAdvanced *h = (WickraAdvanced *)R_ExternalPtrAddr(ext);
    if (h) {
        wickra_advanced_free(h);
    }
    R_ClearExternalPtr(ext);
}

static WickraAdvanced *adv_of(SEXP ext) {
    WickraAdvanced *h = (WickraAdvanced *)R_ExternalPtrAddr(ext);
    if (!h) {
        Rf_error("wickra: advanced-orders handle is closed");
    }
    return h;
}

SEXP wkex_connect_advanced(SEXP name, SEXP api_key, SEXP api_secret,
                           SEXP passphrase, SEXP private_key, SEXP testnet, SEXP futures) {
    WickraAdvanced *h = wickra_connect_advanced(
        CHAR(STRING_ELT(name, 0)), CHAR(STRING_ELT(api_key, 0)), CHAR(STRING_ELT(api_secret, 0)),
        opt_cstr(passphrase), opt_cstr(private_key),
        (bool)Rf_asLogical(testnet), (bool)Rf_asLogical(futures));
    if (!h) {
        Rf_error("wickra: failed to connect advanced-orders client (spot-only or unknown venue?)");
    }
    SEXP ext = PROTECT(R_MakeExternalPtr(h, R_NilValue, R_NilValue));
    R_RegisterCFinalizerEx(ext, wkex_adv_finalize, TRUE);
    UNPROTECT(1);
    return ext;
}

SEXP wkex_advanced_amend_order(SEXP ext, SEXP market, SEXP order_id,
                               SEXP new_price, SEXP new_quantity) {
    WickraOrder order;
    int rc = wickra_advanced_amend_order(adv_of(ext), CHAR(STRING_ELT(market, 0)),
                                         CHAR(STRING_ELT(order_id, 0)),
                                         Rf_asReal(new_price), Rf_asReal(new_quantity), &order);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: amend failed with code %d", rc);
    }
    return order_to_list(&order);
}

SEXP wkex_advanced_cancel_batch(SEXP ext, SEXP market, SEXP order_ids) {
    R_xlen_t n = Rf_xlength(order_ids);
    const char **ids = string_vec(order_ids, n);
    int rc = wickra_advanced_cancel_batch(adv_of(ext), CHAR(STRING_ELT(market, 0)),
                                          ids, (size_t)n);
    return Rf_ScalarInteger(rc);
}

SEXP wkex_advanced_place_oco(SEXP ext, SEXP market, SEXP side, SEXP quantity,
                             SEXP price, SEXP stop_price, SEXP stop_limit_price) {
    const char *m = CHAR(STRING_ELT(market, 0));
    int s = Rf_asInteger(side);
    double qty = Rf_asReal(quantity);
    double p = Rf_asReal(price);
    double sp = Rf_asReal(stop_price);
    /* NA_real_ is a NaN, so it reaches the C ABI as "no stop-limit" (stop-market). */
    double slp = Rf_asReal(stop_limit_price);
    int cap = 4;
    for (;;) {
        WickraOrder *buf = (WickraOrder *)R_alloc(cap, sizeof(WickraOrder));
        int count = wickra_advanced_place_oco(adv_of(ext), m, s, qty, p, sp, slp, buf, (size_t)cap);
        if (count < 0) {
            Rf_error("wickra: place_oco failed with code %d", count);
        }
        if (count > cap) {
            cap = count;
            continue;
        }
        SEXP out = PROTECT(Rf_allocVector(VECSXP, count));
        for (int i = 0; i < count; i++) {
            SET_VECTOR_ELT(out, i, order_to_list(&buf[i]));
        }
        UNPROTECT(1);
        return out;
    }
}

SEXP wkex_advanced_place_batch(SEXP ext, SEXP markets, SEXP sides,
                               SEXP quantities, SEXP prices) {
    R_xlen_t n = Rf_xlength(markets);
    const char **c_markets = string_vec(markets, n);
    int *c_sides = INTEGER(sides);
    double *c_qty = REAL(quantities);
    double *c_prices = REAL(prices);
    WickraOrder *out = (WickraOrder *)R_alloc(n, sizeof(WickraOrder));
    int *codes = (int *)R_alloc(n, sizeof(int));
    int count = wickra_advanced_place_batch(adv_of(ext), c_markets, c_sides, c_qty, c_prices,
                                            (size_t)n, out, codes, (size_t)n);
    if (count < 0) {
        Rf_error("wickra: place_batch failed with code %d", count);
    }
    const char *rnames[] = {"order", "error", ""};
    SEXP result = PROTECT(Rf_allocVector(VECSXP, count));
    for (int i = 0; i < count; i++) {
        SEXP entry = PROTECT(Rf_mkNamed(VECSXP, rnames));
        if (codes[i] == WICKRA_OK) {
            SET_VECTOR_ELT(entry, 0, order_to_list(&out[i]));
            SET_VECTOR_ELT(entry, 1, R_NilValue);
        } else {
            SET_VECTOR_ELT(entry, 0, R_NilValue);
            SET_VECTOR_ELT(entry, 1, Rf_ScalarInteger(codes[i]));
        }
        SET_VECTOR_ELT(result, i, entry);
        UNPROTECT(1);
    }
    UNPROTECT(1);
    return result;
}

/* --- user data ----------------------------------------------------------- */

static void wkex_user_data_finalize(SEXP ext) {
    WickraUserData *h = (WickraUserData *)R_ExternalPtrAddr(ext);
    if (h) {
        wickra_user_data_free(h);
    }
    R_ClearExternalPtr(ext);
}

static WickraUserData *user_data_of(SEXP ext) {
    WickraUserData *h = (WickraUserData *)R_ExternalPtrAddr(ext);
    if (!h) {
        Rf_error("wickra: user-data handle is closed");
    }
    return h;
}

SEXP wkex_connect_user_data(SEXP name, SEXP api_key, SEXP api_secret,
                            SEXP passphrase, SEXP private_key, SEXP testnet, SEXP futures) {
    WickraUserData *h = wickra_connect_user_data(
        CHAR(STRING_ELT(name, 0)), CHAR(STRING_ELT(api_key, 0)), CHAR(STRING_ELT(api_secret, 0)),
        opt_cstr(passphrase), opt_cstr(private_key),
        (bool)Rf_asLogical(testnet), (bool)Rf_asLogical(futures));
    if (!h) {
        Rf_error("wickra: failed to connect user-data client (spot-only or unknown venue?)");
    }
    SEXP ext = PROTECT(R_MakeExternalPtr(h, R_NilValue, R_NilValue));
    R_RegisterCFinalizerEx(ext, wkex_user_data_finalize, TRUE);
    UNPROTECT(1);
    return ext;
}

SEXP wkex_user_data_subscribe(SEXP ext) {
    return Rf_ScalarInteger(wickra_user_data_subscribe(user_data_of(ext)));
}

SEXP wkex_user_data_keepalive(SEXP ext) {
    return Rf_ScalarInteger(wickra_user_data_keepalive(user_data_of(ext)));
}

SEXP wkex_user_data_poll(SEXP ext, SEXP capacity) {
    int cap = Rf_asInteger(capacity);
    WickraEvent *buf = (WickraEvent *)R_alloc(cap, sizeof(WickraEvent));
    int count = wickra_user_data_poll(user_data_of(ext), buf, (size_t)cap);
    if (count < 0) {
        Rf_error("wickra: user-data poll failed with code %d", count);
    }
    SEXP out = PROTECT(Rf_allocVector(VECSXP, count));
    for (int i = 0; i < count; i++) {
        SET_VECTOR_ELT(out, i, event_to_list(&buf[i]));
    }
    UNPROTECT(1);
    return out;
}

/* --- ws execution -------------------------------------------------------- */

static void wkex_ws_execution_finalize(SEXP ext) {
    WickraWsExecution *h = (WickraWsExecution *)R_ExternalPtrAddr(ext);
    if (h) {
        wickra_ws_execution_free(h);
    }
    R_ClearExternalPtr(ext);
}

static WickraWsExecution *ws_execution_of(SEXP ext) {
    WickraWsExecution *h = (WickraWsExecution *)R_ExternalPtrAddr(ext);
    if (!h) {
        Rf_error("wickra: ws-execution handle is closed");
    }
    return h;
}

SEXP wkex_connect_ws_execution(SEXP name, SEXP api_key, SEXP api_secret,
                               SEXP passphrase, SEXP private_key, SEXP testnet, SEXP futures) {
    WickraWsExecution *h = wickra_connect_ws_execution(
        CHAR(STRING_ELT(name, 0)), CHAR(STRING_ELT(api_key, 0)), CHAR(STRING_ELT(api_secret, 0)),
        opt_cstr(passphrase), opt_cstr(private_key),
        (bool)Rf_asLogical(testnet), (bool)Rf_asLogical(futures));
    if (!h) {
        Rf_error("wickra: failed to connect ws-execution client (spot-only or unknown venue?)");
    }
    SEXP ext = PROTECT(R_MakeExternalPtr(h, R_NilValue, R_NilValue));
    R_RegisterCFinalizerEx(ext, wkex_ws_execution_finalize, TRUE);
    UNPROTECT(1);
    return ext;
}

SEXP wkex_ws_place_order(SEXP ext, SEXP market, SEXP side, SEXP quantity, SEXP price) {
    WickraOrder order;
    int rc = wickra_ws_place_order(ws_execution_of(ext), CHAR(STRING_ELT(market, 0)),
                                   Rf_asInteger(side), Rf_asReal(quantity), Rf_asReal(price),
                                   &order);
    if (rc != WICKRA_OK) {
        Rf_error("wickra: ws place_order failed with code %d", rc);
    }
    return order_to_list(&order);
}

SEXP wkex_ws_cancel_order(SEXP ext, SEXP market, SEXP order_id) {
    int rc = wickra_ws_cancel_order(ws_execution_of(ext), CHAR(STRING_ELT(market, 0)),
                                    CHAR(STRING_ELT(order_id, 0)));
    return Rf_ScalarInteger(rc);
}

/* --- registration -------------------------------------------------------- */

static const R_CallMethodDef CallEntries[] = {
    {"wkex_version", (DL_FUNC)&wkex_version, 0},
    {"wkex_paper_new", (DL_FUNC)&wkex_paper_new, 5},
    {"wkex_replay_new", (DL_FUNC)&wkex_replay_new, 7},
    {"wkex_name", (DL_FUNC)&wkex_name, 1},
    {"wkex_set_price", (DL_FUNC)&wkex_set_price, 3},
    {"wkex_place", (DL_FUNC)&wkex_place, 5},
    {"wkex_cancel", (DL_FUNC)&wkex_cancel, 3},
    {"wkex_balance", (DL_FUNC)&wkex_balance, 2},
    {"wkex_poll", (DL_FUNC)&wkex_poll, 2},
    {"wkex_connect_derivatives", (DL_FUNC)&wkex_connect_derivatives, 6},
    {"wkex_derivatives_position", (DL_FUNC)&wkex_derivatives_position, 2},
    {"wkex_derivatives_positions", (DL_FUNC)&wkex_derivatives_positions, 2},
    {"wkex_derivatives_set_leverage", (DL_FUNC)&wkex_derivatives_set_leverage, 3},
    {"wkex_derivatives_set_margin_mode", (DL_FUNC)&wkex_derivatives_set_margin_mode, 3},
    {"wkex_derivatives_close_position", (DL_FUNC)&wkex_derivatives_close_position, 2},
    {"wkex_connect_advanced", (DL_FUNC)&wkex_connect_advanced, 7},
    {"wkex_advanced_amend_order", (DL_FUNC)&wkex_advanced_amend_order, 5},
    {"wkex_advanced_cancel_batch", (DL_FUNC)&wkex_advanced_cancel_batch, 3},
    {"wkex_advanced_place_oco", (DL_FUNC)&wkex_advanced_place_oco, 7},
    {"wkex_advanced_place_batch", (DL_FUNC)&wkex_advanced_place_batch, 5},
    {"wkex_connect_user_data", (DL_FUNC)&wkex_connect_user_data, 7},
    {"wkex_user_data_subscribe", (DL_FUNC)&wkex_user_data_subscribe, 1},
    {"wkex_user_data_keepalive", (DL_FUNC)&wkex_user_data_keepalive, 1},
    {"wkex_user_data_poll", (DL_FUNC)&wkex_user_data_poll, 2},
    {"wkex_connect_ws_execution", (DL_FUNC)&wkex_connect_ws_execution, 7},
    {"wkex_ws_place_order", (DL_FUNC)&wkex_ws_place_order, 5},
    {"wkex_ws_cancel_order", (DL_FUNC)&wkex_ws_cancel_order, 3},
    {NULL, NULL, 0}};

void R_init_wickraexchange(DllInfo *dll) {
    R_registerRoutines(dll, NULL, CallEntries, NULL, NULL);
    R_useDynamicSymbols(dll, FALSE);
}
