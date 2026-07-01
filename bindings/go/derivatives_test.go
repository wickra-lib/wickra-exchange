package wickraexchange

import (
	"math"
	"testing"
)

// Construction is offline (no socket opens until an RPC is issued), so the
// surface and the spot-only rejection are checked without a network.

func TestDerivativesRejectsSpotOnly(t *testing.T) {
	for _, name := range []string{"coinbase", "upbit", "ftx"} {
		if d, err := ConnectDerivatives(name, "k", "s", "", "", false); err == nil {
			d.Close()
			t.Fatalf("%s must be rejected for derivatives", name)
		}
	}
}

func TestAdvancedRejectsSpotOnly(t *testing.T) {
	for _, name := range []string{"coinbase", "upbit", "ftx"} {
		if a, err := ConnectAdvanced(name, "k", "s", "", "", false, false); err == nil {
			a.Close()
			t.Fatalf("%s must be rejected for advanced orders", name)
		}
	}
}

func TestDerivativesAndAdvancedConstruct(t *testing.T) {
	d, err := ConnectDerivatives("binance", "k", "s", "", "", false)
	if err != nil {
		t.Fatal(err)
	}
	d.Close()
	a, err := ConnectAdvanced("binance", "k", "s", "", "", false, true)
	if err != nil {
		t.Fatal(err)
	}
	a.Close()
}

func TestPlaceBatchEmptyIsNoop(t *testing.T) {
	// An empty batch returns without opening a socket.
	a, err := ConnectAdvanced("binance", "k", "s", "", "", false, false)
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	results, err := a.PlaceBatch(nil)
	if err != nil {
		t.Fatalf("empty batch must not error: %v", err)
	}
	if results != nil {
		t.Fatalf("empty batch must return nil results, got %v", results)
	}
}

func TestBatchRequestShape(t *testing.T) {
	requests := []OrderRequest{
		{Market: "BTC/USDT", Side: Buy, Quantity: 0.5, Price: 60000},
		{Market: "ETH/USDT", Side: Sell, Quantity: 2, Price: math.NaN()},
	}
	if len(requests) != 2 {
		t.Fatalf("want 2 requests, got %d", len(requests))
	}
	if requests[0].Side != Buy || requests[1].Side != Sell {
		t.Fatal("request sides must round-trip")
	}
	if !math.IsNaN(requests[1].Price) {
		t.Fatal("a market order request carries a NaN price")
	}
}

func TestMarginModeConstants(t *testing.T) {
	if MarginCross == MarginIsolated {
		t.Fatal("margin mode constants must differ")
	}
	if PositionLong == PositionShort {
		t.Fatal("position side constants must differ")
	}
}
