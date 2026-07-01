//! C ABI for `wickra-exchange` — the hub every C-capable language (C, C++, C#,
//! Go, Java, R) links against.
//!
//! The client is an opaque handle ([`WickraExchange`]) constructed as a paper,
//! replay or live backend; every call returns an [`i32`] status code
//! ([`WICKRA_OK`] = 0, negative on error). Results are written into caller-owned,
//! `#[repr(C)]` out-parameters ([`WickraOrder`], [`WickraEvent`]) — no memory
//! crosses the boundary except the opaque handle, which must be released with
//! [`wickra_exchange_free`]. Panics abort (release profile is built with
//! `panic = "abort"`), so nothing unwinds across the boundary.
//!
//! The header `include/wickra_exchange.h` is generated from this file by cbindgen
//! and committed; regenerate and commit it whenever this ABI changes.

use core::ffi::{c_char, CStr};
use core::slice;
use std::ffi::CString;
use std::sync::OnceLock;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use wickra_exchange::{
    connect, connect_advanced, connect_derivatives, AdvancedOrders, Credentials, Derivatives,
    Error, Event, Exchange, ExchangeOptions, MarginMode, MarketType, Order, OrderRequest,
    OrderSide, OrderStatus, PaperExchange, PositionSide, ReplayExchange, Symbol, TradePrint,
};

/// Success.
pub const WICKRA_OK: i32 = 0;
/// A required pointer argument was null.
pub const WICKRA_ERR_NULL: i32 = -1;
/// An argument was invalid (bad market string, non-finite number, bad UTF-8).
pub const WICKRA_ERR_INVALID_ARG: i32 = -2;
/// The operation is not supported on this backend (e.g. `set_price` off paper).
pub const WICKRA_ERR_UNSUPPORTED: i32 = -3;
/// The account had insufficient balance for the request.
pub const WICKRA_ERR_INSUFFICIENT_BALANCE: i32 = -4;
/// The referenced order or entity was not found.
pub const WICKRA_ERR_NOT_FOUND: i32 = -5;
/// The order was rejected by the venue / simulator.
pub const WICKRA_ERR_REJECTED: i32 = -6;
/// Any other error reported by the exchange layer.
pub const WICKRA_ERR_OTHER: i32 = -7;

/// Order side: buy.
pub const WICKRA_SIDE_BUY: i32 = 0;
/// Order side: sell.
pub const WICKRA_SIDE_SELL: i32 = 1;

/// Margin mode: cross (margin shared across positions).
pub const WICKRA_MARGIN_CROSS: i32 = 0;
/// Margin mode: isolated (margin isolated per position).
pub const WICKRA_MARGIN_ISOLATED: i32 = 1;

/// Position side: long.
pub const WICKRA_POSITION_LONG: i32 = 0;
/// Position side: short.
pub const WICKRA_POSITION_SHORT: i32 = 1;

/// Order status codes (mirror `OrderStatus`).
pub const WICKRA_STATUS_NEW: i32 = 0;
pub const WICKRA_STATUS_PARTIALLY_FILLED: i32 = 1;
pub const WICKRA_STATUS_FILLED: i32 = 2;
pub const WICKRA_STATUS_CANCELED: i32 = 3;
pub const WICKRA_STATUS_REJECTED: i32 = 4;
pub const WICKRA_STATUS_EXPIRED: i32 = 5;

/// Stream event kinds.
pub const WICKRA_EVENT_TRADE: i32 = 0;
pub const WICKRA_EVENT_TICKER: i32 = 1;
pub const WICKRA_EVENT_ORDER_UPDATE: i32 = 2;
pub const WICKRA_EVENT_BALANCE_UPDATE: i32 = 3;
pub const WICKRA_EVENT_SUBSCRIBED: i32 = 4;
pub const WICKRA_EVENT_OTHER: i32 = 5;

/// Fixed capacity (including the NUL terminator) of the C-string fields in the
/// `#[repr(C)]` result structs.
pub const WICKRA_STR_CAP: usize = 64;

/// An order as reported by the exchange (C-ABI mirror of `Order`).
#[repr(C)]
pub struct WickraOrder {
    /// Venue order id, NUL-terminated (truncated to `WICKRA_STR_CAP - 1` bytes).
    pub id: [c_char; WICKRA_STR_CAP],
    /// `WICKRA_SIDE_*`.
    pub side: i32,
    /// `WICKRA_STATUS_*`.
    pub status: i32,
    /// Total ordered quantity.
    pub quantity: f64,
    /// Quantity filled so far.
    pub filled_quantity: f64,
    /// Limit price, or `NaN` if none.
    pub price: f64,
    /// Average fill price, or `NaN` if none.
    pub average_price: f64,
}

