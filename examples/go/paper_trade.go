// Paper-trade differentiator demo.
//
// Run from a module that requires github.com/wickra-lib/wickra-exchange-go:
//
//	go run paper_trade.go
package main

import (
	"fmt"

	wx "github.com/wickra-lib/wickra-exchange-go"
)

func main() {
	ex, err := wx.Paper(map[string]float64{"USDT": 100000}, 1, 5, 10)
	if err != nil {
		panic(err)
	}
	defer ex.Close()

	fmt.Println("venue:", ex.Name())
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		panic(err)
	}

	order, err := ex.PlaceMarket("BTC/USDT", wx.Buy, 1)
	if err != nil {
		panic(err)
	}
	fmt.Printf("filled at %v (filled=%v)\n", order.AveragePrice, order.IsFilled())

	btc, _ := ex.Balance("BTC")
	fmt.Printf("  BTC free: %v\n", btc)
}
