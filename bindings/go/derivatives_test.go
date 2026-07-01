package wickraexchange

import "testing"

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

func TestMarginModeConstants(t *testing.T) {
	if MarginCross == MarginIsolated {
		t.Fatal("margin mode constants must differ")
	}
	if PositionLong == PositionShort {
		t.Fatal("position side constants must differ")
	}
}
