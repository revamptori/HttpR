//! Network transport layer: HTTP/1.1, HTTP/2, TLS, and connection pooling.

pub mod connection_pool;
pub mod http1;
pub mod http2;
pub mod tls;

pub use connection_pool::{ConnectionPool, PoolLimits};
pub use http1::Http1Connection;
pub use http2::Http2Connection;
pub use tls::{ClientCertConfig, TlsConfig, TlsVerify};

use crate::exceptions::Result;
use crate::middleware::{RetryConfig, TimeoutConfig};
use crate::proxy::ProxyMounts;
use crate::request::Request;
use crate::response::Response;

/// High-level async transport combining connection pooling, proxy routing, and retries.
#[derive(Clone)]
pub struct HttpTransport {
    pool: ConnectionPool,
    proxies: ProxyMounts,
    retry_config: RetryConfig,
}

impl HttpTransport {
    /// Creates a new [`HttpTransport`].
    pub fn new(
        limits: PoolLimits,
        tls_config: TlsConfig,
        proxies: ProxyMounts,
        retry_config: RetryConfig,
        http2_prior_knowledge: bool,
    ) -> Result<Self> {
        let pool = ConnectionPool::new(limits, tls_config, http2_prior_knowledge)?;
        Ok(Self {
            pool,
            proxies,
            retry_config,
        })
    }

    /// Dispatches a single network hop with optional retry-on-connect-failure.
    pub async fn send(&self, request: &Request, timeout: &TimeoutConfig) -> Result<Response> {
        let proxy = self.proxies.select_for_url(&request.url);
        let mut attempt = 0;

        loop {
            match self.pool.send(request, proxy, timeout).await {
                Ok(resp) => return Ok(resp),
                Err(err)
                    if attempt < self.retry_config.max_retries
                        && self.retry_config.is_retryable(&err)
                        && request.body.is_replayable() =>
                {
                    let delay = self.retry_config.backoff_duration(attempt);
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Returns a reference to the underlying [`ConnectionPool`].
    pub fn pool(&self) -> &ConnectionPool {
        &self.pool
    }

    /// Closes all pooled connections.
    pub fn close(&self) {
        self.pool.close();
    }
}
