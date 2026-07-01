# Authentication

Every venue signs requests differently; wickra-exchange centralises the
primitives in `signing.rs` (`hmac_sha256_hex`/`_base64`, `hmac_sha512_hex`/
`_base64`, `sha256`, `sha512_hex`) and applies the venue scheme in each client.

## Credentials

```rust
use wickra_exchange::Credentials;

let creds = Credentials::new("api-key", "api-secret")
    .with_passphrase("passphrase")   // OKX, Bitget, KuCoin
    .with_private_key("-----BEGIN EC PRIVATE KEY-----\n...");  // Coinbase (ES256)
```

`Credentials` zeroizes its secret material on drop and redacts it in `Debug`.

## Schemes

- **HMAC-SHA256 / SHA512** — the majority (Binance, Bybit, OKX, Bitget, KuCoin,
  Gate.io, HTX, Kraken). The signed payload and header names differ per venue;
  each client builds the exact canonical string the venue expects.
- **ES256 JWT (Coinbase Advanced Trade)** — a per-request JWT signed with a P-256
  key using deterministic RFC-6979 ECDSA (the `p256` crate), so signatures are
  reproducible under test.
- **HS512 JWT (Upbit)** — a JWT whose payload carries an SHA-512 hash of the
  query parameters.

## Security

- Keys never leave the process; they are used only to sign outbound requests.
- The `Credentials` type is the only place secrets live; transports receive
  already-signed requests.
- See [`SECURITY.md`](../SECURITY.md) and [`THREAT_MODEL.md`](../THREAT_MODEL.md).
