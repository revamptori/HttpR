//! Async connection pool supporting HTTP/1.1 keep-alive, HTTP/2 multiplexing,
//! IPv4/IPv6 DNS resolution, TLS, and HTTP/HTTPS/SOCKS5 proxy tunneling.

use crate::exceptions::{HttpxError, Result};
use crate::middleware::timeout::{with_connect_timeout, with_pool_timeout, TimeoutConfig};
use crate::proxy::{ProxyConfig, ProxyScheme};
use crate::request::Request;
use crate::response::Response;
use crate::transport::http1::{open_http_connect_tunnel, Http1Connection};
use crate::transport::http2::Http2Connection;
use crate::transport::tls::TlsConfig;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{lookup_host, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsConnector;
use tokio_socks::tcp::Socks5Stream;
use url::Url;

/// Connection pool limits matching HTTPX `Limits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLimits {
    pub max_connections: Option<usize>,
    pub max_keepalive_connections: Option<usize>,
    pub keepalive_expiry: Option<Duration>,
}

impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            max_connections: Some(100),
            max_keepalive_connections: Some(20),
            keepalive_expiry: Some(Duration::from_secs(5)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    scheme: String,
    host: String,
    port: u16,
    proxy_url: Option<String>,
}

enum PooledProtocol {
    Http1 {
        conn: Http1Connection,
        proxy_abs_form: Option<ProxyConfig>,
    },
    Http2(Http2Connection),
}

struct PoolInner {
    h1_idle: HashMap<PoolKey, VecDeque<(Http1Connection, Option<ProxyConfig>)>>,
    h2_active: HashMap<PoolKey, Http2Connection>,
    total_idle_h1: usize,
}

impl PoolInner {
    fn new() -> Self {
        Self {
            h1_idle: HashMap::new(),
            h2_active: HashMap::new(),
            total_idle_h1: 0,
        }
    }

    fn take_h2(&mut self, key: &PoolKey) -> Option<Http2Connection> {
        if let Some(conn) = self.h2_active.get(key) {
            if conn.is_usable() {
                return Some(conn.clone());
            }
            self.h2_active.remove(key);
        }
        None
    }

    fn take_h1(
        &mut self,
        key: &PoolKey,
        expiry: Option<Duration>,
    ) -> Option<(Http1Connection, Option<ProxyConfig>)> {
        let queue = self.h1_idle.get_mut(key)?;
        while let Some((conn, proxy_cfg)) = queue.pop_back() {
            self.total_idle_h1 = self.total_idle_h1.saturating_sub(1);
            if conn.is_usable(expiry) {
                return Some((conn, proxy_cfg));
            }
        }
        if queue.is_empty() {
            self.h1_idle.remove(key);
        }
        None
    }

    fn put_h1(
        &mut self,
        key: PoolKey,
        mut conn: Http1Connection,
        proxy_cfg: Option<ProxyConfig>,
        limits: &PoolLimits,
    ) {
        if !conn.is_usable(limits.keepalive_expiry) {
            return;
        }
        if let Some(max_keepalive) = limits.max_keepalive_connections {
            if max_keepalive == 0 {
                return;
            }
            if self.total_idle_h1 >= max_keepalive {
                self.evict_oldest_h1();
            }
        }
        conn.mark_used();
        self.h1_idle
            .entry(key)
            .or_default()
            .push_back((conn, proxy_cfg));
        self.total_idle_h1 += 1;
    }

    fn evict_oldest_h1(&mut self) {
        let mut oldest_key: Option<PoolKey> = None;
        let mut oldest_time: Option<Instant> = None;
        for (k, q) in &self.h1_idle {
            if let Some((front, _)) = q.front() {
                let t = front.created_at();
                if oldest_time.map(|ot| t < ot).unwrap_or(true) {
                    oldest_time = Some(t);
                    oldest_key = Some(k.clone());
                }
            }
        }
        if let Some(k) = oldest_key {
            if let Some(q) = self.h1_idle.get_mut(&k) {
                if q.pop_front().is_some() {
                    self.total_idle_h1 = self.total_idle_h1.saturating_sub(1);
                }
                if q.is_empty() {
                    self.h1_idle.remove(&k);
                }
            }
        }
    }
}

