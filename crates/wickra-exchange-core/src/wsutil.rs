//! Shared WebSocket reconnect + resubscribe used by every venue's `poll_events`.
//!
//! Every client stores the exact subscribe messages it has sent. When the peer
//! closes the stream, [`reconnect_if_dropped`] reopens the connection and replays
//! them, so an interrupted subscription transparently resumes — the caller only
//! sees a `Disconnected` followed by a `Reconnected` event in the pull stream.

use crate::events::Event;
use crate::transport::{WsConnection, WsTransport};

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

    events.push(Event::Disconnected);
    *connection = None;

    let Some(ws) = ws else {
        return;
    };
    let Ok(mut fresh) = ws.connect(url) else {
        return;
    };
    for message in subscribe_messages {
        if fresh.send(message).is_err() {
            return; // leave disconnected; the next poll retries
        }
    }
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
