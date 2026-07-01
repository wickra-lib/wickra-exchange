# Cookbook

## Paper trade (offline, deterministic)

```rust
use wickra_exchange::{Execution, MarketData, OrderRequest, PaperExchange, Symbol};
use rust_decimal_macros::dec;

let mut ex = PaperExchange::new()
    .with_fees(dec!(1), dec!(5))       // maker / taker bps
    .with_slippage_bps(dec!(10))
    .with_balance("USDT", dec!(100000));
ex.set_price(&Symbol::new("BTC", "USDT"), dec!(20000));

let order = ex.place_order(&OrderRequest::market_buy(Symbol::new("BTC", "USDT"), dec!(1)))?;
assert_eq!(order.average_price, Some(dec!(20020)));  // 20000 * 1.001
```

## Backtest on a recorded tape (replay)

```rust
use wickra_exchange::{Event, MarketData, ReplayExchange, PaperExchange};

let paper = PaperExchange::new().with_balance("USDT", dec!(100000));
let mut ex = ReplayExchange::with_paper(recorded_frames, paper);
while !ex.is_finished() {
    for event in ex.poll_events() {
        // feed prices to a wickra-core indicator, place orders on signal
    }
}
```

## Swap paper → live

The strategy code is identical; only the constructor changes:

```rust
let mut ex: Box<dyn Exchange> = if live {
    wickra_exchange::connect("binance", credentials, &options)?
} else {
    Box::new(PaperExchange::new().with_balance("USDT", dec!(100000)))
};
```

## Convert venue feeds to indicator inputs

```rust
use wickra_exchange::{trade_from_print, DerivativesTickBuilder, DerivativesFeed};

let core_trade = trade_from_print(&trade_print)?;   // -> wickra_core::Trade
let mut builder = DerivativesTickBuilder::new();
builder.apply(&DerivativesFeed::MarkIndex(mark_index));
let tick = builder.build()?;                        // -> wickra_core::DerivativesTick
```
