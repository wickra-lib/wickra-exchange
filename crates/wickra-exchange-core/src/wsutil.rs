//! Shared WebSocket reconnect + resubscribe used by every venue's `poll_events`.
//!
//! Every client stores the exact subscribe messages it has sent. When the peer
//! closes the stream, [`reconnect_if_dropped`] reopens the connection and replays
//! them, so an interrupted subscription transparently resumes — the caller only
//! sees a `Disconnected` followed by a `Reconnected` event in the pull stream.

use crate::events::Event;
use crate::transport::{WsConnection, WsTransport};

/// Each call below is written on one line, for the reason `throttle.rs` gives:
/// `llvm-cov` marks the continuation lines of a multi-line macro invocation
/// uncovered even when the call runs.
///
/// The URL is deliberately never logged. A private stream's URL carries its
/// credential in the path -- Binance's user-data socket is
/// `wss://.../ws/<listenKey>`, and a listen key opens the account's order and
/// balance stream. Which venue it is is already clear from the caller's target;
/// what a reader needs here is which of the four outcomes happened.
const TARGET: &str = "wickra_exchange_core::wsutil";

/// If the peer has closed `connection`, reopen it via `ws` at `url` and replay
/// every message in `subscribe_messages`, pushing `Disconnected` then
/// `Reconnected` into `events`.
///
/// A no-op when the connection is still live or nothing was subscribed. On a
/// failed reconnect the connection is left `None`, so the next poll retries.
pub(crate) fn reconnect_if_dropped(
    ws: Option<&dyn WsTransport>,
    url: &str,
    connection: &mut Option<Box<dyn WsConnection>>,
    subscribe_messages: &[String],
    events: &mut Vec<Event>,
) {
    let dropped = connection.as_ref().is_some_and(|c| !c.is_connected());
    if !dropped || subscribe_messages.is_empty() {
        return;
    }

    let subscriptions = subscribe_messages.len();
    tracing::info!(target: TARGET, subscriptions, "stream closed by the peer; reconnecting");
    events.push(Event::Disconnected);
    *connection = None;

    let Some(ws) = ws else {
        tracing::warn!(target: TARGET, "no WebSocket transport is configured; the stream stays closed");
        return;
    };
    let Ok(mut fresh) = ws.connect(url) else {
        tracing::warn!(target: TARGET, "reconnect could not open a connection; the next poll retries");
        return;
    };
    for (replayed, message) in subscribe_messages.iter().enumerate() {
        if fresh.send(message).is_err() {
            tracing::warn!(target: TARGET, replayed, subscriptions, "reconnected, but replaying the subscriptions failed; staying closed");
            return; // leave disconnected; the next poll retries
        }
    }
    tracing::info!(target: TARGET, subscriptions, "reconnected and replayed every subscription");
    *connection = Some(fresh);
    events.push(Event::Reconnected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockWsTransport;

    #[test]
    fn reopens_and_replays_subscribes_on_close() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![Ok(None)]); // first stream: closes on recv
        ws.push_connection(vec![]); // reconnect target: open

        let mut connection = Some(ws.connect("wss://x").unwrap());
        connection.as_mut().unwrap().recv().ok(); // pop Ok(None) -> peer closed
        assert!(!connection.as_ref().unwrap().is_connected());

        let mut events = Vec::new();
        reconnect_if_dropped(
            Some(&ws),
            "wss://x",
            &mut connection,
            &["sub".to_string()],
            &mut events,
        );

        assert_eq!(events, vec![Event::Disconnected, Event::Reconnected]);
        assert!(connection.is_some());
        assert_eq!(ws.connected_urls().len(), 2); // initial + reconnect
        assert_eq!(ws.sent(), vec!["sub".to_string()]); // resubscribed
    }

    #[test]
    fn soak_survives_many_reconnect_cycles() {
        // Every stream closes immediately; the helper must reconnect and
        // resubscribe on each cycle without leaking or panicking.
        const CYCLES: usize = 200;
        let ws = MockWsTransport::new();
        for _ in 0..=CYCLES {
            ws.push_connection(vec![Ok(None)]);
        }

        let mut connection = Some(ws.connect("wss://x").unwrap());
        let subs = vec!["sub".to_string()];

        for _ in 0..CYCLES {
            connection.as_mut().unwrap().recv().ok(); // peer closes
            let mut events = Vec::new();
            reconnect_if_dropped(Some(&ws), "wss://x", &mut connection, &subs, &mut events);
            assert_eq!(events, vec![Event::Disconnected, Event::Reconnected]);
            assert!(connection.is_some());
        }

        // One initial connect + one per cycle; a fresh SUBSCRIBE replayed each time.
        assert_eq!(ws.connected_urls().len(), CYCLES + 1);
        assert_eq!(ws.sent().len(), CYCLES);
    }

    /// A dropped stream with a client that has no transport to reconnect with.
    ///
    /// The caller is told the stream went down and never told it came back, and
    /// no amount of polling will change that: the client was built without a
    /// `WsTransport`. Nothing exercised this before, so the one outcome that is
    /// a mistake in the caller's own code looked like every other failure.
    #[test]
    fn a_client_without_a_transport_reports_the_drop_and_stops() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![Ok(None)]);
        let mut connection = Some(ws.connect("wss://x").unwrap());
        connection.as_mut().unwrap().recv().ok();

        let mut events = Vec::new();
        reconnect_if_dropped(
            None,
            "wss://x",
            &mut connection,
            &["sub".into()],
            &mut events,
        );

        assert_eq!(events, vec![Event::Disconnected]);
        assert!(connection.is_none(), "no transport, so nothing to reopen");
    }

    /// The venue refuses the reconnect. The next poll retries, so the connection
    /// is left empty rather than half-built.
    #[test]
    fn a_refused_reconnect_leaves_the_connection_closed() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![Ok(None)]);
        ws.push_refused_connection();

        let mut connection = Some(ws.connect("wss://x").unwrap());
        connection.as_mut().unwrap().recv().ok();

        let mut events = Vec::new();
        reconnect_if_dropped(
            Some(&ws),
            "wss://x",
            &mut connection,
            &["sub".into()],
            &mut events,
        );

        assert_eq!(events, vec![Event::Disconnected]);
        assert!(connection.is_none(), "a failed reopen must not be kept");
        assert_eq!(ws.connected_urls().len(), 2, "it did try");
    }

    /// The socket reopens and the subscriptions cannot be replayed.
    ///
    /// The worst of the four outcomes, and the reason the logging exists: a
    /// caller that kept this connection would hold a live socket subscribed to
    /// nothing, which delivers no events and no errors -- indistinguishable
    /// from a quiet market. So it is not kept, and no `Reconnected` is claimed.
    #[test]
    fn a_reconnect_that_cannot_resubscribe_is_not_reported_as_recovered() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![Ok(None)]);
        ws.push_unsendable_connection();

        let mut connection = Some(ws.connect("wss://x").unwrap());
        connection.as_mut().unwrap().recv().ok();

        let mut events = Vec::new();
        reconnect_if_dropped(
            Some(&ws),
            "wss://x",
            &mut connection,
            &["sub-a".into(), "sub-b".into()],
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::Disconnected],
            "a socket subscribed to nothing is not a recovery"
        );
        assert!(connection.is_none());
        assert!(ws.sent().is_empty(), "not one subscribe frame landed");
    }

    #[test]
    fn live_connection_is_left_untouched() {
        let ws = MockWsTransport::new();
        let mut connection = Some(ws.connect("wss://x").unwrap()); // open (no close frame)
        let mut events = Vec::new();
        reconnect_if_dropped(
            Some(&ws),
            "wss://x",
            &mut connection,
            &["sub".to_string()],
            &mut events,
        );
        assert!(events.is_empty());
        assert!(connection.is_some());
    }
}
