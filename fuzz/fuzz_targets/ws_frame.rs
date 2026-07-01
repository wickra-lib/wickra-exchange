#![no_main]
//! Fuzz the WebSocket-frame parsing path: arbitrary text frames are deserialized
//! into the public streaming event types. Malformed frames must yield a clean
//! `Err`, never a panic.

use libfuzzer_sys::fuzz_target;
use wickra_exchange_core::{BookDelta, Event, OrderBookSnapshot, TradePrint};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = serde_json::from_str::<Event>(text);
    let _ = serde_json::from_str::<BookDelta>(text);
    let _ = serde_json::from_str::<OrderBookSnapshot>(text);
    let _ = serde_json::from_str::<TradePrint>(text);
});
