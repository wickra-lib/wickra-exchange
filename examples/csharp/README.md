# Wickra Exchange examples — C#

Runnable C# examples for the [Wickra Exchange C# binding](../../bindings/csharp). The binding consumes the C ABI
library through P/Invoke, so build it once before running anything:

```bash
cargo build -p wickra-exchange-c --release
```

## Run

As the CI examples job runs it, from the repository root:

```bash
dotnet run --project examples/csharp/<Example>
```

## The examples

| Example | What it does |
|---------|--------------|
| `Program.cs` | Paper-trade differentiator demo. |
