// What it costs to reach the library from Go.
//
// Same two operations, same offline paper account, same iteration count as
// every other program in this directory and as the Rust baseline. The
// difference from the baseline is this binding's overhead.
//
//	go run binding_cost.go
package main

import (
	"fmt"
	"time"

	wickra "github.com/wickra-lib/wickra-exchange-go"
)

const (
	iterations = 20000
	warmup     = 1000
)

func report(operation string, elapsed time.Duration) {
	perCall := float64(elapsed.Nanoseconds()) / float64(iterations)
	fmt.Printf("%-12s %10.0f ns/op   %12.0f ops/s\n", operation, perCall, 1e9/perCall)
}

func main() {
	ex, err := wickra.Paper(map[string]float64{"USDT": 1e9}, 0, 0, 0)
	if err != nil {
		panic(err)
	}
	defer ex.Close()
	if err := ex.SetPrice("BTC/USDT", 20000); err != nil {
		panic(err)
	}

	// The first call through any boundary pays for one-time setup, which is not
	// what is being measured.
	for i := 0; i < warmup; i++ {
		if _, err := ex.Ticker("BTC/USDT"); err != nil {
			panic(err)
		}
	}
	started := time.Now()
	for i := 0; i < iterations; i++ {
		if _, err := ex.Ticker("BTC/USDT"); err != nil {
			panic(err)
		}
	}
	report("ticker", time.Since(started))

	request := wickra.OrderRequest{Market: "BTC/USDT", Side: wickra.Buy, Quantity: 0.0001}
	for i := 0; i < warmup; i++ {
		if _, err := ex.PlaceOrder(request); err != nil {
			panic(err)
		}
	}
	started = time.Now()
	for i := 0; i < iterations; i++ {
		if _, err := ex.PlaceOrder(request); err != nil {
			panic(err)
		}
	}
	report("place_order", time.Since(started))
}
