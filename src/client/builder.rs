//! Fluent client builder for [`AsyncClient`] and [`Client`].

use crate::auth::AuthMethod;
use crate::client::async_client::AsyncClient;
use crate::client::blocking::Client;
use crate::client::config::ClientConfig;
use crate::exceptions::{HttpxError, Result};
use crate::middleware::{RedirectConfig, RetryConfig, TimeoutConfig};
use crate::proxy::ProxyMounts;
use crate::request::{CookieJar, Headers};
use crate::transport::{ClientCertConfig, PoolLimits, TlsConfig, TlsVerify};
use std::sync::Arc;
use tokio::runtime::Runtime;
use url::Url;

/// Builder for configuring and instantiating [`AsyncClient`] or [`Client`].
#[derive(Debug, Clone, Default)]
pub struct ClientBuilder {
    config: ClientConfig,
}

impl ClientBuilder {
    /// Creates a new [`ClientBuilder`] with HTTPX-compatible defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the base URL used to resolve relative request URLs.
    pub fn base_url(mut self, url: &str) -> Result<Self> {
        let parsed = Url::parse(url)
            .map_err(|e| HttpxError::InvalidUrl(format!("Invalid base_url '{url}': {e}")))?;
        self.config.base_url = Some(parsed);
        Ok(self)
    }

    /// Sets default headers applied to every request.
    pub fn default_headers(mut self, headers: Headers) -> Self {
        self.config.headers = headers;
        self
    }

    /// Inserts a single default header.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self> {
        self.config.headers.insert(name, value)?;
        Ok(self)
    }

    /// Sets the shared cookie jar.
    pub fn cookies(mut self, cookies: CookieJar) -> Self {
        self.config.cookies = cookies;
        self
    }

    /// Sets default query parameters appended to every request.
    pub fn params(mut self, params: Vec<(String, String)>) -> Self {
        self.config.params = params;
        self
    }

    /// Sets default authentication credentials.
    pub fn auth(mut self, auth: AuthMethod) -> Self {
        self.config.auth = Some(auth);
        self
    }

    /// Sets default timeout configuration.
    pub fn timeout(mut self, timeout: TimeoutConfig) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Sets connection pool limits.
    pub fn limits(mut self, limits: PoolLimits) -> Self {
        self.config.limits = limits;
        self
    }

    /// Configures automatic redirect following.
    pub fn follow_redirects(mut self, follow: bool) -> Self {
        self.config.redirects.follow_redirects = follow;
        self
    }

    /// Sets the maximum number of redirect hops.
    pub fn max_redirects(mut self, max: usize) -> Self {
        self.config.redirects.max_redirects = max;
        self
    }

    /// Configures transport retries on transient connection errors.
    pub fn retries(mut self, retries: RetryConfig) -> Self {
        self.config.retries = retries;
        self
    }

    /// Sets TLS verification mode (`Default`, `CustomCa`, or `Disabled`).
    pub fn verify(mut self, verify: TlsVerify) -> Self {
        self.config.tls.verify = verify;
        self
    }

    /// Sets a client mTLS certificate and optional key path.
    pub fn cert(mut self, cert: ClientCertConfig) -> Self {
        self.config.tls.client_cert = Some(cert);
        self
    }

    /// Enables or disables HTTP/2 negotiation via ALPN.
    pub fn http2(mut self, enabled: bool) -> Self {
        self.config.tls.http2 = enabled;
        self
    }

    /// Enables cleartext HTTP/2 (`h2c`) prior knowledge when talking to `http://` servers.
    pub fn http2_prior_knowledge(mut self, enabled: bool) -> Self {
        self.config.http2_prior_knowledge = enabled;
        self
    }

    /// Sets explicit proxy routing configuration.
    pub fn proxies(mut self, proxies: ProxyMounts) -> Self {
        self.config.proxies = Some(proxies);
        self
    }

    /// Enables or disables reading environment variables (`HTTP_PROXY`, `HTTPS_PROXY`, etc.).
    pub fn trust_env(mut self, trust: bool) -> Self {
        self.config.trust_env = trust;
        self
    }

    /// Applies full [`RedirectConfig`].
    pub fn redirect_config(mut self, redirects: RedirectConfig) -> Self {
        self.config.redirects = redirects;
        self
    }

    /// Applies full [`TlsConfig`].
    pub fn tls_config(mut self, tls: TlsConfig) -> Self {
        self.config.tls = tls;
        self
    }

    /// Builds an asynchronous [`AsyncClient`].
    pub fn build_async(self) -> Result<AsyncClient> {
        AsyncClient::from_config(self.config)
    }

    /// Builds a blocking [`Client`] backed by the provided shared Tokio runtime.
    pub fn build_blocking(self, runtime: Arc<Runtime>) -> Result<Client> {
        Client::from_config(self.config, runtime)
    }
}
