//! The transport every adapter sends through. `Transport` is the one seam: `Client` is the HTTPS
//! client in the binary (rustls with the ring provider and the platform's trust roots, a request
//! timeout, same-origin redirects only: another host, scheme or port is `error: provider`); the
//! replay player and recorder in `replay` implement it for fixtures. A response of any status
//! comes back as `Response`; `Response::ok` turns a non-2xx into `Error::Transport` with the
//! status, `retry-after` and, for a 400, whether the adapter reads the body as naming no cause,
//! so the run loop decides about retries. Messages never carry the URL or headers.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use squint_core::retry::TransportError;
use squint_core::{Error, Result};

/// Same-origin redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 5;
/// Body bytes kept in a transport error message.
const ERROR_BODY_BYTES: usize = 2048;

/// Header name and value pairs as an adapter builds them.
pub type Headers = Vec<(String, String)>;

pub type TransportFuture<'a> = Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>;

/// One HTTPS round trip. Implementations never retry; the run loop owns that.
pub trait Transport: Send + Sync {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a serde_json::Value,
    ) -> TransportFuture<'a>;
}

/// A response of any status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Response headers with lowercased names; a repeated header keeps its last value.
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// The server's `retry-after` in seconds, when it gave one.
    pub fn retry_after(&self) -> Option<Duration> {
        self.header("retry-after")
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
    }

    /// 2xx is `Ok`; anything else is a transport error carrying the status, `retry-after` and the
    /// start of the body, for the run loop's retry decision. `names_no_cause` is the adapter's
    /// reading of its provider's error body; it is asked only about a 400 with a non-empty body
    /// (an empty one names no cause whoever sent it).
    pub fn ok(self, names_no_cause: fn(&str) -> bool) -> Result<Response> {
        if (200..300).contains(&self.status) {
            return Ok(self);
        }
        let causeless =
            self.status == 400 && (self.body.trim().is_empty() || names_no_cause(&self.body));
        let excerpt: String = self.body.chars().take(ERROR_BODY_BYTES).collect();
        Err(Error::Transport(TransportError {
            status: Some(self.status),
            retry_after: self.retry_after(),
            message: format!("HTTP {}: {excerpt}", self.status),
            sent: true,
            causeless,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    inner: reqwest::Client,
}

impl Client {
    pub fn new(timeout: Duration) -> Result<Client> {
        // The provider is process-wide; a second installation (tests, embedders) is harmless.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let inner = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(same_origin))
            .user_agent(concat!("squint/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Provider(format!("cannot build the HTTP client: {e}")))?;
        Ok(Client { inner })
    }

    async fn send(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
    ) -> Result<Response> {
        // The body is serialised here and the headers are exactly the adapter's: it names the
        // content type itself, so nothing is added or doubled.
        let bytes = serde_json::to_vec(body)?;
        let mut req = self.inner.post(url).body(bytes);
        for (name, value) in headers {
            req = req.header(name.as_str(), value.as_str());
        }
        let response = req.send().await.map_err(map_send_error)?;
        let status = response.status().as_u16();
        let mut out = BTreeMap::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                out.insert(name.as_str().to_ascii_lowercase(), v.to_string());
            }
        }
        let body = response.text().await.map_err(|e| {
            Error::Transport(TransportError {
                status: Some(status),
                retry_after: None,
                message: format!("reading the response body: {}", strip_url(e)),
                sent: true,
                causeless: false,
            })
        })?;
        Ok(Response {
            status,
            headers: out,
            body,
        })
    }
}

impl Client {
    /// Fetches a resource by GET under the same redirect policy as requests: the body of a 2xx
    /// response, or a transport error carrying the status. Used by `squint sets fetch` for
    /// evaluation-set images; no header beyond the user agent is sent.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.inner.get(url).send().await.map_err(map_send_error)?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|e| {
            Error::Transport(TransportError {
                status: Some(status),
                retry_after: None,
                message: format!("reading the response body: {}", strip_url(e)),
                sent: true,
                causeless: false,
            })
        })?;
        if !(200..300).contains(&status) {
            return Err(Error::Transport(TransportError {
                status: Some(status),
                retry_after: None,
                message: format!("HTTP {status}"),
                sent: true,
                causeless: false,
            }));
        }
        Ok(bytes.to_vec())
    }
}

impl Transport for Client {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a serde_json::Value,
    ) -> TransportFuture<'a> {
        Box::pin(self.send(url, headers, body))
    }
}

/// Follow a redirect only to the same scheme, host and port, at most `MAX_REDIRECTS` times.
fn same_origin(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    let Some(first) = attempt.previous().first() else {
        return attempt.stop();
    };
    let next = attempt.url();
    let same = next.scheme() == first.scheme()
        && next.host_str() == first.host_str()
        && next.port_or_known_default() == first.port_or_known_default();
    if !same {
        let host = next.host_str().unwrap_or("?").to_string();
        return attempt.error(format!("redirect to another host refused: {host}"));
    }
    if attempt.previous().len() > MAX_REDIRECTS {
        return attempt.error("too many redirects");
    }
    attempt.follow()
}

fn map_send_error(e: reqwest::Error) -> Error {
    let status = e.status().map(|s| s.as_u16());
    let (redirect, timeout, connect) = (e.is_redirect(), e.is_timeout(), e.is_connect());
    let detail = strip_url(e);
    if redirect {
        return Error::Provider(format!("redirect refused: {detail}"));
    }
    let message = if timeout {
        "timed out".to_string()
    } else if connect {
        format!("connection failed: {detail}")
    } else {
        detail
    };
    Error::Transport(TransportError {
        status,
        retry_after: None,
        message,
        // A connection that failed carried no request; a timeout may have.
        sent: !connect,
        causeless: false,
    })
}

/// reqwest's messages name the URL, which may carry a key in its query for some providers; the
/// chain of sources without the URL is enough.
fn strip_url(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut parts = vec![e.to_string()];
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    while let Some(s) = source {
        parts.push(s.to_string());
        source = s.source();
    }
    parts.join(": ")
}
