# Capability matrix

Every venue implements the full `Exchange` surface (market data + execution +
streaming). The trait is uniform by design; this matrix records the axes that
legitimately differ per venue.

| Venue    | Spot | Derivatives | Passphrase | Signing      | WS market data | WS user data |
|----------|:----:|:-----------:|:----------:|--------------|:--------------:|:------------:|
| Binance  |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |
| Bybit    |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |
| OKX      |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |
| Bitget   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |
| KuCoin   |  ✅  |     ✅      |     ✅     | HMAC-SHA256  |       ✅       |      ✅      |
| Gate.io  |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |      ✅      |
| HTX      |  ✅  |     ✅      |     —      | HMAC-SHA256  |       ✅       |      ✅      |
| Kraken   |  ✅  |     ✅      |     —      | HMAC-SHA512  |       ✅       |      ✅      |
| Coinbase |  ✅  |     —       |     —      | ES256 JWT    |       ✅       |      ✅      |
| Upbit    |  ✅  |     —       |     —      | HS512 JWT    |       ✅       |      ✅      |

All order types are common across venues: market, limit, stop-market,
stop-limit; time-in-force GTC / IOC / FOK; `reduce_only` and `post_only` flags.
Per-symbol filters (lot step, price tick, min-notional) are enforced through
`InstrumentFilters` before an order is sent.

> The matrix reflects the traits every client implements; the object-safety and
> naming contract is asserted for all ten venues in `tests/conformance.rs`.
