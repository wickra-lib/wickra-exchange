package wickraexchange

// Golden-fixture parity for the Go binding.
//
// The Rust suite (crates/wickra-exchange-core/tests/golden.rs) drives the
// committed replay tapes in golden/ through a ReplayExchange running a fixed
// SMA strategy, and pins the fill price and the resulting balances. This runs
// the same fixtures through the same pipeline over the C ABI.
//
// exchange_test.go already proves a paper order fills. What it does not do is
// check the numbers a *replayed* tape produces: a lost decimal, a dropped fee
// or slippage applied to the wrong side would still produce a fill, and still
// pass. These assert the exact values the Rust suite pins.
//
// The strategy is reimplemented rather than imported, so this tests the
// replay-to-paper-fill pipeline rather than two libraries agreeing.

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"testing"
)

const goldenTol = 1e-6

type goldenSpec struct {
	Market      string             `json:"market"`
	Tape        []float64          `json:"tape"`
	Balances    map[string]float64 `json:"balances"`
	SmaPeriod   int                `json:"sma_period"`
	MakerBps    float64            `json:"maker_bps"`
	TakerBps    float64            `json:"taker_bps"`
	SlippageBps float64            `json:"slippage_bps"`
}

type goldenExpected struct {
	Filled       bool    `json:"filled"`
	AveragePrice float64 `json:"average_price"`
	Btc          float64 `json:"btc"`
	Usdt         float64 `json:"usdt"`
}

// goldenPath resolves a tape for either layout this file is compiled in.
//
// In this repository the package sits at bindings/go and the tapes are two
// directories up. In the published wickra-exchange-go module the package is the
// module root and the tapes are staged beside it, so the repository-relative
// path points outside the module and does not exist. Trying both keeps one copy
// of these tests running in both places, rather than shipping a test that can
// only fail for whoever runs `go test ./...` after `go get`.
func goldenPath(kind, name string) (string, error) {
	candidates := []string{
		filepath.Join("..", "..", "golden", kind, name+".json"),
		filepath.Join("golden", kind, name+".json"),
	}
	for _, path := range candidates {
		if _, err := os.Stat(path); err == nil {
			return path, nil
		}
	}
	return "", fmt.Errorf("no golden tape %s/%s.json in %v", kind, name, candidates)
}

func readGolden(t *testing.T, kind, name string, into any) {
	t.Helper()
	path, err := goldenPath(kind, name)
	if err != nil {
		t.Fatalf("locating golden tape: %v", err)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("reading %s: %v", path, err)
	}
	if err := json.Unmarshal(raw, into); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
}

// newSma returns a streaming simple moving average over the last `window`
// values, reporting false until it has that many.
func newSma(window int) func(float64) (float64, bool) {
	values := make([]float64, 0, window)
	return func(price float64) (float64, bool) {
		values = append(values, price)
		if len(values) > window {
			values = values[1:]
		}
		if len(values) < window {
			return 0, false
		}
		sum := 0.0
		for _, v := range values {
			sum += v
		}
		return sum / float64(window), true
	}
}

func runGoldenCase(t *testing.T, name string) {
	t.Helper()

	var spec goldenSpec
	var expected goldenExpected
	readGolden(t, "replay", name, &spec)
	readGolden(t, "expected", name, &expected)

	ex, err := ReplayTrades(spec.Market, spec.Tape, spec.Balances,
		spec.MakerBps, spec.TakerBps, spec.SlippageBps)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()

	sma := newSma(spec.SmaPeriod)
	filled := false
	fillPrice := math.NaN()

	// Each poll advances the recording by exactly one frame; an empty batch is
	// how an exhausted tape reports itself.
	for {
		events, err := ex.Poll(64)
		if err != nil {
			t.Fatal(err)
		}
		if len(events) == 0 {
			break
		}
		for _, event := range events {
			if !event.IsTrade() {
				continue
			}
			mean, ready := sma(event.Price)
			if ready && !filled && event.Price > mean {
				order, err := ex.PlaceMarket(spec.Market, Buy, 1)
				if err != nil {
					t.Fatal(err)
				}
				fillPrice = order.AveragePrice
				filled = true
			}
		}
	}

	if filled != expected.Filled {
		t.Fatalf("filled = %v, want %v", filled, expected.Filled)
	}
	if math.Abs(fillPrice-expected.AveragePrice) > goldenTol {
		t.Errorf("average price = %v, want %v", fillPrice, expected.AveragePrice)
	}

	btc, err := ex.Balance("BTC")
	if err != nil {
		t.Fatal(err)
	}
	if math.Abs(btc-expected.Btc) > goldenTol {
		t.Errorf("BTC = %v, want %v", btc, expected.Btc)
	}

	usdt, err := ex.Balance("USDT")
	if err != nil {
		t.Fatal(err)
	}
	if math.Abs(usdt-expected.Usdt) > goldenTol {
		t.Errorf("USDT = %v, want %v", usdt, expected.Usdt)
	}
}

func TestGoldenSmaCrossFrictionless(t *testing.T) {
	runGoldenCase(t, "sma_cross")
}

func TestGoldenSmaCrossWithCosts(t *testing.T) {
	runGoldenCase(t, "sma_cross_with_costs")
}
