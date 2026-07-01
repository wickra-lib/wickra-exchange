//! A deterministic replay exchange.
//!
//! [`ReplayExchange`] drives a **recorded** market-data feed (golden / replay
//! fixtures) through the very same [`Exchange`] trait as a live venue, filling
//! orders against the replayed prices through an embedded [`PaperExchange`]. It
//! is the bridge between the two other differentiators: recorded microstructure
//! in, paper fills out — a backtest on the real tape, driven by the exact same
//! strategy code that runs live.
//!
//! Each [`poll_events`](MarketData::poll_events) advances the recording by one
//! frame: a price-bearing frame (ticker, trade, or book snapshot) updates the
//! internal mark that fills execute against, and the frame is returned to the
//! caller alongside any order/balance events the fills produced. When the
//! recording is exhausted, `poll_events` yields nothing further.

use crate::error::Result;
use crate::events::{Event, OrderBookSnapshot};
use crate::exchanges::paper::PaperExchange;
use crate::symbol::Symbol;
use crate::traits::{Exchange, Execution, MarketData};
use crate::types::{Balance, Order, OrderRequest, Ticker};
use wickra_core::Candle;

/// A network-free exchange that replays a recorded feed and fills against it.
/// See the [module docs](self) for the model.
pub struct ReplayExchange {
    frames: Vec<Event>,
    cursor: usize,
    paper: PaperExchange,
}

impl ReplayExchange {
    /// Replay `frames` against a frictionless paper account. Seed balances and
    /// costs with [`with_paper`](Self::with_paper).
    #[must_use]
    pub fn new(frames: Vec<Event>) -> Self {
        Self::with_paper(frames, PaperExchange::new())
    }

    /// Replay `frames`, filling against a caller-configured [`PaperExchange`]
    /// (its balances, fees and slippage).
    #[must_use]
    pub fn with_paper(frames: Vec<Event>, paper: PaperExchange) -> Self {
        Self {
            frames,
            cursor: 0,
            paper,
        }
    }

    /// Whether every recorded frame has been replayed.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.cursor >= self.frames.len()
    }

    /// The number of frames not yet replayed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.frames.len() - self.cursor
    }

    /// Update the mark price a price-bearing frame implies, if any.
    fn absorb(&mut self, event: &Event) {
        match event {
            Event::Ticker(ticker) => self.paper.set_price(&ticker.symbol, ticker.last),
            Event::Trade(trade) => self.paper.set_price(&trade.symbol, trade.price),
            Event::BookSnapshot(book) => {
                if let Some(mid) = book.mid_price() {
                    self.paper.set_price(&book.symbol, mid);
                }
            }
            _ => {}
        }
    }
}

impl MarketData for ReplayExchange {
    fn ticker(&mut self, symbol: &Symbol) -> Result<Ticker> {
        self.paper.ticker(symbol)
    }

    fn klines(&mut self, symbol: &Symbol, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        self.paper.klines(symbol, interval, limit)
    }

    fn order_book(&mut self, symbol: &Symbol, depth: u32) -> Result<OrderBookSnapshot> {
        self.paper.order_book(symbol, depth)
    }

