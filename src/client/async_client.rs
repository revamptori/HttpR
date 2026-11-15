//! Asynchronous HTTP client implementation powered by Tokio, Hyper, and Rustls.

use crate::client::builder::ClientBuilder;
use crate::client::config::{ClientConfig, SendOptions};
use crate::exceptions::{HttpxError, Result};
use crate::middleware::{next_redirect_request, BoxHttpService, FnService, TimeoutConfig};
use crate::proxy::ProxyMounts;
use crate::request::{CookieJar, Headers, Request, RequestBuilder};
use crate::response::Response;
use crate::transport::HttpTransport;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

/// High-performance asynchronous HTTP client matching HTTPX's `AsyncClient` semantics.
#[derive(Clone)]
pub struct AsyncClient {
    config: ClientConfig,
    transport: HttpTransport,
    closed: Arc<AtomicBool>,
}

impl AsyncClient {
    /// Creates an [`AsyncClient`] with default configuration.
    pub fn new() -> Result<Self> {
        Self::from_config(ClientConfig::default())
    }

    /// Returns a [`ClientBuilder`] for customizing client options.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Instantiates an [`AsyncClient`] from an explicit [`ClientConfig`].
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        let proxies = ProxyMounts::merge_with_env(config.proxies.clone(), config.trust_env)?;
        let transport = HttpTransport::new(
            config.limits,
            config.tls.clone(),
            proxies,
            config.retries,
            config.http2_prior_knowledge,
        )?;
        Ok(Self {
            config,
            transport,
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Returns a reference to the client's shared [`CookieJar`].
    pub fn cookies(&self) -> &CookieJar {
        &self.config.cookies
    }

    /// Returns a reference to the client's default [`Headers`].
    pub fn headers(&self) -> &Headers {
        &self.config.headers
    }

    /// Returns `true` if [`AsyncClient::aclose`] has been called.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Closes all pooled connections and marks the client as closed.
    pub async fn aclose(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.transport.close();
    }

    /// Returns a [`RequestBuilder`] pre-populated with the client's `base_url`,
    /// default `headers`, default `params`, and `cookies`.
    pub fn build_request(
        &self,
        method: &str,
        url: &str,
        extra_cookies: Option<&HashMap<String, String>>,
    ) -> Result<RequestBuilder> {
        let mut builder =
            RequestBuilder::with_base_url(method, url, self.config.base_url.as_ref())?;
        if !self.config.params.is_empty() {
            builder = builder.params(self.config.params.clone());
        }
        if !self.config.headers.is_empty() {
            builder = builder.headers(self.config.headers.clone());
        }
        builder = builder.apply_cookies(&self.config.cookies, extra_cookies)?;
        Ok(builder)
    }

    /// Executes a pre-built [`Request`] with optional per-request [`SendOptions`].
    pub async fn send(&self, mut request: Request, options: SendOptions) -> Result<Response> {
        if self.is_closed() {
            return Err(HttpxError::StreamError(
                "Cannot send request after client has been closed".into(),
            ));
        }

        let timeout = options.timeout.unwrap_or(self.config.timeout);
        let follow_redirects = options
            .follow_redirects
            .unwrap_or(self.config.redirects.follow_redirects);
        let max_redirects = self.config.redirects.max_redirects;
        let auth = options.auth.or_else(|| self.config.auth.clone());

        if let Some(ref auth_method) = auth {
            auth_method.prepare_request(&mut request)?;
        }

        let mut history: Vec<Response> = Vec::new();
        let mut redirects_followed = 0usize;
        let mut auth_challenged = false;

        loop {
            let mut response = self.transport.send(&request, &timeout).await?;

            // Persist any Set-Cookie headers into the client cookie jar
            for sc in response.headers.get_list("set-cookie", false) {
                self.config.cookies.extract_set_cookie(&sc, &response.url);
            }

            // Handle 401 authentication challenge (e.g. HTTP Digest Auth)
            if !auth_challenged {
                if let Some(ref auth_method) = auth {
                    if request.body.is_replayable() {
                        let mut retry_req = request.clone();
                        if auth_method.handle_response(&response, &mut retry_req)? {
                            auth_challenged = true;
                            let _ = response.read().await;
                            response.close().await;
                            request = retry_req;
                            continue;
                        }
                    }
                }
            }

            // Handle HTTP redirects
            if follow_redirects {
                if let Some(mut next_req) = next_redirect_request(
                    &request,
                    &response,
                    redirects_followed,
                    max_redirects,
                )? {
                    redirects_followed += 1;
                    // Drain redirect body so the connection returns cleanly to the pool
                    let _ = response.read().await;
                    response.close().await;
                    history.push(response);

                    // Refresh cookies for the redirect target URL
                    next_req.headers.remove("cookie");
                    if let Some(cookie_hdr) =
                        self.config.cookies.cookie_header_for_url(&next_req.url, None)
                    {
                        next_req.headers.insert("cookie", &cookie_hdr)?;
                    }
                    if let Some(ref auth_method) = auth {
                        auth_method.prepare_request(&mut next_req)?;
                    }
                    auth_challenged = false;
                    request = next_req;
                    continue;
                }
            }

            response.history = history;

            if !options.stream {
                response.read().await?;
            }

            return Ok(response);
        }
    }

    /// Convenience helper building and sending a request for `method` and `url`.
    pub async fn request(&self, method: &str, url: &str) -> Result<Response> {
        let req = self.build_request(method, url, None)?.build()?;
        self.send(req, SendOptions::default()).await
    }

    /// Sends an HTTP `GET` request.
    pub async fn get(&self, url: &str) -> Result<Response> {
        self.request("GET", url).await
    }

    /// Sends an HTTP `POST` request.
    pub async fn post(&self, url: &str) -> Result<Response> {
        self.request("POST", url).await
    }

    /// Sends an HTTP `PUT` request.
    pub async fn put(&self, url: &str) -> Result<Response> {
        self.request("PUT", url).await
    }

    /// Sends an HTTP `PATCH` request.
    pub async fn patch(&self, url: &str) -> Result<Response> {
        self.request("PATCH", url).await
    }

    /// Sends an HTTP `DELETE` request.
    pub async fn delete(&self, url: &str) -> Result<Response> {
        self.request("DELETE", url).await
    }

    /// Sends an HTTP `HEAD` request.
    pub async fn head(&self, url: &str) -> Result<Response> {
        self.request("HEAD", url).await
    }

    /// Sends an HTTP `OPTIONS` request.
    pub async fn options(&self, url: &str) -> Result<Response> {
        self.request("OPTIONS", url).await
    }

    /// Sends a request and returns the unbuffered streaming [`Response`].
    pub async fn stream(&self, request: Request, timeout: Option<TimeoutConfig>) -> Result<Response> {
        self.send(
            request,
            SendOptions {
                stream: true,
                timeout,
                ..Default::default()
            },
        )
        .await
    }

    /// Exposes this client as a composable [`tower::Service`].
    pub fn as_tower_service(&self) -> BoxHttpService {
        let client = self.clone();
        FnService::new(move |req: Request| {
            let c = client.clone();
            async move { c.send(req, SendOptions::default()).await }
        })
        .boxed_clone()
    }

    /// Returns a reference to the underlying [`HttpTransport`].
    pub fn transport(&self) -> &HttpTransport {
        &self.transport
    }
}
