/* Golden-fixture parity for the C ABI, and the streaming poll loop it runs on.
 *
 * The Rust suite (crates/wickra-exchange-core/tests/golden.rs) drives the
 * committed replay tapes in golden/ through a ReplayExchange running a fixed
 * SMA strategy, and pins the fill price and the resulting balances. This runs
 * the same fixtures through the same pipeline over the C ABI.
 *
 * replay.c already shows a tape reaching a fill. What it does not check are the
 * numbers: a lost decimal, a dropped fee or slippage applied to the wrong side
 * would still fill, and still pass. This asserts the exact values the Rust suite
 * pins, and it drives them the way a consumer does -- streaming, one
 * wickra_exchange_poll at a time, rather than by asking for a batch.
 *
 * The fixtures are read with the small field reader below rather than a JSON
 * library. This repository ships no C dependency at all and the CMake build
 * links nothing but the ABI itself; taking on a parser so a test can read four
 * numbers and one array out of a file whose shape is fixed and committed would
 * be a poor trade. The reader handles that shape and nothing else.
 *
 * The golden directory is found by walking up from the working directory, since
 * ctest runs from the build tree and its depth differs between generators. */

/* These programs double as the C-side test suite: ctest runs them and a failed
 * expectation must fail the build. CI builds with `--config Release`, and on a
 * multi-config generator that defines NDEBUG -- which turns every assert below
 * into nothing at all, so the Windows runs were asserting no expectation while
 * reporting success. Undefining it before <assert.h> keeps the checks live in
 * every configuration. */
#undef NDEBUG
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "wickra_exchange.h"

#define TOL 1e-6
#define MAX_JSON 4096
#define MAX_TAPE 64
#define MAX_WINDOW 64

/* Read a whole fixture into `out`. Returns 0 on success. */
static int read_fixture(const char *kind, const char *name, char *out, size_t cap) {
    char path[512];
    const char *prefix[] = {"", "../", "../../", "../../../", "../../../../", "../../../../../"};
    for (size_t i = 0; i < sizeof(prefix) / sizeof(prefix[0]); i++) {
        snprintf(path, sizeof(path), "%sgolden/%s/%s.json", prefix[i], kind, name);
        FILE *f = fopen(path, "rb");
        if (f == NULL) {
            continue;
        }
        size_t n = fread(out, 1, cap - 1, f);
        fclose(f);
        out[n] = '\0';
        return 0;
    }
    fprintf(stderr, "golden/%s/%s.json not found from the working directory\n", kind, name);
    return -1;
}

/* The character after `"<key>"` and its colon, or NULL. */
static const char *field(const char *json, const char *key) {
    char needle[64];
    snprintf(needle, sizeof(needle), "\"%s\"", key);
    const char *at = strstr(json, needle);
    if (at == NULL) {
        return NULL;
    }
    at = strchr(at + strlen(needle), ':');
    return at == NULL ? NULL : at + 1;
}

/* The scalar value of "<key>": <number>. */
static double num(const char *json, const char *key) {
    const char *at = field(json, key);
    assert(at != NULL);
    return strtod(at, NULL);
}

/* The value of "<key>": true|false. */
static int flag(const char *json, const char *key) {
    const char *at = field(json, key);
    assert(at != NULL);
    while (*at == ' ') {
        at++;
    }
    return strncmp(at, "true", 4) == 0;
}

/* The array at "<key>": [ ... ] into `out`; returns its length. */
static size_t nums(const char *json, const char *key, double *out, size_t cap) {
    const char *at = field(json, key);
    assert(at != NULL);
    at = strchr(at, '[');
    assert(at != NULL);
    at++;
    size_t count = 0;
    while (count < cap) {
        char *end = NULL;
        double value = strtod(at, &end);
        if (end == at) {
            break;
        }
        out[count++] = value;
        at = end;
        while (*at == ' ' || *at == ',') {
            at++;
        }
        if (*at == ']') {
            break;
        }
    }
    return count;
}

static void run_case(const char *name) {
    char spec[MAX_JSON];
    char expected[MAX_JSON];
    assert(read_fixture("replay", name, spec, sizeof(spec)) == 0);
    assert(read_fixture("expected", name, expected, sizeof(expected)) == 0);

    double tape[MAX_TAPE];
    const size_t n_tape = nums(spec, "tape", tape, MAX_TAPE);
    assert(n_tape > 0);

    const size_t period = (size_t)num(spec, "sma_period");
    assert(period > 0 && period <= MAX_WINDOW);

    const char *assets[] = {"USDT"};
    const double amounts[] = {num(spec, "USDT")};

    WickraExchange *ex = wickra_replay_new("BTC/USDT", tape, n_tape, assets, amounts, 1,
                                           num(spec, "maker_bps"), num(spec, "taker_bps"),
                                           num(spec, "slippage_bps"));
    assert(ex != NULL);

    double window[MAX_WINDOW];
    size_t seen = 0;
    int filled = 0;
    double fill_price = 0.0;

    /* Streaming: each poll advances the recording by exactly one frame, and an
     * empty batch is how an exhausted tape reports itself. */
    for (;;) {
        WickraEvent events[16];
        const int32_t n = wickra_exchange_poll(ex, events, 16);
        assert(n >= 0);
        if (n == 0) {
            break;
        }
        for (int32_t i = 0; i < n; i++) {
            if (events[i].kind != WICKRA_EVENT_TRADE) {
                continue;
            }
            window[seen % MAX_WINDOW] = events[i].price;
            seen++;
            if (seen < period) {
                continue;
            }
            double sum = 0.0;
            for (size_t k = 0; k < period; k++) {
                sum += window[(seen - 1 - k) % MAX_WINDOW];
            }
            const double mean = sum / (double)period;
            if (!filled && events[i].price > mean) {
                WickraOrder order;
                const int32_t rc =
                    wickra_exchange_place_market(ex, "BTC/USDT", WICKRA_SIDE_BUY, 1.0, &order);
                assert(rc == WICKRA_OK);
                fill_price = order.average_price;
                filled = 1;
            }
        }
    }

    assert(filled == flag(expected, "filled"));
    assert(fabs(fill_price - num(expected, "average_price")) < TOL);

    double btc = 0.0;
    double usdt = 0.0;
    assert(wickra_exchange_balance(ex, "BTC", &btc) == WICKRA_OK);
    assert(wickra_exchange_balance(ex, "USDT", &usdt) == WICKRA_OK);
    assert(fabs(btc - num(expected, "btc")) < TOL);
    assert(fabs(usdt - num(expected, "usdt")) < TOL);

    wickra_exchange_free(ex);
    printf("golden %s: fill=%.6f BTC=%.6f USDT=%.6f OK\n", name, fill_price, btc, usdt);
}

int main(void) {
    run_case("sma_cross");
    run_case("sma_cross_with_costs");
    printf("golden parity OK\n");
    return 0;
}
