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

func readPosition(p *C.WickraPosition) Position {
	return Position{
		Symbol:        C.GoString(&p.symbol[0]),
		Side:          PositionSide(p.side),
		Quantity:      float64(p.quantity),
		EntryPrice:    float64(p.entry_price),
		MarkPrice:     float64(p.mark_price),
		Leverage:      float64(p.leverage),
		UnrealizedPnl: float64(p.unrealized_pnl),
		MarginMode:    MarginMode(p.margin_mode),
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

// --- derivatives ---

// MarginMode is the margin mode of a derivatives position.
type MarginMode int32

// Margin modes.
const (
	MarginCross    MarginMode = C.WICKRA_MARGIN_CROSS
	MarginIsolated MarginMode = C.WICKRA_MARGIN_ISOLATED
)

// PositionSide is the direction of a position.
type PositionSide int32

// Position sides.
const (
	PositionLong  PositionSide = C.WICKRA_POSITION_LONG
	PositionShort PositionSide = C.WICKRA_POSITION_SHORT
)

// Position is an open derivatives position.
type Position struct {
	Symbol        string
	Side          PositionSide
	Quantity      float64
	EntryPrice    float64
	MarkPrice     float64
	Leverage      float64
	UnrealizedPnl float64
	MarginMode    MarginMode
}

// Derivatives is a live futures client (positions, leverage, margin, close).
// Construct with ConnectDerivatives; call Close to release native resources.
type Derivatives struct {
	handle *C.WickraDerivatives
}

// ConnectDerivatives opens a USDⓈ-M futures client for name. It fails for a
// spot-only venue (coinbase, upbit).
func ConnectDerivatives(name, apiKey, apiSecret, passphrase, privateKey string, testnet bool) (*Derivatives, error) {
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
	handle := C.wickra_connect_derivatives(cName, cKey, cSecret, cPass, cPriv, C.bool(testnet))
	if handle == nil {
		return nil, fmt.Errorf("wickra: failed to connect derivatives client for %s", name)
	}
	d := &Derivatives{handle: handle}
	runtime.SetFinalizer(d, (*Derivatives).Close)
	return d, nil
}

// Position returns the open position in market. The error wraps
// WICKRA_ERR_NOT_FOUND when the position is flat.
func (d *Derivatives) Position(market string) (Position, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	var out C.WickraPosition
	if err := codeError(C.wickra_derivatives_position(d.handle, cMarket, &out)); err != nil {
		return Position{}, err
	}
	return readPosition(&out), nil
}

// Positions lists every open position. Pass a market to scope to one symbol, or
// "" for all. It grows its buffer and re-queries if the venue reports more
// positions than fit.
func (d *Derivatives) Positions(market string) ([]Position, error) {
	var cMarket *C.char
	if market != "" {
		cMarket = C.CString(market)
		defer C.free(unsafe.Pointer(cMarket))
	}
	capN := 16
	for {
		buf := make([]C.WickraPosition, capN)
		rc := C.wickra_derivatives_positions(d.handle, cMarket, &buf[0], C.uintptr_t(capN))
		if rc < 0 {
			return nil, fmt.Errorf("wickra: positions failed with code %d", int(rc))
		}
		total := int(rc)
		if total > capN {
			capN = total
			continue
		}
		positions := make([]Position, total)
		for i := 0; i < total; i++ {
			positions[i] = readPosition(&buf[i])
		}
		return positions, nil
	}
}

// SetLeverage sets the leverage for market.
func (d *Derivatives) SetLeverage(market string, leverage uint32) error {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	return codeError(C.wickra_derivatives_set_leverage(d.handle, cMarket, C.uint32_t(leverage)))
}

// SetMarginMode sets the margin mode for market.
func (d *Derivatives) SetMarginMode(market string, mode MarginMode) error {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	return codeError(C.wickra_derivatives_set_margin_mode(d.handle, cMarket, C.int(mode)))
}

// ClosePosition flattens the open position in market with a reduce-only market order.
func (d *Derivatives) ClosePosition(market string) (Order, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	var out C.WickraOrder
	if err := codeError(C.wickra_derivatives_close_position(d.handle, cMarket, &out)); err != nil {
		return Order{}, err
	}
	return readOrder(&out), nil
}

// Close releases the native derivatives handle.
func (d *Derivatives) Close() {
	if d.handle != nil {
		C.wickra_derivatives_free(d.handle)
		d.handle = nil
		runtime.SetFinalizer(d, nil)
	}
}

// --- advanced orders ---

// Advanced is a live advanced-orders client (amend, batch cancel). Construct
// with ConnectAdvanced; call Close to release native resources.
type Advanced struct {
	handle *C.WickraAdvanced
}

// ConnectAdvanced opens an advanced-orders client for name. futures selects the
// USDⓈ-M futures market. It fails for a venue without an advanced-order surface.
func ConnectAdvanced(name, apiKey, apiSecret, passphrase, privateKey string, testnet, futures bool) (*Advanced, error) {
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
	handle := C.wickra_connect_advanced(cName, cKey, cSecret, cPass, cPriv, C.bool(testnet), C.bool(futures))
	if handle == nil {
		return nil, fmt.Errorf("wickra: failed to connect advanced-orders client for %s", name)
	}
	a := &Advanced{handle: handle}
	runtime.SetFinalizer(a, (*Advanced).Close)
	return a, nil
}

// AmendOrder amends a resting order's price and/or quantity in place. Pass a NaN
// for newPrice or newQuantity to leave that field unchanged.
func (a *Advanced) AmendOrder(market, orderID string, newPrice, newQuantity float64) (Order, error) {
	cMarket := C.CString(market)
	cOrder := C.CString(orderID)
	defer C.free(unsafe.Pointer(cMarket))
	defer C.free(unsafe.Pointer(cOrder))
	var out C.WickraOrder
	rc := C.wickra_advanced_amend_order(a.handle, cMarket, cOrder, C.double(newPrice), C.double(newQuantity), &out)
	if err := codeError(rc); err != nil {
		return Order{}, err
	}
	return readOrder(&out), nil
}

// OrderRequest describes one order for PlaceBatch. A NaN Price places a market
// order; a finite Price places a limit order.
type OrderRequest struct {
	Market   string
	Side     Side
	Quantity float64
	Price    float64
}

// BatchResult is one order's outcome in a batch placement: Err is nil and Order
// is populated on success, or Err is set and Order is zero on a per-order
// rejection.
type BatchResult struct {
	Order Order
	Err   error
}

// PlaceOco places a one-cancels-other bracket on market: a take-profit limit leg
// at price paired with a stop leg triggered at stopPrice. A finite stopLimitPrice
// makes the stop leg a stop-limit; pass NaN to leave it a stop-market. It returns
// the resulting order legs.
func (a *Advanced) PlaceOco(market string, side Side, quantity, price, stopPrice, stopLimitPrice float64) ([]Order, error) {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	capN := 4
	for {
		buf := make([]C.WickraOrder, capN)
		rc := C.wickra_advanced_place_oco(
			a.handle, cMarket, C.int32_t(side),
			C.double(quantity), C.double(price), C.double(stopPrice), C.double(stopLimitPrice),
			&buf[0], C.uintptr_t(capN))
		if rc < 0 {
			return nil, fmt.Errorf("wickra: place_oco failed with code %d", int(rc))
		}
		total := int(rc)
		if total > capN {
			capN = total
			continue
		}
		orders := make([]Order, total)
		for i := 0; i < total; i++ {
			orders[i] = readOrder(&buf[i])
		}
		return orders, nil
	}
}

// PlaceBatch places several orders in one request. It returns one BatchResult per
// request, in the same order: a whole-request failure (auth, transport) is
// returned as the error, while a per-order rejection surfaces in that result's
// Err.
func (a *Advanced) PlaceBatch(requests []OrderRequest) ([]BatchResult, error) {
	n := len(requests)
	if n == 0 {
		return nil, nil
	}
	markets := make([]*C.char, n)
	sides := make([]C.int32_t, n)
	quantities := make([]C.double, n)
	prices := make([]C.double, n)
	for i, req := range requests {
		markets[i] = C.CString(req.Market)
		sides[i] = C.int32_t(req.Side)
		quantities[i] = C.double(req.Quantity)
		prices[i] = C.double(req.Price)
	}
	defer func() {
		for _, p := range markets {
			C.free(unsafe.Pointer(p))
		}
	}()
	out := make([]C.WickraOrder, n)
	outCodes := make([]C.int32_t, n)
	rc := C.wickra_advanced_place_batch(
		a.handle,
		(**C.char)(unsafe.Pointer(&markets[0])),
		(*C.int32_t)(unsafe.Pointer(&sides[0])),
		(*C.double)(unsafe.Pointer(&quantities[0])),
		(*C.double)(unsafe.Pointer(&prices[0])),
		C.uintptr_t(n),
		&out[0], &outCodes[0], C.uintptr_t(n))
	if rc < 0 {
		return nil, fmt.Errorf("wickra: place_batch failed with code %d", int(rc))
	}
	total := int(rc)
	results := make([]BatchResult, total)
	for i := 0; i < total; i++ {
		if outCodes[i] == C.WICKRA_OK {
			results[i] = BatchResult{Order: readOrder(&out[i])}
		} else {
			results[i] = BatchResult{Err: fmt.Errorf("wickra: order rejected with code %d", int(outCodes[i]))}
		}
	}
	return results, nil
}

// CancelBatch cancels several orders on market in one request.
func (a *Advanced) CancelBatch(market string, orderIDs []string) error {
	cMarket := C.CString(market)
	defer C.free(unsafe.Pointer(cMarket))
	ids := make([]*C.char, 0, len(orderIDs))
	for _, id := range orderIDs {
		ids = append(ids, C.CString(id))
	}
	defer func() {
		for _, p := range ids {
			C.free(unsafe.Pointer(p))
		}
	}()
	return codeError(C.wickra_advanced_cancel_batch(a.handle, cMarket, assetsPtr(ids), C.size_t(len(ids))))
}

// Close releases the native advanced-orders handle.
func (a *Advanced) Close() {
	if a.handle != nil {
		C.wickra_advanced_free(a.handle)
		a.handle = nil
		runtime.SetFinalizer(a, nil)
	}
}