/// Thread-safe asynchronous HTTP/1.1 & HTTP/2 connection pool.
#[derive(Clone)]
pub struct ConnectionPool {
    limits: PoolLimits,
    tls_config: TlsConfig,
    tls_connector: TlsConnector,
    http2_prior_knowledge: bool,
    semaphore: Option<Arc<Semaphore>>,
    inner: Arc<Mutex<PoolInner>>,
}

impl ConnectionPool {
    /// Creates a new [`ConnectionPool`] with the given pool limits and TLS settings.
    pub fn new(
        limits: PoolLimits,
        tls_config: TlsConfig,
        http2_prior_knowledge: bool,
    ) -> Result<Self> {
        let tls_connector = tls_config.build_connector()?;
        let semaphore = limits
            .max_connections
            .map(|n| Arc::new(Semaphore::new(n.max(1))));

        Ok(Self {
            limits,
            tls_config,
            tls_connector,
            http2_prior_knowledge,
            semaphore,
            inner: Arc::new(Mutex::new(PoolInner::new())),
        })
    }

    /// Sends a single [`Request`] using a pooled or newly established connection.
    pub async fn send(
        &self,
        request: &Request,
        proxy: Option<&ProxyConfig>,
        timeout: &TimeoutConfig,
    ) -> Result<Response> {
        let start_time = Instant::now();
        let key = build_pool_key(&request.url, proxy)?;

        let _permit = self.acquire_permit(timeout.pool).await?;

        // 1. Check reusable HTTP/2 multiplexed session or idle HTTP/1.1 connection
        let existing = {
            let mut guard = self.inner.lock();
            if let Some(h2) = guard.take_h2(&key) {
                Some(PooledProtocol::Http2(h2))
            } else if let Some((h1, proxy_abs)) = guard.take_h1(&key, self.limits.keepalive_expiry)
            {
                Some(PooledProtocol::Http1 {
                    conn: h1,
                    proxy_abs_form: proxy_abs,
                })
            } else {
                None
            }
        };

        let protocol = match existing {
            Some(p) => p,
            None => self.connect(&request.url, &key, proxy, timeout).await?,
        };

        match protocol {
            PooledProtocol::Http2(mut h2) => h2.send_request(request, timeout, start_time).await,
            PooledProtocol::Http1 {
                mut conn,
                proxy_abs_form,
            } => {
                let (response, keep_alive) = conn
                    .send_request(request, timeout, proxy_abs_form.as_ref(), start_time)
                    .await?;
                if keep_alive {
                    let mut guard = self.inner.lock();
                    guard.put_h1(key, conn, proxy_abs_form, &self.limits);
                }
                Ok(response)
            }
        }
    }

    /// Drops all idle HTTP/1.1 and active HTTP/2 handles from the pool.
    pub fn close(&self) {
        let mut guard = self.inner.lock();
        guard.h1_idle.clear();
        guard.h2_active.clear();
        guard.total_idle_h1 = 0;
    }

    /// Returns the number of currently idle HTTP/1.1 connections in the pool.
    pub fn idle_http1_count(&self) -> usize {
        self.inner.lock().total_idle_h1
    }

    /// Returns the number of cached HTTP/2 sessions in the pool.
    pub fn active_http2_count(&self) -> usize {
        self.inner.lock().h2_active.len()
    }

    async fn acquire_permit(&self, pool_timeout: Option<Duration>) -> Result<Option<OwnedSemaphorePermit>> {
        let Some(sem) = &self.semaphore else {
            return Ok(None);
        };
        let sem_clone = Arc::clone(sem);
        let permit = with_pool_timeout(pool_timeout, async move {
            sem_clone
                .acquire_owned()
                .await
                .map_err(|_| HttpxError::PoolTimeout("Connection pool closed".into()))
        })
        .await?;
        Ok(Some(permit))
    }

