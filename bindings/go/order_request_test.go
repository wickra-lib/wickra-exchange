package wickraexchange

import (
	"math"
	"testing"
)

// The order type is derived from which prices are set, so a caller never names
// it and cannot name one that contradicts the prices it gave. Setting a stop
// price promotes the order into its trigger form, matching the Rust builder.
func TestOrderTypeIsDerivedFromThePricesSet(t *testing.T) {
	cases := []struct {
		name      string
		price     float64
		stopPrice float64
		want      int32
	}{
		{"market: no price at all", 0, 0, 0},
		{"market: NaN price", math.NaN(), 0, 0},
		{"limit: a price", 19000, 0, 1},
		{"stop-market: a trigger and no price", 0, 19500, 2},
		{"stop-limit: both", 19000, 19500, 3},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			request := OrderRequest{
				Market:    "BTC/USDT",
				Side:      Sell,
				Quantity:  1,
				Price:     tc.price,
				StopPrice: tc.stopPrice,
			}
			if got := int32(request.orderType()); got != tc.want {
				t.Fatalf("orderType() = %d, want %d", got, tc.want)
			}
		})
	}
}

// Zero and NaN both mean "unset" for a price: zero because no exchange accepts
// it, NaN because that is how this package has always spelled an absent price.
func TestZeroAndNaNPricesAreBothUnset(t *testing.T) {
	if isSet(0) {
		t.Error("zero must be unset")
	}
	if isSet(math.NaN()) {
		t.Error("NaN must be unset")
	}
	if !isSet(19000) {
		t.Error("a real price must be set")
	}
	if !math.IsNaN(float64(cPrice(0))) {
		t.Error("an unset price must project to NaN")
	}
	if float64(cPrice(19000)) != 19000 {
		t.Error("a set price must project to itself")
	}
}

// A request written against the four-field shape still means what it meant: the
// zero value of every field added since is "unset".
func TestTheOlderFourFieldShapeStillMeansTheSame(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 100000}, 1, 5, 10)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatal(err)
	}

	order, err := ex.PlaceOrder(OrderRequest{
		Market:   "BTC/USDT",
		Side:     Buy,
		Quantity: 1,
		Price:    math.NaN(),
	})
	if err != nil {
		t.Fatal(err)
	}
	if order.Status != StatusFilled {
		t.Fatalf("status = %v, want filled", order.Status)
	}
}

// The fields that had no way through before now reach the exchange. On the
// paper backend a trigger order is refused, and that refusal is the proof the
// trigger arrived: a request with the field dropped would have been placed as a
// plain market sell instead, at the price the stop existed to protect against.
func TestAStopOrderCarriesItsTrigger(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 100000, "BTC": 5}, 1, 5, 10)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatal(err)
	}

	if _, err := ex.PlaceOrder(OrderRequest{
		Market:    "BTC/USDT",
		Side:      Sell,
		Quantity:  1,
		StopPrice: 19000,
	}); err == nil {
		t.Fatal("a trigger order was accepted without the venue supporting one")
	}
}

// A resting order carries the flags that decide what it is.
func TestARestingOrderCarriesItsFlags(t *testing.T) {
	ex, err := Paper(map[string]float64{"USDT": 100000}, 1, 5, 10)
	if err != nil {
		t.Fatal(err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		t.Fatal(err)
	}

	order, err := ex.PlaceOrder(OrderRequest{
		Market:        "BTC/USDT",
		Side:          Buy,
		Quantity:      1,
		Price:         19000,
		TimeInForce:   GTC,
		ClientOrderID: "retry-safe-1",
		PostOnly:      true,
		STP:           STPExpireMaker,
	})
	if err != nil {
		t.Fatal(err)
	}
	if order.Status != StatusNew {
		t.Fatalf("status = %v, want new", order.Status)
	}
}
