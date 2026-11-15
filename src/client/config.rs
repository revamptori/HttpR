//! Configuration structures for synchronous and asynchronous HTTP clients.

use crate::auth::AuthMethod;
use crate::middleware::{RedirectConfig, RetryConfig, TimeoutConfig};
use crate::proxy::ProxyMounts;
use crate::request::{CookieJar, Headers};
use crate::transport::{PoolLimits, TlsConfig};
use url::Url;

/// Complete configuration for [`crate::client::AsyncClient`] and [`crate::client::Client`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: Option<Url>,
    pub headers: Headers,
    pub cookies: CookieJar,
    pub params: Vec<(String, String)>,
    pub auth: Option<AuthMethod>,
    pub timeout: TimeoutConfig,
    pub limits: PoolLimits,
    pub redirects: RedirectConfig,
    pub retries: RetryConfig,
    pub tls: TlsConfig,
    pub proxies: Option<ProxyMounts>,
    pub trust_env: bool,
    pub http2_prior_knowledge: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            headers: Headers::new(),
            cookies: CookieJar::new(),
            params: Vec::new(),
            auth: None,
            timeout: TimeoutConfig::default(),
            limits: PoolLimits::default(),
            redirects: RedirectConfig::default(),
            retries: RetryConfig::default(),
            tls: TlsConfig::default(),
            proxies: None,
            trust_env: true,
            http2_prior_knowledge: false,
        }
    }
}

/// Per-request execution options overriding client defaults.
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    pub auth: Option<AuthMethod>,
    pub follow_redirects: Option<bool>,
    pub timeout: Option<TimeoutConfig>,
    pub stream: bool,
}
