# Architecture

wickra-exchange mirrors the Wickra tiering: a pure, dependency-light Rust core,
a thin real-socket facade, and language bindings — native for Python/Node,
a C ABI hub for everyone else.

```
                        wickra-exchange-core            (pure logic, ~100% tested)
   traits · types · signing · instruments · orderbook · 10 venue clients
   Paper / Replay simulators · feeds · Mock transports
                                |
                    injected HttpTransport / WsTransport
                                |
                        wickra-exchange (facade)         (real sockets, coverage-excluded)
   Reqwest HTTP · tokio-tungstenite WS · connect() factory
                                |
        +-----------------+-----+------------------+
        |                 |                        |
   Python (PyO3)     Node (napi)             C ABI hub (cbindgen)
                                                    |
                              C · C++ · C# · Go · Java · R
```

## Injected transports

Every venue client is generic over the `HttpTransport` / `WsTransport` traits.
Tests drive `MockHttpTransport` / `MockWsTransport` with recorded JSON fixtures
and an injectable clock (`with_clock`) for exact signature assertions — so the
whole request/parse/normalise path is covered offline, with zero network.

The real adapters (`ReqwestHttpTransport`, `TungsteniteWsTransport`) live in the
facade and are the only code that touches a socket; they are excluded from
coverage and exercised by gated `#[ignore]` integration tests.

## Decimal discipline

The order layer is exact `Decimal` (rust_decimal) end to end — prices and
quantities never touch `f64`. Only the indicator-facing `Candle` (from
`wickra-core`) uses `f64`. The `feeds` module converts venue microstructure into
the exact wickra-core input types with no glue.

**Where a binding cannot carry a `Decimal`, the conversion is the whole of the
fidelity.** A C function signature has no decimal type, and Python, Node and
WASM callers pass the language's own number, so those four take `f64` and
convert. That conversion has to read the number the caller *wrote*, not the
binary double it became: `Decimal::from_f64_retain` keeps the binary expansion,
so a caller asking for 20000.15 sent `20000.150000000001455191522832` to the
venue — which a price filter rejects and a tick check rounds away from. Every
entry point uses `Decimal::from_f64` instead, and each binding has a test
pinning the exact strings.

## Differentiators

`PaperExchange` and `ReplayExchange` implement the same `Exchange` trait, so a
strategy runs paper → replay → live by swapping the constructor — see the
[Cookbook](Cookbook.md).
