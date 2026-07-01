/* Replay-parity example: a recorded tape drives a signal that fills on the book.
 *
 * Mirrors the Rust/Python/Node end-to-end tests: a rising price tape breaks
 * above a 3-period moving average, and the resulting market buy fills on the
 * paper book. Build with the CMakeLists.txt in this directory. */

#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

#include "wickra_exchange.h"

int main(void) {
    const double tape[] = {100.0, 101.0, 102.0, 110.0, 112.0};
    const size_t n_tape = sizeof(tape) / sizeof(tape[0]);

    const char *assets[] = {"USDT"};
    const double amounts[] = {100000.0};

    WickraExchange *ex =
        wickra_replay_new("BTC/USDT", tape, n_tape, assets, amounts, 1, 0.0, 0.0, 0.0);
    assert(ex != NULL);

    char name[32];
    wickra_exchange_name(ex, name, sizeof(name));
    assert(strcmp(name, "replay") == 0);

    double window[3];
    size_t seen = 0;
    int bought = 0;

    for (;;) {
        WickraEvent events[8];
        int count = wickra_exchange_poll(ex, events, 8);
        if (count <= 0) {
            break;
        }
        for (int i = 0; i < count; i++) {
            if (events[i].kind != WICKRA_EVENT_TRADE) {
                continue;
            }
            double price = events[i].price;
            window[seen % 3] = price;
            seen++;
            if (seen >= 3) {
                double mean = (window[0] + window[1] + window[2]) / 3.0;
                if (!bought && price > mean) {
                    WickraOrder order;
                    int rc = wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 1.0,
                                                          &order);
                    assert(rc == WICKRA_OK);
                    assert(order.status == WICKRA_STATUS_FILLED);
                    bought = 1;
                }
            }
        }
    }

    assert(bought);

    double btc = 0.0;
    wickra_exchange_balance(ex, "BTC", &btc);
    printf("filled; BTC balance = %.4f\n", btc);
    assert(fabs(btc - 1.0) < 1e-9);

    wickra_exchange_free(ex);
    printf("replay-parity example OK\n");
    return 0;
}
