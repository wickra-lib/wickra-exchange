#![no_main]
//! Fuzz the JSON response-parsing path: arbitrary bytes are fed to serde
//! deserialization of the public wire types and to the decimal parser. None must
//! panic; malformed input must surface as a clean `Err`.

use libfuzzer_sys::fuzz_target;
use wickra_exchange_core::{parse_decimal, Event, Order, OrderBookSnapshot, TradePrint};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = serde_json::from_str::<Event>(text);
    let _ = serde_json::from_str::<OrderBookSnapshot>(text);
    let _ = serde_json::from_str::<TradePrint>(text);
    let _ = serde_json::from_str::<Order>(text);
    let _ = parse_decimal(text);
});
