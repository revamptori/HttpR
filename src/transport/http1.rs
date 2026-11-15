//! HTTP/1.1 connection management, request serialization, and proxy CONNECT tunneling.

use crate::exceptions::{HttpxError, Result};
use crate::middleware::timeout::{with_read_timeout, with_write_timeout, TimeoutConfig};
use crate::proxy::ProxyConfig;
use crate::request::{Headers, Request, RequestBody};
use crate::response::streaming::ResponseStream;
use crate::response::Response;
use bytes::Bytes;
use futures_util::TryStreamExt;
use http::{HeaderMap, HeaderValue, Method, Request as HttpRequest, Uri, Version};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;

/// Type-erased HTTP request body passed to Hyper.
pub type HyperReqBody = BoxBody<Bytes, HttpxError>;

/// Wraps an established HTTP/1.1 client connection handle.
pub struct Http1Connection {
    sender: http1::SendRequest<HyperReqBody>,
    created_at: Instant,
    last_used: Instant,
}

impl Http1Connection {
    /// Performs the HTTP/1.1 handshake on `io` and spawns the connection driver task.
    pub async fn handshake<S>(io: S) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (sender, conn) = http1::Builder::new()
            .preserve_header_case(true)
            .handshake::<_, HyperReqBody>(TokioIo::new(io))
            .await
            .map_err(|e| {
                HttpxError::ConnectError(format!("HTTP/1.1 handshake failed: {e}"))
            })?;

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let now = Instant::now();
        Ok(Self {
            sender,
            created_at: now,
            last_used: now,
        })
    }

    /// Returns `true` if the underlying connection is still open and ready for a new request.
    pub fn is_usable(&self, keepalive_expiry: Option<Duration>) -> bool {
        if self.sender.is_closed() {
            return false;
        }
        if let Some(expiry) = keepalive_expiry {
            if self.last_used.elapsed() > expiry {
                return false;
            }
        }
        true
    }

    /// Updates the last-used timestamp when returning the connection to the pool.
    pub fn mark_used(&mut self) {
        self.last_used = Instant::now();
    }

    /// Returns creation timestamp of this connection.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Executes an HTTP/1.1 request over this connection.
    pub async fn send_request(
        &mut self,
        request: &Request,
        timeout: &TimeoutConfig,
        proxy_http_absolute_form: Option<&ProxyConfig>,
        start_time: Instant,
    ) -> Result<(Response, bool)> {
        self.sender.ready().await.map_err(|e| {
            HttpxError::ConnectError(format!("HTTP/1.1 connection not ready: {e}"))
        })?;

        let hyper_req = build_hyper_request(request, false, proxy_http_absolute_form)?;

        let send_fut = self.sender.send_request(hyper_req);
        let hyper_resp = with_write_timeout(timeout.write, async {
            with_read_timeout(timeout.read, async {
                send_fut.await.map_err(map_hyper_error)
            })
            .await
        })
        .await?;

        let keep_alive = !hyper_resp
            .headers()
            .get(http::header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_ascii_lowercase().contains("close"))
            .unwrap_or(false);

        let response = convert_hyper_response(hyper_resp, request, timeout.read, start_time);
        self.last_used = Instant::now();
        Ok((response, keep_alive))
    }
}

/// Opens an HTTP `CONNECT host:port` tunnel through an HTTP/HTTPS proxy stream.
pub async fn open_http_connect_tunnel<S>(
    io: S,
    target_host: &str,
    target_port: u16,
    proxy: &ProxyConfig,
    timeout: &TimeoutConfig,
) -> Result<TokioIo<hyper::upgrade::Upgraded>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let authority = format_authority(target_host, target_port);
    let uri: Uri = authority.parse().map_err(|e| {
        HttpxError::ProxyError(format!("Invalid CONNECT authority '{authority}': {e}"))
    })?;

    let (mut sender, conn) = http1::Builder::new()
        .handshake::<_, HyperReqBody>(TokioIo::new(io))
        .await
        .map_err(|e| HttpxError::ProxyError(format!("Proxy handshake failed: {e}")))?;

    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    let mut req_builder = HttpRequest::builder()
        .method(Method::CONNECT)
        .uri(uri)
        .version(Version::HTTP_11)
        .header(http::header::HOST, &authority);

    if let Some(auth) = &proxy.auth {
        req_builder = req_builder.header(
            http::header::PROXY_AUTHORIZATION,
            auth.basic_header_value(),
        );
    }

    let req = req_builder
        .body(Empty::<Bytes>::new().map_err(|never| match never {}).boxed())
        .map_err(|e| HttpxError::ProxyError(format!("Failed to build CONNECT request: {e}")))?;

    let resp = with_read_timeout(timeout.connect, async {
        sender
            .send_request(req)
            .await
            .map_err(|e| HttpxError::ProxyError(format!("Proxy CONNECT failed: {e}")))
    })
    .await?;

    if !resp.status().is_success() {
        return Err(HttpxError::ProxyError(format!(
            "Proxy CONNECT rejected with status {} {}",
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("")
        )));
    }

    let upgraded = hyper::upgrade::on(resp)
        .await
        .map_err(|e| HttpxError::ProxyError(format!("Proxy tunnel upgrade failed: {e}")))?;

    Ok(TokioIo::new(upgraded))
}

