# Wickra Exchange examples — Java

Runnable Java examples for the [Wickra Exchange Java binding](../../bindings/java). The binding reaches the C ABI
through the Foreign Function & Memory API (JDK 22+), so build the library once
and point the JVM at it with `-Dnative.lib.dir`:

```bash
cargo build -p wickra-exchange-c --release
```

## Run

As the CI examples job runs it, from the repository root:

```bash
javac ... && java --enable-native-access=ALL-UNNAMED ...
```

## The examples

| Example | What it does |
|---------|--------------|
| `PaperTrade.java` | Paper-trade differentiator demo. |
