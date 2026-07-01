//! The transport abstraction: the seam between exchange logic and real sockets.
//!
//! Every exchange implementation issues HTTP requests and reads WebSocket frames
//! through these traits rather than touching a socket directly. Production wires
//! a thin real-socket adapter; tests — and users who want to exercise their own
//! trading code offline — wire [`MockHttpTransport`] / [`MockWsTransport`], which
//! replay canned responses and frames. Because all exchange logic runs through
//! this seam, signing, parsing, filter rounding, the WebSocket state machine,
//! reconnect and error handling are all exercisable without a live network.

use crate::error::{Error, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `PATCH`.
    Patch,
    /// `DELETE`.
    Delete,
}

impl HttpMethod {
    /// The uppercase method name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
}

/// An outbound HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The HTTP method.
    pub method: HttpMethod,
    /// The fully-qualified URL, including any query string.
    pub url: String,
    /// Request headers.
    pub headers: Vec<(String, String)>,
    /// Optional request body.
    pub body: Option<String>,
}

impl HttpRequest {
    /// A `GET` request to `url`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// A `POST` request to `url`.
    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// A request with an arbitrary method.
    pub fn new(method: HttpMethod, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Add a header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the body.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// An HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status code.
    pub status: u16,
    /// The response body.
    pub body: String,
}

impl HttpResponse {
    /// Build a response.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// Whether the status is in the 2xx range.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A synchronous HTTP transport. The public surface stays blocking; a real
/// adapter drives an async client internally, a mock returns canned responses.
pub trait HttpTransport: Send + Sync {
    /// Execute a request and return the response.
    ///
    /// # Errors
    ///
    /// Returns a transport-level [`Error`] (network, timeout) on failure; HTTP
    /// error *status codes* are returned as `Ok` responses for the caller to
    /// interpret against the venue's error taxonomy.
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse>;
}

/// A WebSocket transport: opens connections to stream URLs.
pub trait WsTransport: Send + Sync {
    /// Open a connection to `url`.
    ///
    /// # Errors
    ///
    /// Returns a transport-level [`Error`] if the connection cannot be opened.
    fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>>;
}

/// A single WebSocket connection. `recv` returns `Ok(None)` when the peer closed
/// the connection cleanly.
pub trait WsConnection: Send {
    /// Send a text frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotConnected`] if the connection is closed, or a
    /// transport error on failure.
    fn send(&mut self, text: &str) -> Result<()>;

    /// Receive the next text frame, or `Ok(None)` if nothing is pending or the
    /// peer closed the connection. Use [`is_connected`](Self::is_connected) to
    /// distinguish a live-but-idle stream from a closed one.
    ///
    /// # Errors
    ///
    /// Returns a transport-level [`Error`] on a read failure.
    fn recv(&mut self) -> Result<Option<String>>;

    /// Whether the connection is still open. Returns `false` once the peer has
    /// closed it cleanly, so a caller can reconnect and resubscribe.
    fn is_connected(&self) -> bool;

    /// Close the connection.
    ///
    /// # Errors
    ///
    /// Returns a transport-level [`Error`] on failure.
    fn close(&mut self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Mock / replay transports — a first-class offline-testing tool.
// ---------------------------------------------------------------------------

/// An [`HttpTransport`] that returns queued responses and records the requests
/// it was given. Queue responses with [`MockHttpTransport::push_response`] /
/// [`push_json`](MockHttpTransport::push_json) / [`push_error`](MockHttpTransport::push_error);
/// they are returned FIFO.
#[derive(Default)]
pub struct MockHttpTransport {
    responses: Mutex<VecDeque<Result<HttpResponse>>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl MockHttpTransport {
    /// An empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a successful response.
    pub fn push_response(&self, response: HttpResponse) {
        self.responses.lock().unwrap().push_back(Ok(response));
    }

    /// Queue a response from a status code and JSON body.
    pub fn push_json(&self, status: u16, body: impl Into<String>) {
        self.push_response(HttpResponse::new(status, body));
    }

    /// Queue a transport-level error.
    pub fn push_error(&self, error: Error) {
        self.responses.lock().unwrap().push_back(Err(error));
    }

    /// The requests executed so far, in order.
    #[must_use]
    pub fn recorded_requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpTransport for MockHttpTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(Error::Network("mock: no queued response".to_string())))
    }
}

/// A [`WsTransport`] whose connections replay a scripted sequence of frames.
///
/// Each call to [`push_connection`](MockWsTransport::push_connection) queues the
/// script (a list of `recv` results) for the next [`connect`](WsTransport::connect).
/// Frames sent by the client are recorded and readable via
/// [`sent`](MockWsTransport::sent).
#[derive(Default)]
pub struct MockWsTransport {
    scripts: Mutex<VecDeque<Vec<Result<Option<String>>>>>,
    connected_urls: Mutex<Vec<String>>,
    sent: Arc<Mutex<Vec<String>>>,
}

impl MockWsTransport {
    /// An empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue the frame script for the next connection. Each entry is the result
    /// of one `recv`; `Ok(Some(frame))` delivers a frame, `Ok(None)` closes the
    /// connection, `Err(..)` surfaces a read error.
    pub fn push_connection(&self, frames: Vec<Result<Option<String>>>) {
        self.scripts.lock().unwrap().push_back(frames);
    }