/// A single stream event (C-ABI projection of `Event`).
#[repr(C)]
pub struct WickraEvent {
    /// `WICKRA_EVENT_*`.
    pub kind: i32,
    /// Market symbol for trade/ticker events, NUL-terminated (empty otherwise).
    pub symbol: [c_char; WICKRA_STR_CAP],
    /// Price for trade/ticker events (`NaN` otherwise).
    pub price: f64,
    /// Quantity for trade events (`NaN` otherwise).
    pub quantity: f64,
    /// `WICKRA_SIDE_*` for trade events (`-1` otherwise).
    pub side: i32,
    /// The order for `order_update` events.
    pub order: WickraOrder,
}

/// The concrete backend behind a handle.
enum Inner {
    Paper(PaperExchange),
    Replay(ReplayExchange),
    Live(Box<dyn Exchange>),
}

impl Inner {
    fn as_exchange(&mut self) -> &mut dyn Exchange {
        match self {
            Inner::Paper(paper) => paper,
            Inner::Replay(replay) => replay,
            Inner::Live(live) => live.as_mut(),
        }
    }
}

/// An opaque exchange handle. Construct with `wickra_paper_new` /
/// `wickra_replay_new` / `wickra_connect`; release with `wickra_exchange_free`.
pub struct WickraExchange {
    inner: Inner,
}

/// A derivatives position (C-ABI mirror of `Position`).
#[repr(C)]
pub struct WickraPosition {
    /// Market symbol, NUL-terminated (`base/quote`).
    pub symbol: [c_char; WICKRA_STR_CAP],
    /// `WICKRA_POSITION_*` (long / short).
    pub side: i32,
    /// Position size in base units (magnitude, always positive).
    pub quantity: f64,
    /// Average entry price.
    pub entry_price: f64,
    /// Current mark price (may be `0` where the venue omits it).
    pub mark_price: f64,
    /// Account leverage for this position.
    pub leverage: f64,
    /// Unrealized PnL as reported by the venue.
    pub unrealized_pnl: f64,
    /// `WICKRA_MARGIN_*` (cross / isolated).
    pub margin_mode: i32,
}

/// An opaque derivatives handle over a live futures client. Construct with
/// `wickra_connect_derivatives`; release with `wickra_derivatives_free`.
pub struct WickraDerivatives {
    inner: Box<dyn Derivatives>,
}

/// An opaque advanced-orders handle over a live client. Construct with
/// `wickra_connect_advanced`; release with `wickra_advanced_free`.
pub struct WickraAdvanced {
    inner: Box<dyn AdvancedOrders>,
}

// ------------------------------- helpers -------------------------------------

fn error_code(error: &Error) -> i32 {
    match error {
        Error::InvalidSymbol(_) | Error::InvalidOrder(_) | Error::InvalidCredentials(_) => {
            WICKRA_ERR_INVALID_ARG
        }
        Error::UnsupportedExchange(_) => WICKRA_ERR_UNSUPPORTED,
        Error::InsufficientBalance => WICKRA_ERR_INSUFFICIENT_BALANCE,
        Error::NotFound(_) => WICKRA_ERR_NOT_FOUND,
        Error::OrderRejected { .. } => WICKRA_ERR_REJECTED,
        _ => WICKRA_ERR_OTHER,
    }
}

fn side_code(side: OrderSide) -> i32 {
    match side {
        OrderSide::Buy => WICKRA_SIDE_BUY,
        OrderSide::Sell => WICKRA_SIDE_SELL,
    }
}

fn status_code(status: OrderStatus) -> i32 {
    match status {
        OrderStatus::New => WICKRA_STATUS_NEW,
        OrderStatus::PartiallyFilled => WICKRA_STATUS_PARTIALLY_FILLED,
        OrderStatus::Filled => WICKRA_STATUS_FILLED,
        OrderStatus::Canceled => WICKRA_STATUS_CANCELED,
        OrderStatus::Rejected => WICKRA_STATUS_REJECTED,
        OrderStatus::Expired => WICKRA_STATUS_EXPIRED,
    }
}

