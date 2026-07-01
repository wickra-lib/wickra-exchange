#![no_main]
//! Fuzz credential construction/validation and symbol parsing with arbitrary
//! strings. Neither the builder, `validate`, nor the `FromStr` symbol parser may
//! panic on any input.

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use wickra_exchange_core::{Credentials, Symbol};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // Split on a valid char boundary so `split_at` never panics.
    let mut mid = text.len() / 2;
    while mid > 0 && !text.is_char_boundary(mid) {
        mid -= 1;
    }
    let (key, secret) = text.split_at(mid);
    let credentials = Credentials::new(key, secret).with_passphrase(secret);
    let _ = credentials.validate();
    let _ = credentials.has_passphrase();

    let _ = Symbol::from_str(text);
});
