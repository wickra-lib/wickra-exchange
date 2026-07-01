// Package wickraexchange provides idiomatic Go bindings for wickra-exchange over
// its C ABI hub: one synchronous, pull-based API over the ten largest crypto
// exchanges, plus offline paper and replay simulators that share the same API.
//
// The same strategy runs paper, replay and live by swapping the constructor. The
// binding links the prebuilt C ABI library, staged per platform under
// ./lib/<goos>_<goarch>/, with the header vendored under ./include.
package wickraexchange

/*
#cgo CFLAGS: -I${SRCDIR}/include
#cgo linux,amd64 LDFLAGS: -L${SRCDIR}/lib/linux_amd64 -lwickra_exchange -Wl,-rpath,${SRCDIR}/lib/linux_amd64
#cgo linux,arm64 LDFLAGS: -L${SRCDIR}/lib/linux_arm64 -lwickra_exchange -Wl,-rpath,${SRCDIR}/lib/linux_arm64
#cgo darwin,amd64 LDFLAGS: -L${SRCDIR}/lib/darwin_amd64 -lwickra_exchange -Wl,-rpath,${SRCDIR}/lib/darwin_amd64
#cgo darwin,arm64 LDFLAGS: -L${SRCDIR}/lib/darwin_arm64 -lwickra_exchange -Wl,-rpath,${SRCDIR}/lib/darwin_arm64
#cgo windows,amd64 LDFLAGS: -L${SRCDIR}/lib/windows_amd64 -l:wickra_exchange.dll
#cgo windows,arm64 LDFLAGS: -L${SRCDIR}/lib/windows_arm64 -l:wickra_exchange.dll
#include <stdlib.h>
#include "wickra_exchange.h"
*/
import "C"

import (
	"fmt"
	"runtime"
	"unsafe"
)

// Side is the side of an order.
type Side int32

const (
	// Buy side.
	Buy Side = C.WICKRA_SIDE_BUY
	// Sell side.
	Sell Side = C.WICKRA_SIDE_SELL
)

// Status is the lifecycle state of an order.
type Status int32

// Order lifecycle states.
const (
	StatusNew             Status = C.WICKRA_STATUS_NEW
	StatusPartiallyFilled Status = C.WICKRA_STATUS_PARTIALLY_FILLED
	StatusFilled          Status = C.WICKRA_STATUS_FILLED
	StatusCanceled        Status = C.WICKRA_STATUS_CANCELED
	StatusRejected        Status = C.WICKRA_STATUS_REJECTED
	StatusExpired         Status = C.WICKRA_STATUS_EXPIRED
)

// Kind is the kind of a stream event.
type Kind int32

// Stream event kinds.
const (
	KindTrade         Kind = C.WICKRA_EVENT_TRADE
	KindTicker        Kind = C.WICKRA_EVENT_TICKER
	KindOrderUpdate   Kind = C.WICKRA_EVENT_ORDER_UPDATE
	KindBalanceUpdate Kind = C.WICKRA_EVENT_BALANCE_UPDATE
	KindSubscribed    Kind = C.WICKRA_EVENT_SUBSCRIBED
	KindOther         Kind = C.WICKRA_EVENT_OTHER
)

// Order is an order as reported by the exchange.
type Order struct {
	ID             string
	Side           Side
	Status         Status
	Quantity       float64
	FilledQuantity float64
	Price          float64 // NaN if none
	AveragePrice   float64 // NaN if none
}

// IsFilled reports whether the order is fully filled.
func (o Order) IsFilled() bool { return o.Status == StatusFilled }

// Event is a single stream event.
type Event struct {
	Kind     Kind
	Symbol   string
	Price    float64 // NaN unless a trade/ticker
	Quantity float64 // NaN unless a trade
	Side     Side    // -1 unless a trade
	Order    Order   // populated for KindOrderUpdate
}

// IsTrade reports whether this is a trade event.
func (e Event) IsTrade() bool { return e.Kind == KindTrade }

// Version returns the library version.
func Version() string {
	return C.GoString(C.wickra_version())
}

// Exchange is a unified exchange client over the synchronous, pull-based API.
// Construct with Paper, ReplayTrades or Connect. Call Close to release native
// resources; a finalizer is a backstop only.
type Exchange struct {
	handle *C.WickraExchange
}

// Paper opens an offline paper account seeded from balances (asset -> amount).
func Paper(balances map[string]float64, makerBps, takerBps, slippageBps float64) (*Exchange, error) {
	cAssets, cAmounts, free := marshalBalances(balances)
	defer free()
	handle := C.wickra_paper_new(
		assetsPtr(cAssets), amountsPtr(cAmounts), C.size_t(len(balances)),
		C.double(makerBps), C.double(takerBps), C.double(slippageBps))
	return wrap(handle, "paper")
}

// ReplayTrades opens a replay account driven by a recorded tape of trades.
func ReplayTrades(market string, tape []float64, balances map[string]float64, makerBps, takerBps, slippageBps float64) (*Exchange, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	cAssets, cAmounts, free := marshalBalances(balances)
	defer free()
	var tapePtr *C.double
	if len(tape) > 0 {
		tapePtr = (*C.double)(unsafe.Pointer(&tape[0]))
	}
	handle := C.wickra_replay_new(
		cMarket, tapePtr, C.size_t(len(tape)),
		assetsPtr(cAssets), amountsPtr(cAmounts), C.size_t(len(balances)),
		C.double(makerBps), C.double(takerBps), C.double(slippageBps))
	return wrap(handle, "replay")
}