    async fn connect(
        &self,
        url: &Url,
        key: &PoolKey,
        proxy: Option<&ProxyConfig>,
        timeout: &TimeoutConfig,
    ) -> Result<PooledProtocol> {
        let is_https = url.scheme() == "https";
        let target_host = key.host.as_str();
        let target_port = key.port;

        match proxy {
            None => {
                let tcp = connect_tcp_with_dns(target_host, target_port, timeout.connect).await?;
                self.upgrade_and_handshake(tcp, target_host, is_https, key, None, timeout)
                    .await
            }
            Some(proxy_cfg) => match &proxy_cfg.scheme {
                ProxyScheme::Socks5 { remote_dns } => {
                    let socks = connect_socks5(
                        proxy_cfg,
                        target_host,
                        target_port,
                        *remote_dns,
                        timeout.connect,
                    )
                    .await?;
                    self.upgrade_and_handshake(socks, target_host, is_https, key, None, timeout)
                        .await
                }
                ProxyScheme::Http => {
                    let proxy_tcp =
                        connect_tcp_with_dns(&proxy_cfg.host, proxy_cfg.port, timeout.connect)
                            .await?;
                    if is_https {
                        let tunnel = open_http_connect_tunnel(
                            proxy_tcp,
                            target_host,
                            target_port,
                            proxy_cfg,
                            timeout,
                        )
                        .await?;
                        self.upgrade_and_handshake(tunnel, target_host, true, key, None, timeout)
                            .await
                    } else {
                        let conn = Http1Connection::handshake(proxy_tcp).await?;
                        Ok(PooledProtocol::Http1 {
                            conn,
                            proxy_abs_form: Some(proxy_cfg.clone()),
                        })
                    }
                }
                ProxyScheme::Https => {
                    let proxy_tcp =
                        connect_tcp_with_dns(&proxy_cfg.host, proxy_cfg.port, timeout.connect)
                            .await?;
                    let (proxy_tls, _) = with_connect_timeout(timeout.connect, async {
                        TlsConfig::connect(&self.tls_connector, &proxy_cfg.host, proxy_tcp).await
                    })
                    .await
                    .map_err(|e| HttpxError::ProxyError(format!("HTTPS proxy TLS error: {e}")))?;

                    if is_https {
                        let tunnel = open_http_connect_tunnel(
                            proxy_tls,
                            target_host,
                            target_port,
                            proxy_cfg,
                            timeout,
                        )
                        .await?;
                        self.upgrade_and_handshake(tunnel, target_host, true, key, None, timeout)
                            .await
                    } else {
                        let conn = Http1Connection::handshake(proxy_tls).await?;
                        Ok(PooledProtocol::Http1 {
                            conn,
                            proxy_abs_form: Some(proxy_cfg.clone()),
                        })
                    }
                }
            },
        }
    }

    async fn upgrade_and_handshake<S>(
        &self,
        stream: S,
        server_host: &str,
        is_https: bool,
        key: &PoolKey,
        proxy_abs_form: Option<ProxyConfig>,
        timeout: &TimeoutConfig,
    ) -> Result<PooledProtocol>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        if is_https {
            let (tls_stream, alpn) = with_connect_timeout(timeout.connect, async {
                TlsConfig::connect(&self.tls_connector, server_host, stream).await
            })
            .await?;

            let negotiated_h2 = alpn.as_deref() == Some(b"h2");
            if negotiated_h2 && self.tls_config.http2 {
                let h2 = Http2Connection::handshake(tls_stream).await?;
                self.inner.lock().h2_active.insert(key.clone(), h2.clone());
                Ok(PooledProtocol::Http2(h2))
            } else {
                let conn = Http1Connection::handshake(tls_stream).await?;
                Ok(PooledProtocol::Http1 {
                    conn,
                    proxy_abs_form,
                })
            }
        } else if self.http2_prior_knowledge && self.tls_config.http2 {
            let h2 = Http2Connection::handshake(stream).await?;
            self.inner.lock().h2_active.insert(key.clone(), h2.clone());
            Ok(PooledProtocol::Http2(h2))
        } else {
            let conn = Http1Connection::handshake(stream).await?;
            Ok(PooledProtocol::Http1 {
                conn,
                proxy_abs_form,
            })
        }
    }
}

