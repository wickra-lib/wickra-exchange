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
    connect, connect_advanced, connect_derivatives, connect_user_data, connect_ws_execution,
    AdvancedOrders, BookLevel, Candle, Credentials, Derivatives, Error, Event, Exchange,
    ExchangeOptions, MarginMode, MarketType, OcoRequest, Order, OrderRequest, OrderSide,
    OrderStatus, PaperExchange, Position, PositionSide, ReplayExchange, Symbol, Ticker, TradePrint,
    WsExecution, WsUserData,
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

/// A ticker snapshot (C-ABI mirror of `Ticker`).
#[repr(C)]
pub struct WickraTicker {
    /// Market symbol, NUL-terminated (`base/quote`).
    pub symbol: [c_char; WICKRA_STR_CAP],
    /// Last traded price.
    pub last: f64,
    /// Best bid price.
    pub bid: f64,
    /// Best ask price.
    pub ask: f64,
    /// Rolling base-asset volume.
    pub volume: f64,
}

/// A single OHLCV candle (C-ABI mirror of `Candle`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WickraCandle {
    /// Bar open price.
    pub open: f64,
    /// Bar high price.
    pub high: f64,
    /// Bar low price.
    pub low: f64,
    /// Bar close price.
    pub close: f64,
    /// Bar volume.
    pub volume: f64,
    /// Bar timestamp (venue epoch / resolution).
    pub timestamp: i64,
}

/// A single order-book level: price and quantity (C-ABI mirror of `BookLevel`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WickraBookLevel {
    /// Price at this level.
    pub price: f64,
    /// Resting quantity at this level.
    pub quantity: f64,
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

/// An opaque private user-data handle over a live client. Construct with
/// `wickra_connect_user_data`; release with `wickra_user_data_free`.
pub struct WickraUserData {
    inner: Box<dyn WsUserData>,
}

