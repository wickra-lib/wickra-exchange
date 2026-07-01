//! Real-socket transport adapters.
//!
//! These implement the core's [`HttpTransport`] (and, later, `WsTransport`) over
//! actual sockets. They are the only part of the library that touches the
//! network, so they are deliberately thin and are **excluded from coverage** —
//! exercised only by gated `#[ignore]` integration tests against live testnets,
//! never by the offline unit suite that drives the mock transports.

use wickra_exchange_core::{
    Error, ExchangeOptions, HttpMethod, HttpRequest, HttpResponse, HttpTransport, Result,
};

/// A synchronous HTTP transport backed by `reqwest`'s blocking client (rustls
/// TLS). The blocking client keeps the public surface synchronous with no tokio
/// runtime for callers to manage.
pub struct ReqwestHttpTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestHttpTransport {
    /// Build a transport from the connection options (timeout, user agent, proxy).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Network`] if the underlying client cannot be constructed.
    pub fn new(options: &ExchangeOptions) -> Result<Self> {
        let mut builder = reqwest::blocking::Client::builder().timeout(options.timeout);
        if let Some(user_agent) = &options.user_agent {
            builder = builder.user_agent(user_agent);
        }
        if let Some(proxy) = &options.proxy {
            let proxy = reqwest::Proxy::all(proxy).map_err(|e| Error::Network(e.to_string()))?;
            builder = builder.proxy(proxy);
        }
        let client = builder.build().map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self { client })
    }
}

fn reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Delete => reqwest::Method::DELETE,
    }
}

impl HttpTransport for ReqwestHttpTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse> {
        let mut builder = self
            .client
            .request(reqwest_method(request.method), &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().map_err(|e| {
            if e.is_timeout() {
                Error::Timeout
            } else {
                Error::Network(e.to_string())
            }
        })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|e| Error::Network(e.to_string()))?;
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wickra_exchange_core::MarketType;

    #[test]
    fn builds_from_options() {
        // Construction is deterministic and offline; the actual socket path is
        // covered by the gated integration test below.
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        assert!(ReqwestHttpTransport::new(&opts).is_ok());
    }

    #[test]
    #[ignore = "hits the network; run explicitly with --ignored"]
    fn live_request_reaches_binance() {
        let opts = ExchangeOptions::mainnet(MarketType::Spot);
        let transport = ReqwestHttpTransport::new(&opts).unwrap();
        let request = HttpRequest::get("https://api.binance.com/api/v3/time");
        let response = transport.execute(&request).unwrap();
        assert!(response.is_success());
        assert!(response.body.contains("serverTime"));
    }
}