fn build_pool_key(url: &Url, proxy: Option<&ProxyConfig>) -> Result<PoolKey> {
    let host = url
        .host_str()
        .ok_or_else(|| HttpxError::InvalidUrl(format!("URL missing host: {url}")))?
        .to_ascii_lowercase();
    let port = url.port_or_known_default().ok_or_else(|| {
        HttpxError::UnsupportedProtocol(format!("Unknown default port for scheme '{}'", url.scheme()))
    })?;
    Ok(PoolKey {
        scheme: url.scheme().to_ascii_lowercase(),
        host,
        port,
        proxy_url: proxy.map(|p| p.url.as_str().to_string()),
    })
}

/// Resolves DNS (IPv4 and IPv6) and establishes a TCP connection within `connect_timeout`.
async fn connect_tcp_with_dns(
    host: &str,
    port: u16,
    connect_timeout: Option<Duration>,
) -> Result<TcpStream> {
    let clean_host = host.trim_start_matches('[').trim_end_matches(']');
    with_connect_timeout(connect_timeout, async move {
        let addrs: Vec<SocketAddr> = lookup_host((clean_host, port))
            .await
            .map_err(|e| {
                HttpxError::ConnectError(format!("DNS resolution failed for '{host}:{port}': {e}"))
            })?
            .collect();

        if addrs.is_empty() {
            return Err(HttpxError::ConnectError(format!(
                "DNS lookup returned no addresses for '{host}:{port}'"
            )));
        }

        let mut last_err = None;
        for addr in addrs {
            match TcpStream::connect(addr).await {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last_err = Some(e),
            }
        }

        Err(HttpxError::ConnectError(format!(
            "Failed to connect to '{host}:{port}': {}",
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown error".to_string())
        )))
    })
    .await
}

async fn connect_socks5(
    proxy: &ProxyConfig,
    target_host: &str,
    target_port: u16,
    remote_dns: bool,
    connect_timeout: Option<Duration>,
) -> Result<Socks5Stream<TcpStream>> {
    let proxy_host = proxy.host.trim_start_matches('[').trim_end_matches(']');
    let clean_target = target_host.trim_start_matches('[').trim_end_matches(']');

    with_connect_timeout(connect_timeout, async move {
        let target_addr: String = if remote_dns {
            clean_target.to_string()
        } else {
            let resolved = lookup_host((clean_target, target_port))
                .await
                .map_err(|e| {
                    HttpxError::ProxyError(format!(
                        "DNS resolution failed for SOCKS5 target '{clean_target}:{target_port}': {e}"
                    ))
                })?
                .next()
                .ok_or_else(|| {
                    HttpxError::ProxyError(format!(
                        "No DNS results for SOCKS5 target '{clean_target}:{target_port}'"
                    ))
                })?;
            resolved.ip().to_string()
        };

        let res = match &proxy.auth {
            Some(auth) => {
                Socks5Stream::connect_with_password(
                    (proxy_host, proxy.port),
                    (target_addr.as_str(), target_port),
                    &auth.username,
                    &auth.password,
                )
                .await
            }
            None => {
                Socks5Stream::connect((proxy_host, proxy.port), (target_addr.as_str(), target_port))
                    .await
            }
        };

        res.map_err(|e| HttpxError::ProxyError(format!("SOCKS5 connection failed: {e}")))
    })
    .await
}
