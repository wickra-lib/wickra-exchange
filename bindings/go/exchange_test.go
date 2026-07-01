package wickraexchange

import (
	"math"
	"testing"
)

func TestVersion(t *testing.T) {
	if Version() == "" {
		t.Fatal("version must not be empty")
	}
}

func TestPaperMarketBuyFills(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 100000}, 1, 5, 10)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()

	if ex.Name() != "paper" {
		t.Fatalf("name = %q, want paper", ex.Name())
	}
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatal(err)
	}

	order, err := ex.PlaceMarket("BTC/USDT", Buy, 1)
	if err != nil {
		t.Fatal(err)
	}
	if !order.IsFilled() {
		t.Fatalf("order status = %d, want filled", order.Status)
	}
	// 10 bps slippage on a buy: 20000 * 1.001 = 20020.
	if math.Abs(order.AveragePrice-20020) > 1e-6 {
		t.Fatalf("average price = %v, want 20020", order.AveragePrice)
	}

	btc, _ := ex.Balance("BTC")
	if math.Abs(btc-1) > 1e-9 {
		t.Fatalf("BTC = %v, want 1", btc)
	}
}

func TestReplayParity(t *testing.T) {
	tape := []float64{100, 101, 102, 110, 112}
	ex, err := ReplayTrades("BTC/USDT", tape, map[string]float64{"USDT": 100000}, 0, 0, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()

	if ex.Name() != "replay" {
		t.Fatalf("name = %q, want replay", ex.Name())
	}

	var window [3]float64
	seen := 0
	bought := false

	for {
		events, err := ex.Poll(16)
		if err != nil {
			t.Fatal(err)
		}
		if len(events) == 0 {
			break
		}
		for _, ev := range events {
			if !ev.IsTrade() {
				continue
			}
			window[seen%3] = ev.Price
			seen++
			if seen >= 3 {
				mean := (window[0] + window[1] + window[2]) / 3
				if !bought && ev.Price > mean {
					order, err := ex.PlaceMarket("BTC/USDT", Buy, 1)
					if err != nil {
						t.Fatal(err)
					}
					if !order.IsFilled() {
						t.Fatal("order not filled")
					}
					bought = true
				}
			}
		}
	}

	if !bought {
		t.Fatal("the rising tape should have crossed the SMA")
	}
	btc, _ := ex.Balance("BTC")
	if math.Abs(btc-1) > 1e-9 {
		t.Fatalf("BTC = %v, want 1", btc)
	}
}
