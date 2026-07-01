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
    {NULL, NULL, 0}};

void R_init_wickra_exchange(DllInfo *dll) {
    R_registerRoutines(dll, NULL, CallEntries, NULL, NULL);
    R_useDynamicSymbols(dll, FALSE);
}
