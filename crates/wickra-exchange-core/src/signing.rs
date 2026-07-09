//! Shared request-signing primitives.
//!
//! The signing *scheme* (what to sign, where to put the signature) is bespoke
//! per exchange, but the cryptographic primitives are shared: HMAC-SHA256 and
//! HMAC-SHA512, encoded as hex or base64. Centralising them here means each
//! exchange module composes a documented, vector-tested building block rather
//! than re-implementing HMAC.

use base64::Engine;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

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

/// The raw HMAC-SHA512 digest of `message` under `secret` (Upbit's JWT HS512).
#[must_use]
pub fn hmac_sha512_bytes(secret: &[u8], message: &[u8]) -> Vec<u8> {
    hmac_sha512(secret, message)
}

/// The raw SHA-256 digest of `data` (Kraken hashes `nonce + body` before the HMAC).
#[must_use]
pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// SHA-512 of `data`, lower-case hex (Gate.io hashes the request body).
#[must_use]
pub fn sha512_hex(data: &[u8]) -> String {
    hex::encode(Sha512::digest(data))
}

/// HMAC-SHA512 of `message` under a base64-decoded `secret`, standard base64.
/// Kraken supplies its private key base64-encoded; this decodes it first.
///
/// # Errors
///
/// Returns [`Error::InvalidCredentials`](crate::Error::InvalidCredentials) if the
/// secret is not valid base64.
pub fn hmac_sha512_base64_with_b64_secret(
    secret_b64: &str,
    message: &[u8],
) -> crate::Result<String> {
    let secret = base64::engine::general_purpose::STANDARD
        .decode(secret_b64.trim())
        .map_err(|_| crate::Error::InvalidCredentials("api secret must be valid base64"))?;
    Ok(hmac_sha512_base64(&secret, message))
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

    #[test]
    fn plain_hash_vectors() {
        assert_eq!(
            hex::encode(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha512_hex(b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    #[test]
    fn hmac_with_base64_secret_matches_decoded() {
        // base64("key") == "a2V5".
        assert_eq!(
            hmac_sha512_base64_with_b64_secret("a2V5", MSG).unwrap(),
            hmac_sha512_base64(b"key", MSG)
        );
        assert!(hmac_sha512_base64_with_b64_secret("not base64!!!", MSG).is_err());
    }
}