fn to_float(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

/// Read a NUL-terminated C string as `&str`, or `None` on null / bad UTF-8.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated C string.
unsafe fn opt_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Write `value` into a C string buffer, truncating to fit and always
/// NUL-terminating.
fn write_cstr(dst: &mut [c_char], value: &str) {
    for slot in dst.iter_mut() {
        *slot = 0;
    }
    if dst.is_empty() {
        return;
    }
    let bytes = value.as_bytes();
    let copy = bytes.len().min(dst.len() - 1);
    for (slot, &byte) in dst.iter_mut().zip(&bytes[..copy]) {
        *slot = byte as c_char;
    }
}

fn empty_order() -> WickraOrder {
    WickraOrder {
        id: [0; WICKRA_STR_CAP],
        side: -1,
        status: -1,
        quantity: f64::NAN,
        filled_quantity: f64::NAN,
        price: f64::NAN,
        average_price: f64::NAN,
    }
}

fn fill_order(dst: &mut WickraOrder, order: &Order) {
    write_cstr(&mut dst.id, &order.id);
    dst.side = side_code(order.side);
    dst.status = status_code(order.status);
    dst.quantity = to_float(order.quantity);
    dst.filled_quantity = to_float(order.filled_quantity);
    dst.price = order.price.map_or(f64::NAN, to_float);
    dst.average_price = order.average_price.map_or(f64::NAN, to_float);
}

fn margin_mode_from_code(code: i32) -> Option<MarginMode> {
    match code {
        WICKRA_MARGIN_CROSS => Some(MarginMode::Cross),
        WICKRA_MARGIN_ISOLATED => Some(MarginMode::Isolated),
        _ => None,
    }
}

fn position_side_code(side: PositionSide) -> i32 {
    match side {
        PositionSide::Long => WICKRA_POSITION_LONG,
        PositionSide::Short => WICKRA_POSITION_SHORT,
    }
}

/// Collect `(asset, amount)` pairs from parallel C arrays into a paper account.
///
/// # Safety
/// `assets`/`amounts` must each point to `n` valid elements (or be null when
/// `n == 0`).
unsafe fn seed_balances(
    mut paper: PaperExchange,
    assets: *const *const c_char,
    amounts: *const f64,
    n: usize,
) -> Option<PaperExchange> {
    for i in 0..n {
        let asset_ptr = unsafe { *assets.add(i) };
        let asset = unsafe { opt_str(asset_ptr) }?;
        let amount = unsafe { *amounts.add(i) };
        paper = paper.with_balance(asset, Decimal::from_f64_retain(amount)?);
    }
    Some(paper)
}

fn parse_symbol(market: &str) -> Option<Symbol> {
    match market.split_once('/') {
        Some((base, quote)) if !base.is_empty() && !quote.is_empty() => {
            Some(Symbol::new(base, quote))
        }
        _ => None,
    }
}

/// Collect `n` NUL-terminated C strings from a `*const *const c_char` array into
/// owned `String`s. Returns `None` if any element is null or not valid UTF-8.
///
/// # Safety
/// `ptr` must point to `n` valid elements (or be null when `n == 0`).
unsafe fn collect_cstrs(ptr: *const *const c_char, n: usize) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let item = unsafe { *ptr.add(i) };
        out.push(unsafe { opt_str(item) }?.to_string());
    }
    Some(out)
}

/// Interpret an `f64` argument where `NaN` means "leave unchanged": `NaN` ->
/// `Ok(None)`, a finite value -> `Ok(Some(decimal))`, non-finite -> `Err(())`.
fn opt_decimal_arg(value: f64) -> Result<Option<Decimal>, ()> {
    if value.is_nan() {
        return Ok(None);
    }
    Decimal::from_f64_retain(value).map(Some).ok_or(())
}

fn paper_with_costs(maker_bps: f64, taker_bps: f64, slippage_bps: f64) -> Option<PaperExchange> {
    Some(
        PaperExchange::new()
            .with_fees(
                Decimal::from_f64_retain(maker_bps)?,
                Decimal::from_f64_retain(taker_bps)?,
            )
            .with_slippage_bps(Decimal::from_f64_retain(slippage_bps)?),
    )
}

// ------------------------------- exports -------------------------------------

/// The library version as a static NUL-terminated string.
#[no_mangle]
pub extern "C" fn wickra_version() -> *const c_char {
    static VERSION: OnceLock<CString> = OnceLock::new();
    VERSION
        .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap())
        .as_ptr()
}

/// Construct an offline paper account seeded from parallel `assets`/`amounts`
/// arrays (length `n_balances`). Returns null on invalid arguments.
///
/// # Safety
/// The array pointers must be valid for `n_balances` elements (or null when it
/// is zero).
#[no_mangle]
pub unsafe extern "C" fn wickra_paper_new(
    assets: *const *const c_char,
    amounts: *const f64,
    n_balances: usize,
    maker_bps: f64,
    taker_bps: f64,
    slippage_bps: f64,
) -> *mut WickraExchange {
    let Some(paper) = paper_with_costs(maker_bps, taker_bps, slippage_bps) else {
        return core::ptr::null_mut();
    };
    let Some(paper) = (unsafe { seed_balances(paper, assets, amounts, n_balances) }) else {
        return core::ptr::null_mut();
    };
    Box::into_raw(Box::new(WickraExchange {
        inner: Inner::Paper(paper),
    }))
}

