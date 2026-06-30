# Threat Model

`wickra-exchange` is a connectivity library: it authenticates to exchanges, reads
market data and places signed orders. This document records what it protects,
where the trust boundaries are, and the guarantees the code is held to. It is a
living document — update it when the attack surface changes.

## Assets

1. **Secret key material** — API key, API secret, and where applicable a
   passphrase (OKX/Bitget/KuCoin) or private key (Coinbase JWT). Compromise lets
   an attacker trade or, with the wrong key permissions, withdraw funds.
2. **Order flow** — the requests that move capital. Corruption (wrong symbol,
   side, price or quantity) loses money even without a key leak.
3. **Account state** — balances, positions and open orders read back from the
   exchange.

## Trust boundaries

- **The library runs server-side / on a trusted host.** It is *never* shipped to
  a browser or other untrusted client. There is deliberately **no WASM binding**:
  a browser sandbox cannot hold secret keys or open raw sockets safely.
- **The exchange is semi-trusted** — reachable over TLS, but its responses are
  untrusted input and must be parsed defensively (see fuzz targets).
- **The network is untrusted** — all transport is TLS (`rustls`); no plaintext.

## Guarantees the code is held to

- **Secret zeroization.** `Credentials` wipe key material from memory on drop
  (`zeroize`). Secrets are never logged: redaction is enforced on every log and
  error path, and tests assert no secret substring appears in formatted output.
- **No secrets in errors.** Error types carry exchange codes and messages, never
  the signing inputs or key bytes.
- **Exact order arithmetic.** Price and quantity use `rust_decimal::Decimal`, not
  `f64`, so rounding to an exchange's lot/tick/min-notional filters is exact and
  auditable — never scientific-notation or float-drift surprises.
- **Signing is unit-pinned.** Each exchange's signature is tested against a known
  key + timestamp → known signature vector taken from the exchange's own docs.
- **Reconnect reconciliation.** On reconnect the client re-pulls open orders and
  positions and reconciles local state, so fills missed during a disconnect are
  not lost. A dead-man's-switch (cancel-on-disconnect) is wired where the exchange
  supports it.

## Out of scope

- Vulnerabilities in the exchanges themselves.
- Any deployment that places secret keys in a browser or untrusted client — this
  is explicitly unsupported.
- Custody of funds, withdrawal flows (the library favours withdrawal-disabled
  keys and does not implement withdrawals in the default surface).

## Operator guidance

Use API keys scoped to trading only (withdrawals disabled), restrict them by IP
where the exchange allows it, test against testnets before mainnet, and keep keys
out of source control (`.env`, secret managers — never committed).
