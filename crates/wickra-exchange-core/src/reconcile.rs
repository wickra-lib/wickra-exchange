//! Order-state reconciliation after a reconnect.
//!
//! While a stream is disconnected, orders can fill or cancel without the client
//! seeing the update. On reconnect the client re-fetches the venue's open orders
//! and compares them to what it believed was open. The dangerous case is an
//! order the client thought was open that the venue no longer lists — it
//! **vanished**, meaning it filled or cancelled unseen, and the caller must
//! query its final state. This module computes that diff.

use crate::types::Order;
use std::collections::HashSet;

/// The result of reconciling local belief against the venue's open orders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciliation {
    /// Open on the venue but unknown locally (placed elsewhere, or a missed ack).
    pub appeared: Vec<String>,
    /// Believed open locally but no longer open on the venue — filled or
    /// cancelled while disconnected. **These need a state query.**
    pub vanished: Vec<String>,
    /// Open in both views.
    pub still_open: Vec<String>,
}

impl Reconciliation {
    /// Whether anything diverged (something appeared or vanished).
    #[must_use]
    pub fn has_divergence(&self) -> bool {
        !self.appeared.is_empty() || !self.vanished.is_empty()
    }
}

/// Reconcile the order ids the client believed were open against the venue's
/// current open orders. Output vectors are sorted for determinism.
#[must_use]
pub fn reconcile_orders(local_open_ids: &[String], venue_open: &[Order]) -> Reconciliation {
    let local: HashSet<&str> = local_open_ids.iter().map(String::as_str).collect();
    let venue: HashSet<&str> = venue_open.iter().map(|o| o.id.as_str()).collect();

    let mut appeared: Vec<String> = venue.difference(&local).map(|s| (*s).to_string()).collect();
    let mut vanished: Vec<String> = local.difference(&venue).map(|s| (*s).to_string()).collect();
    let mut still_open: Vec<String> = local
        .intersection(&venue)
        .map(|s| (*s).to_string())
        .collect();
    appeared.sort();
    vanished.sort();
    still_open.sort();

    Reconciliation {
        appeared,
        vanished,
        still_open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::Symbol;
    use crate::types::{OrderSide, OrderStatus, OrderType};
    use rust_decimal_macros::dec;

    fn order(id: &str) -> Order {
        Order {
            id: id.to_string(),
            client_order_id: None,
            symbol: Symbol::new("BTC", "USDT"),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            status: OrderStatus::New,
            quantity: dec!(1),
            filled_quantity: dec!(0),
            price: Some(dec!(100)),
            average_price: None,
        }
    }

    #[test]
    fn identical_views_have_no_divergence() {
        let local = vec!["a".to_string(), "b".to_string()];
        let venue = vec![order("a"), order("b")];
        let rec = reconcile_orders(&local, &venue);
        assert!(!rec.has_divergence());
        assert_eq!(rec.still_open, vec!["a", "b"]);
        assert!(rec.appeared.is_empty());
        assert!(rec.vanished.is_empty());
    }

    #[test]
    fn detects_vanished_and_appeared() {
        // Locally believed open: a, b, c. Venue now lists: b, c, d.
        let local = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let venue = vec![order("b"), order("c"), order("d")];
        let rec = reconcile_orders(&local, &venue);
        assert!(rec.has_divergence());
        // `a` vanished (filled/cancelled while disconnected).
        assert_eq!(rec.vanished, vec!["a"]);
        // `d` appeared (unknown locally).
        assert_eq!(rec.appeared, vec!["d"]);
        assert_eq!(rec.still_open, vec!["b", "c"]);
    }

    #[test]
    fn empty_inputs() {
        let rec = reconcile_orders(&[], &[]);
        assert!(!rec.has_divergence());
        assert_eq!(rec, Reconciliation::default());
    }
}