    fn subscribe_trades(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn subscribe_book(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn subscribe_ticker(&mut self, _symbol: &Symbol) -> Result<()> {
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if self.cursor < self.frames.len() {
            let event = self.frames[self.cursor].clone();
            self.cursor += 1;
            self.absorb(&event);
            out.push(event);
        }
        // Surface any order/balance events the updated mark produced.
        out.extend(self.paper.poll_events());
        out
    }
}

impl Execution for ReplayExchange {
    fn place_order(&mut self, request: &OrderRequest) -> Result<Order> {
        self.paper.place_order(request)
    }

    fn cancel_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<()> {
        self.paper.cancel_order(symbol, order_id)
    }

    fn query_order(&mut self, symbol: &Symbol, order_id: &str) -> Result<Order> {
        self.paper.query_order(symbol, order_id)
    }

    fn open_orders(&mut self, symbol: Option<&Symbol>) -> Result<Vec<Order>> {
        self.paper.open_orders(symbol)
    }

    fn balances(&mut self) -> Result<Vec<Balance>> {
        self.paper.balances()
    }
}

impl Exchange for ReplayExchange {
    fn name(&self) -> &'static str {
        "replay"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TradePrint;
    use crate::events::{BookLevel, OrderBookSnapshot};
    use crate::types::{OrderSide, OrderStatus};
    use rust_decimal::prelude::ToPrimitive;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use wickra_core::{Indicator, Sma};

    fn sym() -> Symbol {
        Symbol::new("BTC", "USDT")
    }

    fn trade_frame(price: Decimal, ts: i64) -> Event {
        Event::Trade(TradePrint {
            symbol: sym(),
            price,
            quantity: dec!(1),
            aggressor: OrderSide::Buy,
            timestamp: ts,
        })
    }

    #[test]
    fn replays_frames_then_finishes() {
        let paper = PaperExchange::new().with_balance("USDT", dec!(1000));
        let mut ex = ReplayExchange::with_paper(
            vec![
                Event::Subscribed {
                    channel: "trade".to_string(),
                },
                trade_frame(dec!(100), 1),
            ],
            paper,
        );
        assert_eq!(ex.remaining(), 2);

        // A non-price frame passes through and sets no mark.
        let first = ex.poll_events();
        assert!(matches!(first[0], Event::Subscribed { .. }));

        // The trade frame sets the mark; the ticker now reports it.
        let second = ex.poll_events();
        assert!(matches!(second[0], Event::Trade(_)));
        assert_eq!(ex.ticker(&sym()).unwrap().last, dec!(100));

        assert!(ex.is_finished());
        assert!(ex.poll_events().is_empty());
        assert_eq!(ex.name(), "replay");
    }

    #[test]
    fn fills_against_the_replayed_mark() {
        let paper = PaperExchange::new().with_balance("USDT", dec!(10000));
        let mut ex = ReplayExchange::with_paper(vec![trade_frame(dec!(2000), 1)], paper);
        ex.poll_events(); // advance the mark to 2000

        let order = ex
            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
            .unwrap();
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.average_price, Some(dec!(2000)));
        // The fill's order + balance events surface on the next poll.
        let events = ex.poll_events();
        assert!(events.iter().any(|e| matches!(e, Event::OrderUpdate(_))));
    }

    #[test]
    fn subscriptions_are_inert_and_delegate_market_data() {
        let mut ex = ReplayExchange::new(vec![]);
        assert!(ex.subscribe_trades(&sym()).is_ok());
        assert!(ex.subscribe_book(&sym()).is_ok());
        assert!(ex.subscribe_ticker(&sym()).is_ok());
        // No mark set yet: delegated market data errors like the paper exchange.
        assert!(ex.ticker(&sym()).is_err());
        assert!(ex.klines(&sym(), "1m", 1).is_err());
        assert!(ex.order_book(&sym(), 1).is_err());
    }

    #[test]
    fn book_snapshot_frame_sets_the_mid_mark() {
        let paper = PaperExchange::new().with_balance("USDT", dec!(1000));
        let snapshot = Event::BookSnapshot(OrderBookSnapshot {
            symbol: sym(),
            last_update_id: 1,
            bids: vec![BookLevel::new(dec!(100), dec!(1))],
            asks: vec![BookLevel::new(dec!(102), dec!(1))],
        });
        let mut ex = ReplayExchange::with_paper(vec![snapshot], paper);
        ex.poll_events();
        // The mid of 100/102 becomes the mark.
        assert_eq!(ex.ticker(&sym()).unwrap().last, dec!(101));
    }

    #[test]
    fn open_orders_and_cancel_delegate_to_paper() {
        let paper = PaperExchange::new().with_balance("USDT", dec!(100000));
        let mut ex = ReplayExchange::with_paper(vec![trade_frame(dec!(20000), 1)], paper);
        ex.poll_events();
        let order = ex
            .place_order(&OrderRequest::limit_buy(sym(), dec!(1), dec!(19000)))
            .unwrap();
        assert_eq!(ex.open_orders(None).unwrap().len(), 1);
        ex.cancel_order(&sym(), &order.id).unwrap();
        assert!(ex.open_orders(None).unwrap().is_empty());
        assert_eq!(
            ex.query_order(&sym(), &order.id).unwrap().status,
            OrderStatus::Canceled
        );
        assert!(!ex.balances().unwrap().is_empty());
    }

    /// End-to-end: a recorded tape drives a wickra-core indicator, whose signal
    /// places an order that fills on the paper book — the full paper↔live↔replay
    /// path in one deterministic test.
    #[test]
    fn recorded_tape_drives_indicator_signal_to_a_fill() {
        // A recorded price tape that rises through an SMA.
        let tape = [
            dec!(100),
            dec!(101),
            dec!(102),
            dec!(110), // breaks clearly above the 3-period SMA
            dec!(112),
        ];
        let frames = tape
            .iter()
            .enumerate()
            .map(|(i, &price)| trade_frame(price, i64::try_from(i).unwrap()))
            .collect();

        let paper = PaperExchange::new().with_balance("USDT", dec!(100000));
        let mut ex = ReplayExchange::with_paper(frames, paper);

        let mut sma = Sma::new(3).unwrap();
        let mut bought = false;

        while !ex.is_finished() {
            for event in ex.poll_events() {
                let Event::Trade(trade) = event else { continue };
                let price_f64 = trade.price.to_f64().unwrap();
                // Feed the recorded print into the indicator.
                if let Some(mean) = sma.update(price_f64) {
                    // Signal: price breaks above the moving average -> go long once.
                    if !bought && price_f64 > mean {
                        let order = ex
                            .place_order(&OrderRequest::market_buy(sym(), dec!(1)))
                            .unwrap();
                        assert_eq!(order.status, OrderStatus::Filled);
                        bought = true;
                    }
                }
            }
        }

        assert!(bought, "the rising tape should have crossed the SMA");
        // The strategy now holds one BTC, funded from the paper USDT balance.
        let btc = ex
            .balances()
            .unwrap()
            .into_iter()
            .find(|b| b.asset == "BTC")
            .unwrap();
        assert_eq!(btc.free, dec!(1));
    }
}