/// An opaque WebSocket order-API handle over a live client. Construct with
/// `wickra_connect_ws_execution`; release with `wickra_ws_execution_free`.
pub struct WickraWsExecution {
    inner: Box<dyn WsExecution>,
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

fn fill_position(dst: &mut WickraPosition, position: &Position) {
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
}

fn fill_ticker(dst: &mut WickraTicker, ticker: &Ticker) {
    write_cstr(&mut dst.symbol, &ticker.symbol.to_string());
    dst.last = to_float(ticker.last);
    dst.bid = to_float(ticker.bid);
    dst.ask = to_float(ticker.ask);
    dst.volume = to_float(ticker.volume);
}

fn fill_candle(dst: &mut WickraCandle, candle: &Candle) {
    dst.open = candle.open;
    dst.high = candle.high;
    dst.low = candle.low;
    dst.close = candle.close;
    dst.volume = candle.volume;
    dst.timestamp = candle.timestamp;
}

fn fill_book_level(dst: &mut WickraBookLevel, level: &BookLevel) {
    dst.price = to_float(level.price);
    dst.quantity = to_float(level.quantity);
}

/// Which market-data stream `subscribe` opens.
#[derive(Clone, Copy)]
enum SubKind {
    Trades,
    Book,
    Ticker,
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

/// Build an [`OrderRequest`] from C-ABI scalars: `side` is `WICKRA_SIDE_*`,
/// a finite `price` yields a limit order and `NaN` a market order. Returns
/// `None` on a bad side code or a non-finite quantity/price.
fn build_request(symbol: Symbol, side: i32, quantity: f64, price: f64) -> Option<OrderRequest> {
    let quantity = Decimal::from_f64_retain(quantity)?;
    let price = if price.is_nan() {
        None
    } else {
        Some(Decimal::from_f64_retain(price)?)
    };
    match (side, price) {
        (WICKRA_SIDE_BUY, None) => Some(OrderRequest::market_buy(symbol, quantity)),
        (WICKRA_SIDE_SELL, None) => Some(OrderRequest::market_sell(symbol, quantity)),
        (WICKRA_SIDE_BUY, Some(price)) => Some(OrderRequest::limit_buy(symbol, quantity, price)),
        (WICKRA_SIDE_SELL, Some(price)) => Some(OrderRequest::limit_sell(symbol, quantity, price)),
        _ => None,
    }
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

/// Fetch the current ticker for `market`, writing it into `out`.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_ticker(
    handle: *mut WickraExchange,
    market: *const c_char,
    out: *mut WickraTicker,
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
    match exchange.inner.as_exchange().ticker(&symbol) {
        Ok(ticker) => {
            unsafe { fill_ticker(&mut *out, &ticker) };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Fetch up to `limit` candles for `market` at `interval` into `out` (capacity
/// `cap`). Returns the total candle count (`>= 0`); when it exceeds `cap` the
/// buffer was truncated — re-call with a larger buffer.
///
/// # Safety
/// `handle` must be valid; `market`/`interval` must be valid C strings; `out`
/// must be writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_klines(
    handle: *mut WickraExchange,
    market: *const c_char,
    interval: *const c_char,
    limit: u32,
    out: *mut WickraCandle,
    cap: usize,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    let (Some(market), Some(interval)) = (unsafe { (opt_str(market), opt_str(interval)) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match exchange
        .inner
        .as_exchange()
        .klines(&symbol, interval, limit)
    {
        Ok(candles) => {
            let written = candles.len().min(cap);
            for (i, candle) in candles.iter().take(written).enumerate() {
                fill_candle(unsafe { &mut *out.add(i) }, candle);
            }
            i32::try_from(candles.len()).unwrap_or(i32::MAX)
        }
        Err(e) => error_code(&e),
    }
}

/// Fetch a depth snapshot for `market` (up to `depth` levels per side) into the
/// `bids_out`/`asks_out` buffers (capacities `bids_cap`/`asks_cap`), writing the
/// total per-side level counts into `out_bid_count`/`out_ask_count` (either may
/// exceed its cap — the buffer was truncated). Returns `WICKRA_OK` or an error.
///
/// # Safety
/// `handle`, `out_bid_count` and `out_ask_count` must be valid; `market` must be
/// a valid C string; `bids_out`/`asks_out` must be writable for their caps.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_order_book(
    handle: *mut WickraExchange,
    market: *const c_char,
    depth: u32,
    bids_out: *mut WickraBookLevel,
    bids_cap: usize,
    asks_out: *mut WickraBookLevel,
    asks_cap: usize,
    out_bid_count: *mut usize,
    out_ask_count: *mut usize,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out_bid_count.is_null() || out_ask_count.is_null() {
        return WICKRA_ERR_NULL;
    }
    if (bids_out.is_null() && bids_cap != 0) || (asks_out.is_null() && asks_cap != 0) {
        return WICKRA_ERR_NULL;
    }
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match exchange.inner.as_exchange().order_book(&symbol, depth) {
        Ok(book) => {
            let bids_written = book.bids.len().min(bids_cap);
            for (i, level) in book.bids.iter().take(bids_written).enumerate() {
                fill_book_level(unsafe { &mut *bids_out.add(i) }, level);
            }
            let asks_written = book.asks.len().min(asks_cap);
            for (i, level) in book.asks.iter().take(asks_written).enumerate() {
                fill_book_level(unsafe { &mut *asks_out.add(i) }, level);
            }
            unsafe {
                *out_bid_count = book.bids.len();
                *out_ask_count = book.asks.len();
            }
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

fn subscribe(handle: *mut WickraExchange, market: *const c_char, which: SubKind) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let exchange = exchange.inner.as_exchange();
    let result = match which {
        SubKind::Trades => exchange.subscribe_trades(&symbol),
        SubKind::Book => exchange.subscribe_book(&symbol),
        SubKind::Ticker => exchange.subscribe_ticker(&symbol),
    };
    match result {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Subscribe to the public trade stream for `market`.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_subscribe_trades(
    handle: *mut WickraExchange,
    market: *const c_char,
) -> i32 {
    subscribe(handle, market, SubKind::Trades)
}

/// Subscribe to the order-book stream for `market`.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_subscribe_book(
    handle: *mut WickraExchange,
    market: *const c_char,
) -> i32 {
    subscribe(handle, market, SubKind::Book)
}

/// Subscribe to the ticker stream for `market`.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_subscribe_ticker(
    handle: *mut WickraExchange,
    market: *const c_char,
) -> i32 {
    subscribe(handle, market, SubKind::Ticker)
}

/// Look up a single order by venue id, writing it into `out`.
///
/// # Safety
/// `handle` and `out` must be valid; `market`/`order_id` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_query_order(
    handle: *mut WickraExchange,
    market: *const c_char,
    order_id: *const c_char,
    out: *mut WickraOrder,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
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
    match exchange.inner.as_exchange().query_order(&symbol, order_id) {
        Ok(order) => {
            unsafe { fill_order(&mut *out, &order) };
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// List open orders into `out` (capacity `cap`). Pass a `market` C string to
/// scope to one symbol, or null for all. Returns the total number of open orders
/// (`>= 0`); when it exceeds `cap` the buffer was truncated.
///
/// # Safety
/// `handle` must be valid; `market` must be null or a valid C string; `out` must
/// be writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_exchange_open_orders(
    handle: *mut WickraExchange,
    market: *const c_char,
    out: *mut WickraOrder,
    cap: usize,
) -> i32 {
    let Some(exchange) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    let symbol = if market.is_null() {
        None
    } else {
        let Some(market) = (unsafe { opt_str(market) }) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        let Some(symbol) = parse_symbol(market) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        Some(symbol)
    };
    match exchange.inner.as_exchange().open_orders(symbol.as_ref()) {
        Ok(orders) => {
            let written = orders.len().min(cap);
            for (i, order) in orders.iter().take(written).enumerate() {
                fill_order(unsafe { &mut *out.add(i) }, order);
            }
            i32::try_from(orders.len()).unwrap_or(i32::MAX)
        }
        Err(e) => error_code(&e),
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
                fill_position(unsafe { &mut *out }, &position);
                WICKRA_OK
            }
            None => WICKRA_ERR_NOT_FOUND,
        },
        Err(e) => error_code(&e),
    }
}

/// List every open position into `out` (capacity `cap`). Pass a `market` C
/// string to scope to one symbol, or null for all. Returns the total number of
/// open positions (`>= 0`) or a negative error code; when the return exceeds
/// `cap` the buffer was truncated — re-call with a larger buffer.
///
/// # Safety
/// `handle` must be valid; `market` must be null or a valid C string; `out` must
/// be writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_derivatives_positions(
    handle: *mut WickraDerivatives,
    market: *const c_char,
    out: *mut WickraPosition,
    cap: usize,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    let symbol = if market.is_null() {
        None
    } else {
        let Some(market) = (unsafe { opt_str(market) }) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        let Some(symbol) = parse_symbol(market) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        Some(symbol)
    };
    match handle.inner.positions(symbol.as_ref()) {
        Ok(positions) => {
            let written = positions.len().min(cap);
            for (i, position) in positions.iter().take(written).enumerate() {
                fill_position(unsafe { &mut *out.add(i) }, position);
            }
            i32::try_from(positions.len()).unwrap_or(i32::MAX)
        }
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

/// Place a one-cancels-other bracket on `market`: a take-profit limit leg at
/// `price` paired with a stop leg triggered at `stop_price`. A finite
/// `stop_limit_price` makes the stop leg a stop-limit; `NaN` leaves it a
/// stop-market. The resulting legs are written into `out` (capacity `cap`);
/// returns the number of legs placed (`>= 0`, typically 2) or a negative error
/// code. When the return exceeds `cap` the buffer was truncated.
///
/// # Safety
/// `handle` must be valid; `market` must be a valid C string; `out` must be
/// writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_advanced_place_oco(
    handle: *mut WickraAdvanced,
    market: *const c_char,
    side: i32,
    quantity: f64,
    price: f64,
    stop_price: f64,
    stop_limit_price: f64,
    out: *mut WickraOrder,
    cap: usize,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    let Some(market) = (unsafe { opt_str(market) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let side = match side {
        WICKRA_SIDE_BUY => OrderSide::Buy,
        WICKRA_SIDE_SELL => OrderSide::Sell,
        _ => return WICKRA_ERR_INVALID_ARG,
    };
    let (Some(quantity), Some(price), Some(stop_price)) = (
        Decimal::from_f64_retain(quantity),
        Decimal::from_f64_retain(price),
        Decimal::from_f64_retain(stop_price),
    ) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let mut request = OcoRequest::new(symbol, side, quantity, price, stop_price);
    if !stop_limit_price.is_nan() {
        let Some(slp) = Decimal::from_f64_retain(stop_limit_price) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        request = request.with_stop_limit_price(slp);
    }
    match handle.inner.place_oco(&request) {
        Ok(orders) => {
            let written = orders.len().min(cap);
            for (i, order) in orders.iter().take(written).enumerate() {
                fill_order(unsafe { &mut *out.add(i) }, order);
            }
            i32::try_from(orders.len()).unwrap_or(i32::MAX)
        }
        Err(e) => error_code(&e),
    }
}

/// Place several orders in one request. The `n` orders are described by parallel
/// arrays: `markets[i]` (C string), `sides[i]` (`WICKRA_SIDE_*`), `quantities[i]`,
/// and `prices[i]` (finite for a limit order, `NaN` for market). Each order's
/// outcome is written into `out[i]` and its per-order status into `out_codes[i]`
/// (`WICKRA_OK` on success, else a negative error code with `out[i]` left empty).
/// Returns the number of results written (`>= 0`, capped at `cap`) or a negative
/// error code for a whole-request failure.
///
/// # Safety
/// `handle` must be valid; `markets`/`sides`/`quantities`/`prices` must each
/// point to `n` valid elements; `out` and `out_codes` must be writable for `cap`
/// elements.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn wickra_advanced_place_batch(
    handle: *mut WickraAdvanced,
    markets: *const *const c_char,
    sides: *const i32,
    quantities: *const f64,
    prices: *const f64,
    n: usize,
    out: *mut WickraOrder,
    out_codes: *mut i32,
    cap: usize,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if (out.is_null() || out_codes.is_null()) && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    if n != 0 && (markets.is_null() || sides.is_null() || quantities.is_null() || prices.is_null())
    {
        return WICKRA_ERR_NULL;
    }
    let mut requests = Vec::with_capacity(n);
    for i in 0..n {
        let Some(market) = (unsafe { opt_str(*markets.add(i)) }) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        let Some(symbol) = parse_symbol(market) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        let side = unsafe { *sides.add(i) };
        let quantity = unsafe { *quantities.add(i) };
        let price = unsafe { *prices.add(i) };
        let Some(request) = build_request(symbol, side, quantity, price) else {
            return WICKRA_ERR_INVALID_ARG;
        };
        requests.push(request);
    }
    match handle.inner.place_batch(&requests) {
        Ok(results) => {
            let written = results.len().min(cap);
            for (i, result) in results.iter().take(written).enumerate() {
                let slot = unsafe { &mut *out.add(i) };
                let code = unsafe { &mut *out_codes.add(i) };
                match result {
                    Ok(order) => {
                        fill_order(slot, order);
                        *code = WICKRA_OK;
                    }
                    Err(e) => {
                        *slot = empty_order();
                        *code = error_code(e);
                    }
                }
            }
            i32::try_from(results.len()).unwrap_or(i32::MAX)
        }
        Err(e) => error_code(&e),
    }
}

// ------------------------------ user data ------------------------------------

/// Connect a private user-data client for `name`. `futures` selects the USDⓈ-M
/// futures market. Returns null for an unknown / spot-only venue or bad UTF-8.
///
/// # Safety
/// The string arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_connect_user_data(
    name: *const c_char,
    api_key: *const c_char,
    api_secret: *const c_char,
    passphrase: *const c_char,
    private_key: *const c_char,
    testnet: bool,
    futures: bool,
) -> *mut WickraUserData {
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
    match connect_user_data(name, credentials, &options) {
        Ok(inner) => Box::into_raw(Box::new(WickraUserData { inner })),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Release a user-data handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null or a pointer from `wickra_connect_user_data`, freed
/// exactly once.
#[no_mangle]
pub unsafe extern "C" fn wickra_user_data_free(handle: *mut WickraUserData) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Open the private user-data stream. Afterwards `wickra_user_data_poll` also
/// drains the account's own order/balance events.
///
/// # Safety
/// `handle` must be valid.
#[no_mangle]
pub unsafe extern "C" fn wickra_user_data_subscribe(handle: *mut WickraUserData) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    match handle.inner.subscribe_user_data() {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Keep the private stream alive (refresh the venue session / send a heartbeat)
/// so it is not dropped for inactivity; call this periodically. A dropped stream
/// is also recovered automatically on the next `wickra_user_data_poll`. A no-op
/// before `wickra_user_data_subscribe`.
///
/// # Safety
/// `handle` must be valid.
#[no_mangle]
pub unsafe extern "C" fn wickra_user_data_keepalive(handle: *mut WickraUserData) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    match handle.inner.keepalive_user_data() {
        Ok(()) => WICKRA_OK,
        Err(e) => error_code(&e),
    }
}

/// Drain buffered user-data events into `out` (capacity `cap`). Returns the
/// number written (`>= 0`) or a negative error code.
///
/// # Safety
/// `handle` must be valid; `out` must be writable for `cap` elements.
#[no_mangle]
pub unsafe extern "C" fn wickra_user_data_poll(
    handle: *mut WickraUserData,
    out: *mut WickraEvent,
    cap: usize,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    if out.is_null() && cap != 0 {
        return WICKRA_ERR_NULL;
    }
    // `WsUserData: MarketData`, so the handle can poll directly.
    let events = handle.inner.poll_events();
    let count = events.len().min(cap);
    for (i, event) in events.iter().take(count).enumerate() {
        fill_event(unsafe { &mut *out.add(i) }, event);
    }
    i32::try_from(count).unwrap_or(i32::MAX)
}

// ---------------------------- ws execution -----------------------------------

/// Connect a WebSocket order-API client for `name`. `futures` selects the USDⓈ-M
/// futures market. Returns null for an unknown / spot-only venue or bad UTF-8.
///
/// # Safety
/// The string arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_connect_ws_execution(
    name: *const c_char,
    api_key: *const c_char,
    api_secret: *const c_char,
    passphrase: *const c_char,
    private_key: *const c_char,
    testnet: bool,
    futures: bool,
) -> *mut WickraWsExecution {
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
    match connect_ws_execution(name, credentials, &options) {
        Ok(inner) => Box::into_raw(Box::new(WickraWsExecution { inner })),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Release a ws-execution handle. Safe to call with null.
///
/// # Safety
/// `handle` must be null or a pointer from `wickra_connect_ws_execution`, freed
/// exactly once.
#[no_mangle]
pub unsafe extern "C" fn wickra_ws_execution_free(handle: *mut WickraWsExecution) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Place an order over the WebSocket order API; writes the resulting order into
/// `out`. A `NaN` `price` places a market order, a finite value a limit order.
///
/// # Safety
/// `handle` and `out` must be valid; `market` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn wickra_ws_place_order(
    handle: *mut WickraWsExecution,
    market: *const c_char,
    side: i32,
    quantity: f64,
    price: f64,
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
    let Some(request) = build_request(symbol, side, quantity, price) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.place_order_ws(&request) {
        Ok(order) => {
            fill_order(unsafe { &mut *out }, &order);
            WICKRA_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Cancel an order over the WebSocket order API by venue id.
///
/// # Safety
/// `handle` must be valid; `market` and `order_id` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn wickra_ws_cancel_order(
    handle: *mut WickraWsExecution,
    market: *const c_char,
    order_id: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return WICKRA_ERR_NULL;
    };
    let (Some(market), Some(order_id)) = (unsafe { (opt_str(market), opt_str(order_id)) }) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    let Some(symbol) = parse_symbol(market) else {
        return WICKRA_ERR_INVALID_ARG;
    };
    match handle.inner.cancel_order_ws(&symbol, order_id) {
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
    fn market_data_reads_over_the_abi() {
        let market = cstr("BTC/USDT");
        let usdt = cstr("USDT");
        let assets = [usdt.as_ptr()];
        let amounts = [100_000.0_f64];
        let interval = cstr("1m");

        unsafe {
            let ex = wickra_paper_new(assets.as_ptr(), amounts.as_ptr(), 1, 1.0, 5.0, 10.0);
            assert!(!ex.is_null());
            assert_eq!(
                wickra_exchange_set_price(ex, market.as_ptr(), 20_000.0),
                WICKRA_OK
            );

            // ticker reflects the mark on both sides.
            let mut ticker = WickraTicker {
                symbol: [0; WICKRA_STR_CAP],
                last: 0.0,
                bid: 0.0,
                ask: 0.0,
                volume: -1.0,
            };
            assert_eq!(
                wickra_exchange_ticker(ex, market.as_ptr(), &raw mut ticker),
                WICKRA_OK
            );
            assert_eq!(read_field(&ticker.symbol), "BTC/USDT");
            assert!((ticker.last - 20_000.0).abs() < 1e-9);
            assert!((ticker.bid - ticker.ask).abs() < 1e-9);

            // subscribe_* are accepted by the paper feed.
            assert_eq!(
                wickra_exchange_subscribe_trades(ex, market.as_ptr()),
                WICKRA_OK
            );
            assert_eq!(
                wickra_exchange_subscribe_book(ex, market.as_ptr()),
                WICKRA_OK
            );
            assert_eq!(
                wickra_exchange_subscribe_ticker(ex, market.as_ptr()),
                WICKRA_OK
            );

            // paper has no historical / depth feed: these report an error.
            let mut candles = [WickraCandle {
                open: 0.0,
                high: 0.0,
                low: 0.0,
                close: 0.0,
                volume: 0.0,
                timestamp: 0,
            }; 4];
            assert!(
                wickra_exchange_klines(
                    ex,
                    market.as_ptr(),
                    interval.as_ptr(),
                    10,
                    candles.as_mut_ptr(),
                    4
                ) < 0
            );
            let mut bids = [WickraBookLevel {
                price: 0.0,
                quantity: 0.0,
            }; 4];
            let mut asks = [WickraBookLevel {
                price: 0.0,
                quantity: 0.0,
            }; 4];
            let mut bid_count = 0usize;
            let mut ask_count = 0usize;
            assert!(
                wickra_exchange_order_book(
                    ex,
                    market.as_ptr(),
                    10,
                    bids.as_mut_ptr(),
                    4,
                    asks.as_mut_ptr(),
                    4,
                    &raw mut bid_count,
                    &raw mut ask_count,
                ) < 0
            );

            wickra_exchange_free(ex);
        }
    }

    #[test]
    fn order_lifecycle_reads_over_the_abi() {
        let market = cstr("BTC/USDT");
        let usdt = cstr("USDT");
        let assets = [usdt.as_ptr()];
        let amounts = [100_000.0_f64];

        unsafe {
            let ex = wickra_paper_new(assets.as_ptr(), amounts.as_ptr(), 1, 1.0, 5.0, 10.0);
            assert!(!ex.is_null());
            assert_eq!(
                wickra_exchange_set_price(ex, market.as_ptr(), 20_000.0),
                WICKRA_OK
            );

            // A resting limit can be read back by id and appears in open_orders.
            let mut resting = empty_order();
            assert_eq!(
                wickra_exchange_place_limit(
                    ex,
                    market.as_ptr(),
                    WICKRA_SIDE_BUY,
                    1.0,
                    19_000.0,
                    &raw mut resting,
                ),
                WICKRA_OK
            );
            assert_eq!(resting.status, WICKRA_STATUS_NEW);
            let order_id = cstr(&read_field(&resting.id));

            let mut queried = empty_order();
            assert_eq!(
                wickra_exchange_query_order(
                    ex,
                    market.as_ptr(),
                    order_id.as_ptr(),
                    &raw mut queried
                ),
                WICKRA_OK
            );
            assert_eq!(read_field(&queried.id), read_field(&resting.id));

            let mut open = [empty_order()];
            assert_eq!(
                wickra_exchange_open_orders(ex, market.as_ptr(), open.as_mut_ptr(), 1),
                1
            );
            assert_eq!(read_field(&open[0].id), read_field(&resting.id));

            wickra_exchange_free(ex);
        }
    }

    #[test]
    fn market_data_null_handle_guards() {
        let market = cstr("BTC/USDT");
        let interval = cstr("1m");
        let order_id = cstr("x");
        unsafe {
            let mut ticker = WickraTicker {
                symbol: [0; WICKRA_STR_CAP],
                last: 0.0,
                bid: 0.0,
                ask: 0.0,
                volume: 0.0,
            };
            assert_eq!(
                wickra_exchange_ticker(core::ptr::null_mut(), market.as_ptr(), &raw mut ticker),
                WICKRA_ERR_NULL
            );
            assert_eq!(
                wickra_exchange_klines(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    interval.as_ptr(),
                    10,
                    core::ptr::null_mut(),
                    0,
                ),
                WICKRA_ERR_NULL
            );
            let mut bid_count = 0usize;
            let mut ask_count = 0usize;
            assert_eq!(
                wickra_exchange_order_book(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    10,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    0,
                    &raw mut bid_count,
                    &raw mut ask_count,
                ),
                WICKRA_ERR_NULL
            );
            assert_eq!(
                wickra_exchange_subscribe_trades(core::ptr::null_mut(), market.as_ptr()),
                WICKRA_ERR_NULL
            );
            assert_eq!(
                wickra_exchange_subscribe_book(core::ptr::null_mut(), market.as_ptr()),
                WICKRA_ERR_NULL
            );
            assert_eq!(
                wickra_exchange_subscribe_ticker(core::ptr::null_mut(), market.as_ptr()),
                WICKRA_ERR_NULL
            );
            let mut order = empty_order();
            assert_eq!(
                wickra_exchange_query_order(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    order_id.as_ptr(),
                    &raw mut order,
                ),
                WICKRA_ERR_NULL
            );
            assert_eq!(
                wickra_exchange_open_orders(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    core::ptr::null_mut(),
                    0,
                ),
                WICKRA_ERR_NULL
            );
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

    fn live_user_data(name: &str) -> *mut WickraUserData {
        let (name, key, secret) = (cstr(name), cstr("k"), cstr("s"));
        unsafe {
            wickra_connect_user_data(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
                false,
            )
        }
    }

    fn live_ws_execution(name: &str) -> *mut WickraWsExecution {
        let (name, key, secret) = (cstr(name), cstr("k"), cstr("s"));
        unsafe {
            wickra_connect_ws_execution(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
                false,
            )
        }
    }

    #[test]
    fn user_data_spot_only_is_null_and_trading_venue_polls() {
        assert!(
            live_user_data("coinbase").is_null(),
            "coinbase has no private user-data stream"
        );
        let handle = live_user_data("binance");
        assert!(!handle.is_null());
        // `WsUserData: MarketData`, so the handle polls (nothing buffered offline).
        let mut buf: [WickraEvent; 4] = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { wickra_user_data_poll(handle, buf.as_mut_ptr(), buf.len()) },
            0
        );
        // Keepalive is a no-op before subscribe (no stream open yet).
        assert_eq!(unsafe { wickra_user_data_keepalive(handle) }, WICKRA_OK);
        unsafe { wickra_user_data_free(handle) };
    }

    #[test]
    fn user_data_null_handle_guards() {
        assert_eq!(
            unsafe { wickra_user_data_subscribe(core::ptr::null_mut()) },
            WICKRA_ERR_NULL
        );
        assert_eq!(
            unsafe { wickra_user_data_keepalive(core::ptr::null_mut()) },
            WICKRA_ERR_NULL
        );
        let mut buf: [WickraEvent; 1] = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { wickra_user_data_poll(core::ptr::null_mut(), buf.as_mut_ptr(), buf.len()) },
            WICKRA_ERR_NULL
        );
        unsafe { wickra_user_data_free(core::ptr::null_mut()) };
    }

    #[test]
    fn ws_execution_spot_only_is_null_and_bad_arg_is_rejected() {
        assert!(
            live_ws_execution("upbit").is_null(),
            "upbit has no WebSocket order API"
        );
        let handle = live_ws_execution("binance");
        assert!(!handle.is_null());
        // A malformed market is rejected before any request.
        let bad = cstr("BTCUSDT");
        let mut out = empty_order();
        assert_eq!(
            unsafe {
                wickra_ws_place_order(
                    handle,
                    bad.as_ptr(),
                    WICKRA_SIDE_BUY,
                    1.0,
                    100.0,
                    &raw mut out,
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_ws_execution_free(handle) };
    }

    #[test]
    fn ws_execution_null_handle_guards() {
        let (market, order_id) = (cstr("BTC/USDT"), cstr("1"));
        let mut out = empty_order();
        assert_eq!(
            unsafe {
                wickra_ws_place_order(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    WICKRA_SIDE_BUY,
                    1.0,
                    100.0,
                    &raw mut out,
                )
            },
            WICKRA_ERR_NULL
        );
        assert_eq!(
            unsafe {
                wickra_ws_cancel_order(core::ptr::null_mut(), market.as_ptr(), order_id.as_ptr())
            },
            WICKRA_ERR_NULL
        );
        unsafe { wickra_ws_execution_free(core::ptr::null_mut()) };
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

    fn live_derivatives(name: &str) -> *mut WickraDerivatives {
        let (name, key, secret) = (cstr(name), cstr("k"), cstr("s"));
        unsafe {
            wickra_connect_derivatives(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
            )
        }
    }

    fn live_advanced(name: &str) -> *mut WickraAdvanced {
        let (name, key, secret) = (cstr(name), cstr("k"), cstr("s"));
        unsafe {
            wickra_connect_advanced(
                name.as_ptr(),
                key.as_ptr(),
                secret.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                false,
                false,
            )
        }
    }

    #[test]
    fn derivatives_positions_null_and_bad_arg() {
        // Null handle is rejected without touching the buffer.
        let mut buf: [WickraPosition; 2] = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe {
                wickra_derivatives_positions(
                    core::ptr::null_mut(),
                    core::ptr::null(),
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            },
            WICKRA_ERR_NULL
        );
        // Live handle (offline until an RPC): a malformed market is rejected.
        let handle = live_derivatives("binance");
        assert!(!handle.is_null());
        let bad_market = cstr("BTCUSDT");
        assert_eq!(
            unsafe {
                wickra_derivatives_positions(
                    handle,
                    bad_market.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_derivatives_free(handle) };
    }

    #[test]
    fn advanced_place_oco_null_and_bad_args() {
        let market = cstr("BTC/USDT");
        let mut buf: [WickraOrder; 2] = [empty_order(), empty_order()];
        assert_eq!(
            unsafe {
                wickra_advanced_place_oco(
                    core::ptr::null_mut(),
                    market.as_ptr(),
                    WICKRA_SIDE_SELL,
                    1.0,
                    110.0,
                    90.0,
                    f64::NAN,
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            },
            WICKRA_ERR_NULL
        );
        let handle = live_advanced("binance");
        assert!(!handle.is_null());
        // A bad side code is rejected before any network request.
        assert_eq!(
            unsafe {
                wickra_advanced_place_oco(
                    handle,
                    market.as_ptr(),
                    42,
                    1.0,
                    110.0,
                    90.0,
                    f64::NAN,
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        // A non-finite stop-limit price is rejected.
        assert_eq!(
            unsafe {
                wickra_advanced_place_oco(
                    handle,
                    market.as_ptr(),
                    WICKRA_SIDE_SELL,
                    1.0,
                    110.0,
                    90.0,
                    f64::INFINITY,
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_advanced_free(handle) };
    }

    #[test]
    fn advanced_place_batch_null_and_bad_arg() {
        let good = cstr("BTC/USDT");
        let markets = [good.as_ptr()];
        let sides = [WICKRA_SIDE_BUY];
        let quantities = [1.0_f64];
        let prices = [f64::NAN];
        let mut out: [WickraOrder; 1] = [empty_order()];
        let mut codes = [0_i32; 1];
        assert_eq!(
            unsafe {
                wickra_advanced_place_batch(
                    core::ptr::null_mut(),
                    markets.as_ptr(),
                    sides.as_ptr(),
                    quantities.as_ptr(),
                    prices.as_ptr(),
                    1,
                    out.as_mut_ptr(),
                    codes.as_mut_ptr(),
                    out.len(),
                )
            },
            WICKRA_ERR_NULL
        );
        // A malformed market in the array is rejected before any request.
        let handle = live_advanced("binance");
        assert!(!handle.is_null());
        let bad = cstr("BTCUSDT");
        let bad_markets = [bad.as_ptr()];
        assert_eq!(
            unsafe {
                wickra_advanced_place_batch(
                    handle,
                    bad_markets.as_ptr(),
                    sides.as_ptr(),
                    quantities.as_ptr(),
                    prices.as_ptr(),
                    1,
                    out.as_mut_ptr(),
                    codes.as_mut_ptr(),
                    out.len(),
                )
            },
            WICKRA_ERR_INVALID_ARG
        );
        unsafe { wickra_advanced_free(handle) };
    }
}
