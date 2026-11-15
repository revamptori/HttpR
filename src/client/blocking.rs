//! Synchronous (blocking) HTTP client backed by a shared Tokio runtime.

use crate::client::async_client::AsyncClient;
use crate::client::builder::ClientBuilder;
use crate::client::config::{ClientConfig, SendOptions};
use crate::exceptions::{HttpxError, Result};
use crate::middleware::TimeoutConfig;
use crate::request::{CookieJar, Headers, Request, RequestBuilder};
use crate::response::Response;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::runtime::{Builder, Runtime};

static SHARED_RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

/// Returns the process-wide shared multi-threaded Tokio runtime.
///
/// Reusing a single runtime avoids per-request runtime creation overhead and preserves
/// connection pools across synchronous requests.
pub fn shared_runtime() -> Result<Arc<Runtime>> {
    if let Some(rt) = SHARED_RUNTIME.get() {
        return Ok(Arc::clone(rt));
    }
    let rt = Builder::new_multi_thread()
        .enable_all()
        .thread_name("httpx-rs-worker")
        .build()
        .map_err(|e| HttpxError::HttpError(format!("Failed to initialize Tokio runtime: {e}")))?;
    let arc_rt = Arc::new(rt);
    let _ = SHARED_RUNTIME.set(Arc::clone(&arc_rt));
    Ok(SHARED_RUNTIME
        .get()
        .map(Arc::clone)
        .unwrap_or(arc_rt))
}

/// Synchronous HTTP client matching HTTPX's `Client` API.
#[derive(Clone)]
pub struct Client {
    inner: AsyncClient,
    runtime: Arc<Runtime>,
}

impl Client {
    /// Creates a blocking [`Client`] using default configuration and the shared Tokio runtime.
    pub fn new() -> Result<Self> {
        Self::from_config(ClientConfig::default(), shared_runtime()?)
    }

    /// Returns a [`ClientBuilder`] for customizing client options.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Creates a blocking [`Client`] from `config` and an explicit shared [`Runtime`].
    pub fn from_config(config: ClientConfig, runtime: Arc<Runtime>) -> Result<Self> {
        let inner = AsyncClient::from_config(config)?;
        Ok(Self { inner, runtime })
    }

    /// Returns a reference to the underlying [`AsyncClient`].
    pub fn as_async(&self) -> &AsyncClient {
        &self.inner
    }

    /// Returns a reference to the shared Tokio runtime.
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Returns the client's shared [`CookieJar`].
    pub fn cookies(&self) -> &CookieJar {
        self.inner.cookies()
    }

    /// Returns the client's default [`Headers`].
    pub fn headers(&self) -> &Headers {
        self.inner.headers()
    }

    /// Returns `true` if [`Client::close`] has been called.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Closes all pooled connections synchronously.
    pub fn close(&self) {
        self.runtime.block_on(self.inner.aclose());
    }

    /// Builds a [`RequestBuilder`] using the client's defaults.
    pub fn build_request(
        &self,
        method: &str,
        url: &str,
        extra_cookies: Option<&HashMap<String, String>>,
    ) -> Result<RequestBuilder> {
        self.inner.build_request(method, url, extra_cookies)
    }

    /// Sends a pre-built [`Request`] synchronously.
    pub fn send(&self, request: Request, options: SendOptions) -> Result<Response> {
        self.runtime.block_on(self.inner.send(request, options))
    }

    /// Sends a request for `method` and `url` synchronously.
    pub fn request(&self, method: &str, url: &str) -> Result<Response> {
        self.runtime.block_on(self.inner.request(method, url))
    }

    /// Sends an HTTP `GET` request synchronously.
    pub fn get(&self, url: &str) -> Result<Response> {
        self.request("GET", url)
    }

    /// Sends an HTTP `POST` request synchronously.
    pub fn post(&self, url: &str) -> Result<Response> {
        self.request("POST", url)
    }

    /// Sends an HTTP `PUT` request synchronously.
    pub fn put(&self, url: &str) -> Result<Response> {
        self.request("PUT", url)
    }

    /// Sends an HTTP `PATCH` request synchronously.
    pub fn patch(&self, url: &str) -> Result<Response> {
        self.request("PATCH", url)
    }

    /// Sends an HTTP `DELETE` request synchronously.
    pub fn delete(&self, url: &str) -> Result<Response> {
        self.request("DELETE", url)
    }

    /// Sends an HTTP `HEAD` request synchronously.
    pub fn head(&self, url: &str) -> Result<Response> {
        self.request("HEAD", url)
    }

    /// Sends an HTTP `OPTIONS` request synchronously.
    pub fn options(&self, url: &str) -> Result<Response> {
        self.request("OPTIONS", url)
    }

    /// Sends a request and returns an unbuffered streaming [`Response`].
    pub fn stream(&self, request: Request, timeout: Option<TimeoutConfig>) -> Result<Response> {
        self.runtime
            .block_on(self.inner.stream(request, timeout))
    }
}