// Connect opens a live client for name, authenticated with API keys.
func Connect(name, apiKey, apiSecret, passphrase, privateKey string, testnet bool) (*Exchange, error) {
	cName := C.CString(name)
	cKey := C.CString(apiKey)
	cSecret := C.CString(apiSecret)
	defer C.free(unsafe.Pointer(cName))
	defer C.free(unsafe.Pointer(cKey))
	defer C.free(unsafe.Pointer(cSecret))
	var cPass, cPriv *C.char
	if passphrase != "" {
		cPass = C.CString(passphrase)
		defer C.free(unsafe.Pointer(cPass))
	}
	if privateKey != "" {
		cPriv = C.CString(privateKey)
		defer C.free(unsafe.Pointer(cPriv))
	}
	handle := C.wickra_connect(cName, cKey, cSecret, cPass, cPriv, C.bool(testnet))
	return wrap(handle, name)
}

// Name returns the venue identifier ("paper", "replay", "binance", ...).
func (e *Exchange) Name() string {
	buf := make([]C.char, 32)
	C.wickra_exchange_name(e.handle, &buf[0], C.size_t(len(buf)))
	return C.GoString(&buf[0])
}

// SetPrice sets the mark price a paper account fills against (paper backend only).
func (e *Exchange) SetPrice(market string, price float64) error {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	return codeError(C.wickra_exchange_set_price(e.handle, cMarket, C.double(price)))
}

// PlaceMarket places a market order and returns the resulting order.
func (e *Exchange) PlaceMarket(market string, side Side, quantity float64) (Order, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	var out C.WickraOrder
	rc := C.wickra_exchange_place_market(e.handle, cMarket, C.int(side), C.double(quantity), &out)
	if err := codeError(rc); err != nil {
		return Order{}, err
	}
	return readOrder(&out), nil
}

// PlaceLimit places a limit order and returns the resulting order.
func (e *Exchange) PlaceLimit(market string, side Side, quantity, price float64) (Order, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	var out C.WickraOrder
	rc := C.wickra_exchange_place_limit(e.handle, cMarket, C.int(side), C.double(quantity), C.double(price), &out)
	if err := codeError(rc); err != nil {
		return Order{}, err
	}
	return readOrder(&out), nil
}

// Cancel cancels an open order by venue id.
func (e *Exchange) Cancel(market, orderID string) error {
	cMarket := C.CString(market)
	cOrder := C.CString(orderID)
	defer C.free(unsafe.Pointer(cMarket))
	defer C.free(unsafe.Pointer(cOrder))
	return codeError(C.wickra_exchange_cancel(e.handle, cMarket, cOrder))
}

// Balance returns the free balance of asset.
func (e *Exchange) Balance(asset string) (float64, error) {
	cAsset := C.CString(asset)
	defer C.free(unsafe.Pointer(cAsset))
	var out C.double
	if err := codeError(C.wickra_exchange_balance(e.handle, cAsset, &out)); err != nil {
		return 0, err
	}
	return float64(out), nil
}

// Poll drains buffered events (up to capacity per call).
func (e *Exchange) Poll(capacity int) ([]Event, error) {
	buf := make([]C.WickraEvent, capacity)
	count := C.wickra_exchange_poll(e.handle, &buf[0], C.size_t(capacity))
	if count < 0 {
		return nil, fmt.Errorf("wickra: poll failed with code %d", int(count))
	}
	events := make([]Event, int(count))
	for i := 0; i < int(count); i++ {
		events[i] = readEvent(&buf[i])
	}
	return events, nil
}

// Close releases the native handle.
func (e *Exchange) Close() {
	if e.handle != nil {
		C.wickra_exchange_free(e.handle)
		e.handle = nil
		runtime.SetFinalizer(e, nil)
	}
}

// --- helpers ---

func wrap(handle *C.WickraExchange, what string) (*Exchange, error) {
	if handle == nil {
		return nil, fmt.Errorf("wickra: failed to construct %s exchange", what)
	}
	ex := &Exchange{handle: handle}
	runtime.SetFinalizer(ex, (*Exchange).Close)
	return ex, nil
}

func marshalBalances(balances map[string]float64) ([]*C.char, []C.double, func()) {
	assets := make([]*C.char, 0, len(balances))
	amounts := make([]C.double, 0, len(balances))
	for k, v := range balances {
		assets = append(assets, C.CString(k))
		amounts = append(amounts, C.double(v))
	}
	free := func() {
		for _, p := range assets {
			C.free(unsafe.Pointer(p))
		}
	}
	return assets, amounts, free
}

func assetsPtr(assets []*C.char) **C.char {
	if len(assets) == 0 {
		return nil
	}
	return (**C.char)(unsafe.Pointer(&assets[0]))
}

func amountsPtr(amounts []C.double) *C.double {
	if len(amounts) == 0 {
		return nil
	}
	return &amounts[0]
}

func readOrder(o *C.WickraOrder) Order {
	return Order{
		ID:             C.GoString(&o.id[0]),
		Side:           Side(o.side),
		Status:         Status(o.status),
		Quantity:       float64(o.quantity),
		FilledQuantity: float64(o.filled_quantity),
		Price:          float64(o.price),
		AveragePrice:   float64(o.average_price),
	}
}

func readEvent(e *C.WickraEvent) Event {
	return Event{
		Kind:     Kind(e.kind),
		Symbol:   C.GoString(&e.symbol[0]),
		Price:    float64(e.price),
		Quantity: float64(e.quantity),
		Side:     Side(e.side),
		Order:    readOrder(&e.order),
	}
}

func codeError(code C.int32_t) error {
	if code == C.WICKRA_OK {
		return nil
	}
	return fmt.Errorf("wickra: exchange call failed with code %d", int(code))
}
