// Paper-trading example from C++ (the header is `extern "C"` under __cplusplus).
//
// Opens an offline paper account, sets a mark price, places a market buy and
// prints the fill. Build with the CMakeLists.txt in this directory.

#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>

#include "wickra_exchange.h"

int main() {
    const char *assets[] = {"USDT"};
    const double amounts[] = {100000.0};

    // maker 1 bps, taker 5 bps, slippage 10 bps.
    WickraExchange *ex = wickra_paper_new(assets, amounts, 1, 1.0, 5.0, 10.0);
    assert(ex != nullptr);

    int rc = wickra_exchange_set_price(ex, "BTC/USDT", 20000.0);
    assert(rc == WICKRA_OK);

    WickraOrder order;
    rc = wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 1.0, &order);
    assert(rc == WICKRA_OK);
    assert(order.status == WICKRA_STATUS_FILLED);
    // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
    assert(std::fabs(order.average_price - 20020.0) < 1e-6);

    double btc = 0.0, usdt = 0.0;
    wickra_exchange_balance(ex, "BTC", &btc);
    wickra_exchange_balance(ex, "USDT", &usdt);

    std::cout << "filled at " << order.average_price << "; BTC=" << btc << " USDT=" << usdt
              << std::endl;
    assert(std::fabs(btc - 1.0) < 1e-9);

    wickra_exchange_free(ex);
    std::cout << "paper example OK" << std::endl;
    return 0;
}