/// Converts an internal [`Request`] into a [`http::Request<HyperReqBody>`].
pub fn build_hyper_request(
    request: &Request,
    is_http2: bool,
    proxy_http_absolute_form: Option<&ProxyConfig>,
) -> Result<HttpRequest<HyperReqBody>> {
    let uri_str = if !is_http2 && proxy_http_absolute_form.is_some() {
        request.url.as_str().to_string()
    } else {
        match request.url.query() {
            Some(q) => format!("{}?{q}", request.url.path()),
            None => request.url.path().to_string(),
        }
    };

    let uri: Uri = uri_str
        .parse()
        .map_err(|e| HttpxError::InvalidUrl(format!("Invalid request URI '{uri_str}': {e}")))?;

    let mut builder = HttpRequest::builder()
        .method(request.method.clone())
        .uri(uri)
        .version(if is_http2 {
            Version::HTTP_2
        } else {
            Version::HTTP_11
        });

    let mut headers_map: HeaderMap = request.headers.as_header_map().clone();

    if !is_http2 {
        if !headers_map.contains_key(http::header::HOST) {
            let host_header = host_header_value(&request.url);
            let hv = HeaderValue::from_str(&host_header).map_err(|e| {
                HttpxError::InvalidUrl(format!("Invalid Host header '{host_header}': {e}"))
            })?;
            headers_map.insert(http::header::HOST, hv);
        }
        if let Some(proxy) = proxy_http_absolute_form {
            if let Some(auth) = &proxy.auth {
                if !headers_map.contains_key(http::header::PROXY_AUTHORIZATION) {
                    if let Ok(hv) = HeaderValue::from_str(&auth.basic_header_value()) {
                        headers_map.insert(http::header::PROXY_AUTHORIZATION, hv);
                    }
                }
            }
        }
    } else {
        // HTTP/2 forbids connection-specific headers (RFC 9113 Section 8.2.2)
        headers_map.remove(http::header::CONNECTION);
        headers_map.remove("keep-alive");
        headers_map.remove("proxy-connection");
        headers_map.remove(http::header::TRANSFER_ENCODING);
        headers_map.remove(http::header::UPGRADE);
    }

    if let Some(req_headers) = builder.headers_mut() {
        *req_headers = headers_map;
    }

    let body = to_hyper_body(&request.body)?;
    builder
        .body(body)
        .map_err(|e| HttpxError::LocalProtocolError(format!("Failed to build HTTP request: {e}")))
}

fn to_hyper_body(body: &RequestBody) -> Result<HyperReqBody> {
    match body {
        RequestBody::Empty => Ok(Empty::<Bytes>::new().map_err(|never| match never {}).boxed()),
        RequestBody::Bytes(bytes) => {
            Ok(Full::new(bytes.clone()).map_err(|never| match never {}).boxed())
        }
        RequestBody::Stream(_) => {
            let stream = body.take_stream().ok_or_else(|| {
                HttpxError::StreamError(
                    "Streaming request body has already been consumed".into(),
                )
            })?;
            let frame_stream = stream.map_ok(Frame::data);
            Ok(StreamBody::new(frame_stream).boxed())
        }
    }
}

/// Converts a Hyper [`http::Response<Incoming>`] into our streaming [`Response`].
pub fn convert_hyper_response(
    hyper_resp: http::Response<Incoming>,
    request: &Request,
    read_timeout: Option<Duration>,
    start_time: Instant,
) -> Response {
    let status = hyper_resp.status();
    let version = hyper_resp.version();
    let headers = Headers::from_header_map(hyper_resp.headers().clone());
    let cookies = crate::request::CookieJar::new();
    for set_cookie in headers.get_list("set-cookie", false) {
        cookies.extract_set_cookie(&set_cookie, &request.url);
    }
    let elapsed = start_time.elapsed();
    let stream = ResponseStream::from_hyper_incoming(hyper_resp.into_body(), read_timeout);

    Response::from_stream(
        status,
        headers,
        cookies,
        request.url.clone(),
        version,
        elapsed,
        stream,
        Some(request.clone()),
        Vec::new(),
    )
}

/// Maps a [`hyper::Error`] to an HTTPX-compatible [`HttpxError`].
pub fn map_hyper_error(err: hyper::Error) -> HttpxError {
    if err.is_timeout() {
        HttpxError::ReadTimeout(format!("HTTP timeout: {err}"))
    } else if err.is_connect() {
        HttpxError::ConnectError(format!("HTTP connection error: {err}"))
    } else if err.is_parse() || err.is_parse_status() || err.is_parse_too_large() {
        HttpxError::RemoteProtocolError(format!("Malformed HTTP response: {err}"))
    } else if err.is_incomplete_message() || err.is_closed() {
        HttpxError::ReadError(format!("Connection closed or incomplete message: {err}"))
    } else {
        HttpxError::TransportError(format!("HTTP transport error: {err}"))
    }
}

fn host_header_value(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

fn format_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
