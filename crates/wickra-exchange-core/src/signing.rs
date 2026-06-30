//! Shared request-signing primitives.
//!
//! The signing *scheme* (what to sign, where to put the signature) is bespoke
//! per exchange, but the cryptographic primitives are shared: HMAC-SHA256 and
//! HMAC-SHA512, encoded as hex or base64. Centralising them here means each
//! exchange module composes a documented, vector-tested building block rather
//! than re-implementing HMAC.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha512};

type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;

fn hmac_sha256(secret: &[u8], message: &[u8]) -> Vec<u8> {
    // `new_from_slice` accepts a key of any length, so this never errors.
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha512(secret: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha512::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA256 of `message` under `secret`, lower-case hex (Binance, Bybit, …).
#[must_use]
pub fn hmac_sha256_hex(secret: &[u8], message: &[u8]) -> String {
    hex::encode(hmac_sha256(secret, message))
}

/// HMAC-SHA256 of `message` under `secret`, standard base64 (OKX, Bitget, KuCoin).
#[must_use]
pub fn hmac_sha256_base64(secret: &[u8], message: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(hmac_sha256(secret, message))
}

/// HMAC-SHA512 of `message` under `secret`, lower-case hex (Gate.io).
#[must_use]
pub fn hmac_sha512_hex(secret: &[u8], message: &[u8]) -> String {
    hex::encode(hmac_sha512(secret, message))
}

/// HMAC-SHA512 of `message` under `secret`, standard base64 (Kraken's outer step).
#[must_use]
pub fn hmac_sha512_base64(secret: &[u8], message: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(hmac_sha512(secret, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC-style known vector: HMAC-SHA256("key", "The quick brown fox jumps over
    // the lazy dog") — a widely published reference value.
    const KEY: &[u8] = b"key";
    const MSG: &[u8] = b"The quick brown fox jumps over the lazy dog";

    #[test]
    fn hmac_sha256_matches_known_vector() {
        assert_eq!(
            hmac_sha256_hex(KEY, MSG),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        assert_eq!(
            hmac_sha256_base64(KEY, MSG),
            "97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg="
        );
    }

    #[test]
    fn hmac_sha512_matches_known_vector() {
        assert_eq!(
            hmac_sha512_hex(KEY, MSG),
            "b42af09057bac1e2d41708e48a902e09b5ff7f12ab428a4fe86653c73dd248fb\
             82f948a549f7b791a5b41915ee4d1ec3935357e4e2317250d0372afa2ebeeb3a"
        );
    }

    #[test]
    fn binance_style_query_signature_vector() {
        // A fixed reference: the Binance-style signed-endpoint key + query string
        // produces this HMAC-SHA256. Doubles as the cross-check for the Binance
        // module's signing step (same primitive, same expected bytes).
        let secret = b"NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0";
        let payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC\
                       &quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        assert_eq!(
            hmac_sha256_hex(secret, payload.as_bytes()),
            "b89008e7051ffbf2242be7dc5ae67fd146e6430688627b802c0cbec146e46aef"
        );
    }
}