/// Construct a replay account driven by a `tape` of `n_tape` trade prices for
/// `market`, filling a paper book seeded from `assets`/`amounts`. Returns null on
/// invalid arguments.
///
/// # Safety
/// `market` must be a valid C string; the array pointers must be valid for their
/// stated lengths (or null when zero).
#[no_mangle]
pub unsafe extern "C" fn wickra_replay_new(
    market: *const c_char,
    tape: *const f64,
    n_tape: usize,
    assets: *const *const c_char,
    amounts: *const f64,
    n_balances: usize,
    maker_bps: f64,
    taker_bps: f64,
    slippage_bps: f64,
) -> *mut WickraExchange {
    let Some(market) = (unsafe { opt_str(market) }) else {
        return core::ptr::null_mut();
    };
    let Some(symbol) = parse_symbol(market) else {
        return core::ptr::null_mut();
    };
    let Some(paper) = paper_with_costs(maker_bps, taker_bps, slippage_bps) else {
        return core::ptr::null_mut();
    };
    let Some(paper) = (unsafe { seed_balances(paper, assets, amounts, n_balances) }) else {
        return core::ptr::null_mut();
    };
    let mut frames = Vec::with_capacity(n_tape);
    for i in 0..n_tape {
        let price = unsafe { *tape.add(i) };
        let Some(price) = Decimal::from_f64_retain(price) else {
            return core::ptr::null_mut();
        };
        frames.push(Event::Trade(TradePrint {
            symbol: symbol.clone(),
            price,
            quantity: Decimal::ONE,
            aggressor: OrderSide::Buy,
            timestamp: i64::try_from(i).unwrap_or(i64::MAX),
        }));
    }
    Box::into_raw(Box::new(WickraExchange {
        inner: Inner::Replay(ReplayExchange::with_paper(frames, paper)),
    }))
}

/// Connect a live client for `name`, authenticated with the given credentials
/// (`passphrase`/`private_key` may be null). Returns null on failure.
///
/// # Safety
/// The non-null string arguments must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_connect(
    name: *const c_char,
    api_key: *const c_char,
    api_secret: *const c_char,
    passphrase: *const c_char,
    private_key: *const c_char,
    testnet: bool,
) -> *mut WickraExchange {
    let (Some(name), Some(api_key), Some(api_secret)) =
        (unsafe { (opt_str(name), opt_str(api_key), opt_str(api_secret)) })
    else {
        return core::ptr::null_mut();
    };
    let mut credentials = Credentials::new(api_key, api_secret);
    if let Some(passphrase) = unsafe { opt_str(passphrase) } {
        credentials = credentials.with_passphrase(passphrase);
    }
    if let Some(private_key) = unsafe { opt_str(private_key) } {
        credentials = credentials.with_private_key(private_key);
    }
    let options = if testnet {
        ExchangeOptions::testnet(MarketType::Spot)
    } else {
        ExchangeOptions::mainnet(MarketType::Spot)
    };
    match connect(name, credentials, &options) {
        Ok(live) => Box::into_raw(Box::new(WickraExchange {
            inner: Inner::Live(live),
        })),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Release an exchange handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null or a pointer returned by a `wickra_*_new` / `wickra_connect`
/// constructor, freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_free(handle: *mut WickraExchange) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Write the venue name into `out` (capacity `cap`, always NUL-terminated).
///
/// # Safety
/// `handle` must be valid; `out` must be writable for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_name(
    handle: *const WickraExchange,
    out: *mut c_char,
    cap: usize,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_ref() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() || cap == 0 {
        return WICKRA_ERR_NULL;
    }
    let name = match &exchange.inner {
        Inner::Paper(paper) => paper.name(),
        Inner::Replay(replay) => replay.name(),
        Inner::Live(live) => live.name(),
    };
    let dst = unsafe { slice::from_raw_parts_mut(out, cap) };
    write_cstr(dst, name);
    WICKRA_OK
}

/// Set the mark price a paper account fills against (paper backend only).
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_set_price(
    handle: *mut WickraExchange,
    market: *const c_char,
    price: f64,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(price) = Decimal::from_f64_retain(price) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match &mut exchange.inner {
        Inner::Paper(paper) => {
            paper.set_price(&symbol, price);
            WICKRA_OK
        }
        _ => WICKRA_ERR_UNSUPPORTED,
    }
}

/// Place a market order (`side` is `WICKRA_SIDE_*`); writes the resulting order
/// into `out`.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_place_market(
    handle: *mut WickraExchange,
    market: *const c_char,
    side: i32,
    quantity: f64,
    out: *mut WickraOrder,
) -> i32 {
    place(handle, market, side, quantity, None, out)
}

/// Place a limit order (`side` is `WICKRA_SIDE_*`) at `price`; writes the
/// resulting order into `out`.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_place_limit(
    handle: *mut WickraExchange,
    market: *const c_char,
    side: i32,
    quantity: f64,
    price: f64,
    out: *mut WickraOrder,
) -> i32 {
    place(handle, market, side, quantity, Some(price), out)
}

