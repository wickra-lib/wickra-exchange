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

## Differentiators

`PaperExchange` and `ReplayExchange` implement the same `Exchange` trait, so a
strategy runs paper → replay → live by swapping the constructor — see the
[Cookbook](Cookbook.md).
