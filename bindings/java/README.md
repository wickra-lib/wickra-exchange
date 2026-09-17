<p align="center">
  <a href="https://wickra.org"><img src="https://raw.githubusercontent.com/wickra-lib/.github/main/profile/wickra-banner.webp?v=514-7" alt="Wickra Exchange — streaming-native crypto-exchange connectivity: one typed API over the ten largest exchanges, across ten languages" width="100%"></a>
</p>

[![CI](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/ci.svg)](https://github.com/wickra-lib/wickra-exchange/actions/workflows/ci.yml)
[![codecov](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/codecov.svg)](https://codecov.io/gh/wickra-lib/wickra-exchange)
[![Maven Central](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/maven.svg)](https://central.sonatype.com/artifact/org.wickra/wickra-exchange)
[![License: MIT OR Apache-2.0](https://raw.githubusercontent.com/wickra-lib/.github/main/profile/badges/wickra-exchange/license.svg)](https://github.com/wickra-lib/wickra-exchange#license)

# Wickra Exchange — Java

---

> **▶ Live demo:** all 514 indicators over real Binance market data, computed live in your browser — **[live.wickra.org](https://live.wickra.org)** · zero backend, powered by `wickra-wasm`.

**One typed API. Ten exchanges. Eight languages — for Java. `org.wickra:wickra-exchange` — prebuilt native library inside the jar, no JNI, no system dependencies.**

JVM bindings for [`wickra-exchange`](https://github.com/wickra-lib/wickra-exchange)
over the Wickra C ABI via the Java FFM (Panama) API — no JNI. One synchronous,
pull-based API over the ten largest crypto exchanges, plus offline paper and
replay simulators that share the same API.

## Requirements

- **Java 22 or later** (the FFM API is final since Java 22; no preview flag).
- The FFM API is *restricted*: pass `--enable-native-access=ALL-UNNAMED` when you
  run your application to silence the native-access warning.

## Install

Maven:

```xml
<dependency>
  <groupId>org.wickra</groupId>
  <artifactId>wickra-exchange</artifactId>
  <version>0.1.5</version>
</dependency>
```

Gradle:

```kotlin
implementation("org.wickra:wickra-exchange:0.1.5")
```

The native library ships prebuilt per platform inside the jar and is
extracted automatically on first use. There is nothing to compile.

Requires Java 22+ (FFM). The native library path is set via the `native.lib.dir`
system property. The same strategy runs **paper, replay and live** by swapping the
constructor. Licensed under `MIT OR Apache-2.0`.

## Quick start

```java
import org.wickra.exchange.Exchange;
import java.util.Map;

try (Exchange ex = Exchange.paper(Map.of("USDT", 100_000.0), 0, 5, 0)) {
    ex.setPrice("BTC/USDT", 20_000.0);
    var order = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0);
    System.out.println(order.status());        // FILLED
    System.out.println(ex.balance("BTC"));      // 1.0
}
```

## Benchmark

Every binding forwards to the same data-driven Rust core, so what this one adds is
the call overhead of the Java Foreign Function & Memory API over the C ABI, not a different result. The core's throughput is
measured by the repository's benchmark suite and the nightly `bench.yml` run; the
numbers, the machine and how to reproduce them are in the repository
[BENCHMARKS.md](https://github.com/wickra-lib/wickra-exchange/blob/main/BENCHMARKS.md).

## Documentation

The full guide, the spec reference and the API documentation live in the main
repository and the documentation site:

- **Repository:** <https://github.com/wickra-lib/wickra-exchange>
- **Docs** (guides, spec reference, cookbook): <https://exchange.wickra.org>
- **Runnable example:** [`examples/java/`](https://github.com/wickra-lib/wickra-exchange/tree/main/examples/java)

Wickra Exchange ships native bindings for Python, Node.js, WASM and Rust, plus a C ABI hub that any
C-capable language (C, C++, C#, Go, Java, R) links against — all forwarding to the
same data-driven, `unsafe`-forbidden Rust core.

## Security

Found a security issue? **Please don't open a public issue.** Report it privately
via the repository's *Security* tab (*"Report a vulnerability"*) or email
**support@wickra.org** with a subject line starting `[wickra security]`. Full
policy: <https://github.com/wickra-lib/wickra-exchange/blob/main/SECURITY.md>.

## Disclaimer

Not a trading system and not financial advice. This library connects to exchanges
and can place real orders that risk real capital; any such use is **entirely at
your own risk**. Authentication, order rounding, reconnect handling and rate
limiting can fail in ways that lose money — test against testnets, use
withdrawal-disabled keys, and review the code before trading. The software is
provided **as is**, without warranty of any kind; see the license files for the
full terms.

## License

Licensed under either of [Apache-2.0](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/wickra-lib/wickra-exchange/blob/main/LICENSE-MIT) at your option.
