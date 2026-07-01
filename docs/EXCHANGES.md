# Supported exchanges

wickra-exchange speaks ten venues behind one typed `Exchange` API. Every client
is generic over an injected HTTP/WebSocket transport, so its request → parse →
normalise logic is tested offline against a mock; the real sockets live in the
`wickra-exchange` facade.

| Venue    | `name()`   | Auth scheme                                   | Symbol wire form |
|----------|------------|-----------------------------------------------|------------------|
| Binance  | `binance`  | HMAC-SHA256 (query signature)                 | `BTCUSDT`        |
| Bybit    | `bybit`    | HMAC-SHA256 (`X-BAPI-*` headers)              | `BTCUSDT`        |
| OKX      | `okx`      | HMAC-SHA256 base64 over ISO-8601 ts + passphrase | `BTC-USDT`    |
| Bitget   | `bitget`   | HMAC-SHA256 base64 over ms ts + passphrase    | `BTCUSDT`        |
| KuCoin   | `kucoin`   | HMAC-SHA256 base64 + passphrase               | `BTC-USDT`       |
| Gate.io  | `gate`     | HMAC-SHA512 hex (payload hash)                | `BTC_USDT`       |
| HTX      | `htx`      | HMAC-SHA256 base64 (sorted query)             | `btcusdt`        |
| Kraken   | `kraken`   | HMAC-SHA512 over SHA256(nonce+body)           | `XBTUSDT`        |
| Coinbase | `coinbase` | ES256 JWT (per-request, RFC-6979)             | `BTC-USD`        |
| Upbit    | `upbit`    | HS512 JWT with query hash                     | `USDT-BTC`       |

Two offline backends implement the same trait for testing and simulation:

| Backend  | `name()`  | Purpose                                             |
|----------|-----------|-----------------------------------------------------|
| Paper    | `paper`   | Deterministic fills against an internal portfolio   |
| Replay   | `replay`  | Drive a recorded tape through the same API          |

Open a live client by name with the factory:

```rust
let exchange = wickra_exchange::connect("binance", credentials, &options)?;
```

See [AUTH.md](AUTH.md) for the signing details and [CAPABILITIES.md](CAPABILITIES.md)
for the per-venue capability matrix.