fn place(
    handle: *mut WickraExchange,
    market: *const c_char,
    side: i32,
    quantity: f64,
    price: Option<f64>,
    out: *mut WickraOrder,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() {
        return WICKRA_ERR_NULL;
    }
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(quantity) = Decimal::from_f64_retain(quantity) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let request = match (side, price) {
        (WICKRA_SIDE_BUY, None) => OrderRequest::market_buy(symbol, quantity),
        (WICKRA_SIDE_SELL, None) => OrderRequest::market_sell(symbol, quantity),
        (WICKRA_SIDE_BUY, Some(price)) => {
            let Some(price) = Decimal::from_f64_retain(price) else {
                return WICKRA_ERR_INVALID_ARG;
            };
            OrderRequest::limit_buy(symbol, quantity, price)
        }
        (WICKRA_SIDE_SELL, Some(price)) => {
            let Some(price) = Decimal::from_f64_retain(price) else {
                return WICKRA_ERR_INVALID_ARG;
            };
            OrderRequest::limit_sell(symbol, quantity, price)
        }
        _ => return WICKRA_ERR_INVALID_ARG,
    };
    match exchange.inner.as_exchange().place_order(&request) {
        Ok(order) => {
            unsafe { fill_order(&mut *out, &order) };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Cancel an open order by venue id.
///
/// # Safety
/// `handle` must be valid; `market` and `order_id` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_cancel(
    handle: *mut WickraExchange,
    market: *const c_char,
    order_id: *const c_char,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let (Some(market), Some(order_id)) = (unsafe { (opt_str(market), opt_str(order_id)) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match exchange.inner.as_exchange().cancel_order(&symbol, order_id) {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Write the free balance of `asset` into `*out_free`.
///
/// # Safety
/// `handle` and `out_free` must be valid; `asset` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_balance(
    handle: *mut WickraExchange,
    asset: *const c_char,
    out_free: *mut f64,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out_free.is_null() {
        return WICKRA_ERR_NULL;
    }
    let Some(asset) = (unsafe { opt_str(asset) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match exchange.inner.as_exchange().balances() {
        Ok(balances) => {
            let free = balances
                .iter()
                .find(|b| b.asset == asset)
                .map_or(0.0, |b| to_float(b.free));
            unsafe { *out_free = free };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Drain buffered events into `out` (capacity `cap`). Returns the number written
/// (`>= 0`) or a negative error code.
///
/// # Safety
/// `handle` must be valid; `out` must be writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_poll(
    handle: *mut WickraExchange,
    out: *mut WickraEvent,
    cap: usize,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() {
        return WICKRA_ERR_NULL;
    }
    let events = exchange.inner.as_exchange().poll_events();
    let count = events.len().min(cap);
    for (i, event) in events.iter().take(count).enumerate() {
        let slot = unsafe { &mut *out.add(i) };
        fill_event(slot, event);
    }
    i32::try_from(count).unwrap_or(i32::MAX)
}

fn fill_event(slot: &mut WickraEvent, event: &Event) {
    slot.kind = WICKRA_EVENT_OTHER;
    slot.symbol = [0; WICKRA_STR_CAP];
    slot.price = f64::NAN;
    slot.quantity = f64::NAN;
    slot.side = -1;
    slot.order = empty_order();
    match event {
        Event::Trade(trade) => {
            slot.kind = WICKRA_EVENT_TRADE;
            write_cstr(&mut slot.symbol, &trade.symbol.to_string());
            slot.price = to_float(trade.price);
            slot.quantity = to_float(trade.quantity);
            slot.side = side_code(trade.aggressor);
        }
        Event::Ticker(ticker) => {
            slot.kind = WICKRA_EVENT_TICKER;
            write_cstr(&mut slot.symbol, &ticker.symbol.to_string());
            slot.price = to_float(ticker.last);
        }
        Event::OrderUpdate(order) => {
            slot.kind = WICKRA_EVENT_ORDER_UPDATE;
            fill_order(&mut slot.order, order);
        }
        Event::BalanceUpdate(_) => slot.kind = WICKRA_EVENT_BALANCE_UPDATE,
        Event::Subscribed { .. } => slot.kind = WICKRA_EVENT_SUBSCRIBED,
        _ => slot.kind = WICKRA_EVENT_OTHER,
    }
}

// ------------------------- derivatives (futures) -----------------------------

/// Connect a live **derivatives** (USDⓈ-M futures) client for `name`, returning
/// an opaque [`WickraDerivatives`] handle (positions / leverage / margin / close).
/// Returns null on failure or for a spot-only venue (`coinbase`, `upbit`).
///
/// # Safety
/// The non-null string arguments must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_connect_derivatives(
    name: *const c_char,
    api_key: *const c_char,
    api_secret: *const c_char,
    passphrase: *const c_char,
    private_key: *const c_char,
    testnet: bool,
) -> *mut WickraDerivatives {
    let (Some(name), Some(api_key), Some(api_secret)) =
        (unsafe { (opt_str(name), opt_str(api_key), opt_str(api_secret)) })
    else {
        return core::ptr::null_mut();
    };
    let mut credentials = Credentials::new(api_key, api_secret);
    if let Some(passphrase) = unsafe { opt_str(passphrase) } {
        credentials = credentials.with_passphrase(passphrase);
    }
    if let Some(private_key) = unsafe { opt_str(private_key) } {
        credentials = credentials.with_private_key(private_key);
    }
    let options = if testnet {
        ExchangeOptions::testnet(MarketType::UsdMFutures)
    } else {
        ExchangeOptions::mainnet(MarketType::UsdMFutures)
    };
    match connect_derivatives(name, credentials, &options) {
        Ok(inner) => Box::into_raw(Box::new(WickraDerivatives { inner })),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Release a derivatives handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null or a pointer from `wickra_connect_derivatives`, freed
/// exactly once.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_free(handle: *mut WickraDerivatives) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Write the open position in `market` into `out`. Returns
/// [`WICKRA_ERR_NOT_FOUND`] if the position is flat.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_position(
    handle: *mut WickraDerivatives,
    market: *const c_char,
    out: *mut WickraPosition,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() {
        return WICKRA_ERR_NULL;
    }
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.positions(Some(&symbol)) {
        Ok(positions) => match positions.into_iter().find(|p| p.symbol == symbol) {
            Some(position) => {
                let dst = unsafe { &mut *out };
                write_cstr(&mut dst.symbol, &position.symbol.to_string());
                dst.side = position_side_code(position.side);
                dst.quantity = to_float(position.quantity);
                dst.entry_price = to_float(position.entry_price);
                dst.mark_price = to_float(position.mark_price);
                dst.leverage = to_float(position.leverage);
                dst.unrealized_pnl = to_float(position.unrealized_pnl);
                dst.margin_mode = match position.margin_mode {
                    MarginMode::Cross => WICKRA_MARGIN_CROSS,
                    MarginMode::Isolated => WICKRA_MARGIN_ISOLATED,
                };
                WICKRA_OK
            }
            None => WICKRA_ERR_NOT_FOUND,
        },
        Err(e) => error_code(&e),
    }
}

/// Set the leverage for `market`.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_set_leverage(
    handle: *mut WickraDerivatives,
    market: *const c_char,
    leverage: u32,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.set_leverage(&symbol, leverage) {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Set the margin mode for `market` (`mode` is `WICKRA_MARGIN_*`).
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_set_margin_mode(
    handle: *mut WickraDerivatives,
    market: *const c_char,
    mode: i32,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(mode) = margin_mode_from_code(mode) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.set_margin_mode(&symbol, mode) {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Flatten the open position in `market` with a reduce-only market order; writes
/// the resulting order into `out`.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_close_position(
    handle: *mut WickraDerivatives,
    market: *const c_char,
    out: *mut WickraOrder,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() {
        return WICKRA_ERR_NULL;
    }
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.close_position(&symbol) {
        Ok(order) => {
            unsafe { fill_order(&mut *out, &order) };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

// --------------------------- advanced orders ---------------------------------

/// Connect a live client for `name` as an advanced-orders handle (amend / batch
/// cancel). `futures` selects the USDⓈ-M futures market. Returns null on failure
/// or for a venue without an advanced-order surface (`coinbase`, `upbit`).
///
/// # Safety
/// The non-null string arguments must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_connect_advanced(
    name: *const c_char,
    api_key: *const c_char,
    api_secret: *const c_char,
    passphrase: *const c_char,
    private_key: *const c_char,
    testnet: bool,
    futures: bool,
) -> *mut WickraAdvanced {
    let (Some(name), Some(api_key), Some(api_secret)) =
        (unsafe { (opt_str(name), opt_str(api_key), opt_str(api_secret)) })
    else {
        return core::ptr::null_mut();
    };
    let mut credentials = Credentials::new(api_key, api_secret);
    if let Some(passphrase) = unsafe { opt_str(passphrase) } {
        credentials = credentials.with_passphrase(passphrase);
    }
    if let Some(private_key) = unsafe { opt_str(private_key) } {
        credentials = credentials.with_private_key(private_key);
    }
    let market_type = if futures {
        MarketType::UsdMFutures
    } else {
        MarketType::Spot
    };
    let options = if testnet {
        ExchangeOptions::testnet(market_type)
    } else {
        ExchangeOptions::mainnet(market_type)
    };
    match connect_advanced(name, credentials, &options) {
        Ok(inner) => Box::into_raw(Box::new(WickraAdvanced { inner })),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Release an advanced-orders handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null or a pointer from `wickra_connect_advanced`, freed
/// exactly once.
#[no_mangle]
pub unsafe extern "C" fn wickra_advanced_free(handle: *mut WickraAdvanced) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Amend a resting order's price and/or quantity in place; writes the refreshed
/// order into `out`. Pass `NaN` for `new_price` / `new_quantity` to leave that
/// field unchanged. Returns [`WICKRA_ERR_UNSUPPORTED`] on a venue without a
/// native amend.
///
/// # Safety
/// `handle` and `out` must be valid; `market` and `order_id` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_advanced_amend_order(
    handle: *mut WickraAdvanced,
    market: *const c_char,
    order_id: *const c_char,
    new_price: f64,
    new_quantity: f64,
    out: *mut WickraOrder,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() {
        return WICKRA_ERR_NULL;
    }
    let (Some(market), Some(order_id)) = (unsafe { (opt_str(market), opt_str(order_id)) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let (Ok(price), Ok(quantity)) = (opt_decimal_arg(new_price), opt_decimal_arg(new_quantity))
    else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.amend_order(&symbol, order_id, price, quantity) {
        Ok(order) => {
            unsafe { fill_order(&mut *out, &order) };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Cancel several orders on `market` in one request. `order_ids` is an array of
/// `n` NUL-terminated C strings.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string; `order_ids` must
/// point to `n` valid C strings (or be null when `n == 0`).
#[no_mangle]
pub unsafe extern "C" fn wickra_advanced_cancel_batch(
    handle: *mut WickraAdvanced,
    market: *const c_char,
    order_ids: *const *const c_char,
    n: usize,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(ids) = (unsafe { collect_cstrs(order_ids, n) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.cancel_batch(&symbol, &ids) {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cstr(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    /// Read a NUL-terminated C-string field back into a Rust `String`.
    fn read_field(field: &[c_char]) -> String {
        let bytes: Vec<u8> = field
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn paper_place_and_balances_over_the_abi() {
        let market = cstr("BTC/USDT");
        let usdt = cstr("USDT");
        let btc = cstr("BTC");
        let assets = [usdt.as_ptr()];
        let amounts = [100_000.0_f64];

        unsafe {
            let ex = wickra_paper_new(assets.as_ptr(), amounts.as_ptr(), 1, 1.0, 5.0, 10.0);
            assert!(!ex.is_null());

            let mut name = [0_i8; 32];
            assert_eq!(wickra_exchange_name(ex, name.as_mut_ptr(), 32), WICKRA_OK);
            assert_eq!(read_field(&name), "paper");

            assert_eq!(
                wickra_exchange_set_price(ex, market.as_ptr(), 20_000.0),
                WICKRA_OK
            );

            let mut order = empty_order();
            let rc = wickra_exchange_place_market(
                ex,
                market.as_ptr(),
                WICKRA_SIDE_BUY,
                1.0,
                &raw mut order,
            );
            assert_eq!(rc, WICKRA_OK);
            assert_eq!(order.status, WICKRA_STATUS_FILLED);
            // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
            assert!((order.average_price - 20_020.0).abs() < 1e-6);
            assert!(read_field(&order.id).starts_with("paper-"));

            let mut free = 0.0;
            assert_eq!(
                wickra_exchange_balance(ex, btc.as_ptr(), &raw mut free),
                WICKRA_OK
            );
            assert!((free - 1.0).abs() < 1e-9);

            wickra_exchange_free(ex);
        }
    }

    #[test]
    fn set_price_on_replay_is_unsupported() {
        let market = cstr("BTC/USDT");
        let tape = [100.0_f64];
        unsafe {
            let ex = wickra_replay_new(
                market.as_ptr(),
                tape.as_ptr(),
                1,
                core::ptr::null(),
                core::ptr::null(),
                0,
                0.0,
                0.0,
                0.0,
            );
            assert!(!ex.is_null());
            assert_eq!(
                wickra_exchange_set_price(ex, market.as_ptr(), 1.0),
                WICKRA_ERR_UNSUPPORTED
            );
            wickra_exchange_free(ex);
        }
    }

    #[test]
    fn replay_parity_over_the_abi() {
        // A rising tape crosses a 3-period SMA; the market buy fills.
        let market = cstr("BTC/USDT");
        let usdt = cstr("USDT");
        let btc = cstr("BTC");
        let tape = [100.0_f64, 101.0, 102.0, 110.0, 112.0];
        let assets = [usdt.as_ptr()];
        let amounts = [100_000.0_f64];

        unsafe {
            let ex = wickra_replay_new(
                market.as_ptr(),
                tape.as_ptr(),
                tape.len(),
                assets.as_ptr(),
                amounts.as_ptr(),
                1,
                0.0,
                0.0,
                0.0,
            );
            assert!(!ex.is_null());

            let mut window = [0.0_f64; 3];
            let mut seen = 0usize;
            let mut bought = false;

            loop {
                let mut events: [WickraEvent; 8] = std::array::from_fn(|_| WickraEvent {
                    kind: 0,
                    symbol: [0; WICKRA_STR_CAP],
                    price: 0.0,
                    quantity: 0.0,
                    side: 0,
                    order: empty_order(),
                });
                let count = wickra_exchange_poll(ex, events.as_mut_ptr(), 8);
                if count <= 0 {
                    break;
                }
                for event in events.iter().take(count as usize) {
                    if event.kind != WICKRA_EVENT_TRADE {
                        continue;
                    }
                    window[seen % 3] = event.price;
                    seen += 1;
                    if seen >= 3 {
                        let mean = (window[0] + window[1] + window[2]) / 3.0;
                        if !bought && event.price > mean {
                            let mut order = empty_order();
                            let rc = wickra_exchange_place_market(
                                ex,
                                market.as_ptr(),
                                WICKRA_SIDE_BUY,
                                1.0,
                                &raw mut order,
                            );
                            assert_eq!(rc, WICKRA_OK);
                            assert_eq!(order.status, WICKRA_STATUS_FILLED);
                            bought = true;
                        }
                    }
                }
            }

            assert!(bought);
            let mut free = 0.0;
            wickra_exchange_balance(ex, btc.as_ptr(), &raw mut free);
            assert!((free - 1.0).abs() < 1e-9);
            wickra_exchange_free(ex);
        }
    }

    #[test]
    fn invalid_market_and_null_handle_are_rejected() {
        let bad = cstr("BTCUSDT");
        let usdt = cstr("USDT");
        let assets = [usdt.as_ptr()];
        let amounts = [1.0_f64];
        unsafe {
            // A market without '/' fails to parse -> null handle for replay.
            let ex = wickra_replay_new(
                bad.as_ptr(),
                core::ptr::null(),
                0,
                assets.as_ptr(),
                amounts.as_ptr(),
                1,
                0.0,
                0.0,
                0.0,
            );
            assert!(ex.is_null());

            // Null handle -> WICKRA_ERR_NULL.
            let mut out = 0.0;
            assert_eq!(
                wickra_exchange_balance(core::ptr::null_mut(), bad.as_ptr(), &raw mut out),
                WICKRA_ERR_NULL
            );
        }
    }

    #[test]
    fn version_is_exposed() {
        let ptr = wickra_version();
        assert!(!ptr.is_null());
        let version = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn derivatives_spot_only_venue_is_null() {
        let (name, key, secret) = (cstr("coinbase"), cstr("k"), cstr("s"));
        let handle = unsafe {
            wickra_connect_derivatives(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
            )
        };
        assert!(handle.is_null(), "coinbase has no derivatives market");
    }

    #[test]
    fn derivatives_null_handle_guards() {
        let market = cstr("BTC/USDT");
        assert_eq!(
            unsafe { wickra_derivatives_set_leverage(core::ptr::null_mut(), market.as_ptr(), 5) },
            WICKRA_ERR_NULL
        );
        assert_eq!(
            unsafe {
                wickra_derivatives_position(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    core::ptr::null_mut(),
                )
            },
            WICKRA_ERR_NULL
        );
        // Freeing null is a no-op.
        unsafe { wickra_derivatives_free(core::ptr::null_mut()) };
    }

    #[test]
    fn derivatives_bad_margin_mode_is_invalid_arg() {
        // Construction is offline (no socket until an RPC is issued), so a live
        // handle can be built and the argument validation exercised without a
        // network — an out-of-range margin mode is rejected before any request.
        let (name, key, secret) = (cstr("binance"), cstr("k"), cstr("s"));
        let handle = unsafe {
            wickra_connect_derivatives(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
            )
        };
        assert!(!handle.is_null());
        let market = cstr("BTC/USDT");
        assert_eq!(
            unsafe { wickra_derivatives_set_margin_mode(handle, market.as_ptr(), 99) },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_derivatives_free(handle) };
    }

    #[test]
    fn advanced_spot_only_venue_is_null() {
        let (name, key, secret) = (cstr("upbit"), cstr("k"), cstr("s"));
        let handle = unsafe {
            wickra_connect_advanced(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
                false,
            )
        };
        assert!(handle.is_null(), "upbit has no advanced-order surface");
    }

    #[test]
    fn advanced_null_handle_guards() {
        let (market, order_id) = (cstr("BTC/USDT"), cstr("1"));
        let ids = [order_id.as_ptr()];
        assert_eq!(
            unsafe {
                wickra_advanced_cancel_batch(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    ids.as_ptr(),
                    1,
                )
            },
            WICKRA_ERR_NULL
        );
        assert_eq!(
            unsafe {
                wickra_advanced_amend_order(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    order_id.as_ptr(),
                    f64::NAN,
                    f64::NAN,
                    core::ptr::null_mut(),
                )
            },
            WICKRA_ERR_NULL
        );
        unsafe { wickra_advanced_free(core::ptr::null_mut()) };
    }

    #[test]
    fn advanced_amend_rejects_non_finite_price() {
        // Construction is offline; an infinite price argument is rejected before
        // any network request.
        let (name, key, secret) = (cstr("binance"), cstr("k"), cstr("s"));
        let handle = unsafe {
            wickra_connect_advanced(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
                false,
            )
        };
        assert!(!handle.is_null());
        let (market, order_id) = (cstr("BTC/USDT"), cstr("1"));
        let mut out = empty_order();
        assert_eq!(
            unsafe {
                wickra_advanced_amend_order(
                    handle,
                    market.as_ptr(),
                    order_id.as_ptr(),
                    f64::INFINITY,
                    f64::NAN,
                    &raw mut out,
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_advanced_free(handle) };
    }
}
