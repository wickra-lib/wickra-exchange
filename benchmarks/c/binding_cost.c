/* What it costs to reach the library from C.
 *
 * Same two operations, same offline paper account, same iteration count as
 * every other program in this directory and as the Rust baseline. The C ABI is
 * the floor the four languages built on it cannot go below, so this is the one
 * number that says how much of their cost is the boundary itself.
 *
 * Built by benchmarks/c/CMakeLists.txt against the same header and library the
 * examples use.
 */

#include <stdio.h>
#include <time.h>

#include "wickra_exchange.h"

#define ITERATIONS 20000
#define WARMUP 1000

static void report(const char *operation, double nanos) {
    double per_call = nanos / (double)ITERATIONS;
    printf("%-12s %10.0f ns/op   %12.0f ops/s\n", operation, per_call, 1e9 / per_call);
}

static double elapsed_nanos(struct timespec start, struct timespec end) {
    return (double)(end.tv_sec - start.tv_sec) * 1e9 + (double)(end.tv_nsec - start.tv_nsec);
}

int main(void) {
    const char *assets[] = {"USDT"};
    const double amounts[] = {1e9};
    WickraExchange *ex = wickra_paper_new(assets, amounts, 1, 0.0, 0.0, 0.0);
    if (ex == NULL) {
        fprintf(stderr, "could not build the paper exchange\n");
        return 1;
    }
    wickra_exchange_set_price(ex, "BTC/USDT", 20000.0);

    WickraTicker ticker;
    struct timespec start, end;

    /* The first call through any boundary pays for one-time setup, which is not
     * what is being measured. */
    for (int i = 0; i < WARMUP; i++) {
        wickra_exchange_ticker(ex, "BTC/USDT", &ticker);
    }
    timespec_get(&start, TIME_UTC);
    for (int i = 0; i < ITERATIONS; i++) {
        wickra_exchange_ticker(ex, "BTC/USDT", &ticker);
    }
    timespec_get(&end, TIME_UTC);
    report("ticker", elapsed_nanos(start, end));

    WickraOrder order;
    for (int i = 0; i < WARMUP; i++) {
        wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 0.0001, &order);
    }
    timespec_get(&start, TIME_UTC);
    for (int i = 0; i < ITERATIONS; i++) {
        wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 0.0001, &order);
    }
    timespec_get(&end, TIME_UTC);
    report("place_order", elapsed_nanos(start, end));

    wickra_exchange_free(ex);
    return 0;
}
