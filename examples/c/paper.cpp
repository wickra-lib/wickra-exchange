// Paper-trading example from C++ (the header is `extern "C"` under __cplusplus).
//
// Opens an offline paper account, sets a mark price, places a market buy and
// prints the fill. Build with the CMakeLists.txt in this directory.
//
// The handle is owned by `wickra::Exchange` from the optional C++ layer rather
// than freed by hand at the end: every `assert` below is an early exit, and a
// hand-written free after them is not reached when one fires.

/* These programs double as the C-side test suite: ctest runs them and a failed
 * expectation must fail the build. CI builds with `--config Release`, and on a
 * multi-config generator that defines NDEBUG -- which turns every assert below
 * into nothing at all, so the Windows runs were asserting no expectation while
 * reporting success. Undefining it before <assert.h> keeps the checks live in
 * every configuration. */
#undef NDEBUG
#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>

#include "wickra_exchange.hpp"

int main() {
    const char *assets[] = {"USDT"};
    const double amounts[] = {100000.0};

    // maker 1 bps, taker 5 bps, slippage 10 bps.
    wickra::Exchange ex(wickra_paper_new(assets, amounts, 1, 1.0, 5.0, 10.0));
    assert(static_cast<bool>(ex));

    int rc = wickra_exchange_set_price(ex.get(), "BTC/USDT", 20000.0);
    assert(rc == WICKRA_OK);

    WickraOrder order;
    rc = wickra_exchange_place_market(ex.get(), "BTC/USDT", WICKRA_SIDE_BUY, 1.0, &order);
    assert(rc == WICKRA_OK);
    assert(order.status == WICKRA_STATUS_FILLED);
    // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
    assert(std::fabs(order.average_price - 20020.0) < 1e-6);

    double btc = 0.0, usdt = 0.0;
    wickra_exchange_balance(ex.get(), "BTC", &btc);
    wickra_exchange_balance(ex.get(), "USDT", &usdt);

    std::cout << "filled at " << order.average_price << "; BTC=" << btc << " USDT=" << usdt
              << std::endl;
    assert(std::fabs(btc - 1.0) < 1e-9);

    std::cout << "paper example OK" << std::endl;
    return 0;
}
