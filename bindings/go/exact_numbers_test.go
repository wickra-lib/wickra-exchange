package wickraexchange

import (
	"testing"
)

// An order number arrives as the number that was written.
//
// A float64 holds about fifteen significant digits, and the library holds every
// order number in an exact decimal. Sent as a float64, 12345678.90123456789
// arrives as 12345678.90123457 -- a different order, placed without a word. Go
// has no decimal type, so the exact spelling is a string, which is what every
// exchange's own API takes for the same reason.

// Wider than a float64: the last digits are the ones it cannot hold.
const wideNumber = "12345678.90123456789"

// The order type is derived from which prices are set. A price given only as
// exact text still has to count as set -- otherwise a limit order with an exact
// price becomes a *market* order, which takes whatever the book offers.
func TestExactTextCountsWhenTheOrderTypeIsDerived(t *testing.T) {
	cases := []struct {
		name    string
		request OrderRequest
		want    int32
	}{
		{
			"limit from text alone",
			OrderRequest{PriceText: "19000"},
			1,
		},
		{
			"stop-market from text alone",
			OrderRequest{StopPriceText: "19500"},
			2,
		},
		{
			"stop-limit from text alone",
			OrderRequest{PriceText: "19000", StopPriceText: "19500"},
			3,
		},
		{
			"market when neither is set either way",
			OrderRequest{},
			0,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := int32(tc.request.orderType()); got != tc.want {
				t.Fatalf("order type = %d, want %d", got, tc.want)
			}
		})
	}
}

// The exact value is the one that is used, and the float64 beside it may say
// anything. Every number this binding reports is a float64, so what can be
// observed here is which of the two was read -- that the wide number itself
// survives the crossing is held by the C ABI's own tests, where it can be read
// back exactly.
func TestTheExactQuantityIsTheOneThatIsUsed(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 1000000, "BTC": 100}, 0, 0, 0)
	if err != nil {
		t.Fatalf("paper: %v", err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatalf("set price: %v", err)
	}

	order, err := ex.PlaceOrder(OrderRequest{
		Market:       "BTC/USDT",
		Side:         Sell,
		Quantity:     999,
		Price:        21000,
		QuantityText: "1.5",
	})
	if err != nil {
		t.Fatalf("place: %v", err)
	}
	if order.Quantity != 1.5 {
		t.Fatalf("quantity = %v, want 1.5 (the exact value, not the float beside it)", order.Quantity)
	}
}

func TestTheExactPriceIsTheOneThatIsUsed(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 1000000, "BTC": 100}, 0, 0, 0)
	if err != nil {
		t.Fatalf("paper: %v", err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatalf("set price: %v", err)
	}

	order, err := ex.PlaceOrder(OrderRequest{
		Market:    "BTC/USDT",
		Side:      Sell,
		Quantity:  1,
		Price:     21000,
		PriceText: "21111.25",
	})
	if err != nil {
		t.Fatalf("place: %v", err)
	}
	if order.Price != 21111.25 {
		t.Fatalf("price = %v, want 21111.25", order.Price)
	}
}

// Text that is not a decimal is a refused order, not an order at some other
// number.
func TestTextThatIsNotANumberRefusesTheOrder(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 1000000, "BTC": 100}, 0, 0, 0)
	if err != nil {
		t.Fatalf("paper: %v", err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatalf("set price: %v", err)
	}

	if _, err := ex.PlaceOrder(OrderRequest{
		Market:    "BTC/USDT",
		Side:      Sell,
		Quantity:  1,
		PriceText: "nineteen thousand",
	}); err == nil {
		t.Fatal("an unparsable price placed an order")
	}
}

// A wide number is accepted where the float64 beside it is left empty, which is
// the whole point of the field.
func TestAWideNumberIsAccepted(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 1000000000000, "BTC": 100}, 0, 0, 0)
	if err != nil {
		t.Fatalf("paper: %v", err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatalf("set price: %v", err)
	}

	order, err := ex.PlaceOrder(OrderRequest{
		Market:    "BTC/USDT",
		Side:      Sell,
		Quantity:  1,
		PriceText: wideNumber,
	})
	if err != nil {
		t.Fatalf("place: %v", err)
	}
	if order.Status != StatusNew {
		t.Fatalf("status = %v, want a resting order", order.Status)
	}
}