    /// The URLs connected to so far, in order.
    #[must_use]
    pub fn connected_urls(&self) -> Vec<String> {
        self.connected_urls.lock().unwrap().clone()
    }

    /// The frames the client sent across all connections, in order.
    #[must_use]
    pub fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl WsTransport for MockWsTransport {
    fn connect(&self, url: &str) -> Result<Box<dyn WsConnection>> {
        self.connected_urls.lock().unwrap().push(url.to_string());
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(MockWsConnection {
            incoming: script.into(),
            sent: Arc::clone(&self.sent),
            closed: false,
        }))
    }
}

/// A scripted [`WsConnection`] produced by [`MockWsTransport`].
pub struct MockWsConnection {
    incoming: VecDeque<Result<Option<String>>>,
    sent: Arc<Mutex<Vec<String>>>,
    closed: bool,
}

impl WsConnection for MockWsConnection {
    fn send(&mut self, text: &str) -> Result<()> {
        if self.closed {
            return Err(Error::NotConnected);
        }
        self.sent.lock().unwrap().push(text.to_string());
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<String>> {
        // An explicit `Ok(None)` frame in the script is a peer close; running out
        // of scripted frames just means nothing more is pending (still open).
        match self.incoming.pop_front() {
            Some(Ok(None)) => {
                self.closed = true;
                Ok(None)
            }
            Some(other) => other,
            None => Ok(None),
        }
    }

    fn is_connected(&self) -> bool {
        !self.closed
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_method_strings() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn request_builder() {
        let req = HttpRequest::get("https://api/x")
            .with_header("X-KEY", "abc")
            .with_body("payload");
        assert_eq!(req.method, HttpMethod::Get);
        assert_eq!(req.url, "https://api/x");
        assert_eq!(req.headers, vec![("X-KEY".to_string(), "abc".to_string())]);
        assert_eq!(req.body.as_deref(), Some("payload"));

        let post = HttpRequest::post("https://api/y");
        assert_eq!(post.method, HttpMethod::Post);
        let del = HttpRequest::new(HttpMethod::Delete, "https://api/z");
        assert_eq!(del.method, HttpMethod::Delete);
    }

    #[test]
    fn response_success_classification() {
        assert!(HttpResponse::new(200, "{}").is_success());
        assert!(HttpResponse::new(204, "").is_success());
        assert!(!HttpResponse::new(400, "{}").is_success());
        assert!(!HttpResponse::new(500, "{}").is_success());
    }

    #[test]
    fn mock_http_returns_queued_responses_fifo_and_records() {
        let http = MockHttpTransport::new();
        http.push_json(200, "{\"ok\":1}");
        http.push_error(Error::Timeout);

        let first = http.execute(&HttpRequest::get("https://api/a")).unwrap();
        assert_eq!(first.body, "{\"ok\":1}");
        let second = http.execute(&HttpRequest::post("https://api/b"));
        assert_eq!(second.unwrap_err(), Error::Timeout);

        // Queue exhausted -> network error.
        let third = http.execute(&HttpRequest::get("https://api/c"));
        assert!(matches!(third.unwrap_err(), Error::Network(_)));

        let recorded = http.recorded_requests();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].url, "https://api/a");
        assert_eq!(recorded[1].method, HttpMethod::Post);
    }

    #[test]
    fn mock_ws_replays_frames_then_closes() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![
            Ok(Some("frame-1".to_string())),
            Ok(Some("frame-2".to_string())),
        ]);

        let mut conn = ws.connect("wss://stream/x").unwrap();
        conn.send("subscribe").unwrap();
        assert_eq!(conn.recv().unwrap(), Some("frame-1".to_string()));
        assert_eq!(conn.recv().unwrap(), Some("frame-2".to_string()));
        // Exhausted script reads as a clean close.
        assert_eq!(conn.recv().unwrap(), None);

        assert_eq!(ws.connected_urls(), vec!["wss://stream/x".to_string()]);
        assert_eq!(ws.sent(), vec!["subscribe".to_string()]);
    }

    #[test]
    fn mock_ws_surfaces_read_errors_and_blocks_send_after_close() {
        let ws = MockWsTransport::new();
        ws.push_connection(vec![Err(Error::Network("reset".to_string()))]);
        let mut conn = ws.connect("wss://stream/y").unwrap();
        assert!(matches!(conn.recv().unwrap_err(), Error::Network(_)));

        conn.close().unwrap();
        assert_eq!(conn.send("late").unwrap_err(), Error::NotConnected);
    }

    #[test]
    fn mock_ws_connect_without_script_is_immediately_closed() {
        let ws = MockWsTransport::new();
        let mut conn = ws.connect("wss://stream/empty").unwrap();
        assert_eq!(conn.recv().unwrap(), None);
    }
}
