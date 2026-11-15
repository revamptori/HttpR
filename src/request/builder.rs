//! HTTP request representation and fluent builder.

use crate::exceptions::{HttpxError, Result};
use crate::request::cookies::CookieJar;
use crate::request::headers::Headers;
use crate::request::multipart::MultipartEncoder;
use bytes::Bytes;
use futures_util::Stream;
use http::Method;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use url::Url;

/// Type-erased asynchronous byte stream for streaming request bodies.
pub type BoxByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;

/// Request payload representation supporting zero-copy buffered bodies and async streams.
#[derive(Clone)]
pub enum RequestBody {
    Empty,
    Bytes(Bytes),
    Stream(Arc<Mutex<Option<BoxByteStream>>>),
}

impl RequestBody {
    /// Returns `true` if the body is empty.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Bytes(b) => b.is_empty(),
            Self::Stream(_) => false,
        }
    }

    /// Returns the known byte length if the body is buffered.
    pub fn content_length(&self) -> Option<usize> {
        match self {
            Self::Empty => Some(0),
            Self::Bytes(b) => Some(b.len()),
            Self::Stream(_) => None,
        }
    }

    /// Returns the buffered bytes if non-streaming.
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Self::Empty => None,
            Self::Bytes(b) => Some(b),
            Self::Stream(_) => None,
        }
    }

    /// Takes the underlying stream if this is a streaming body.
    pub fn take_stream(&self) -> Option<BoxByteStream> {
        match self {
            Self::Stream(lock) => lock.lock().take(),
            _ => None,
        }
    }

    /// Returns `true` if this body can be replayed across redirects or retries.
    pub fn is_replayable(&self) -> bool {
        !matches!(self, Self::Stream(_))
    }
}

impl std::fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "RequestBody::Empty"),
            Self::Bytes(b) => write!(f, "RequestBody::Bytes({} bytes)", b.len()),
            Self::Stream(_) => write!(f, "RequestBody::Stream(...)"),
        }
    }
}

/// Complete HTTP request object passed through middleware, auth, and transport layers.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: Headers,
    pub body: RequestBody,
    pub extensions: HashMap<String, serde_json::Value>,
}

impl Request {
    /// Starts a [`RequestBuilder`] for the given HTTP method and URL string.
    pub fn builder(method: &str, url: &str) -> Result<RequestBuilder> {
        RequestBuilder::new(method, url)
    }

    /// Returns the raw byte slice if buffered, or empty slice if empty/stream.
    pub fn content(&self) -> Bytes {
        match &self.body {
            RequestBody::Empty => Bytes::new(),
            RequestBody::Bytes(b) => b.clone(),
            RequestBody::Stream(_) => Bytes::new(),
        }
    }

    /// Clones this request for redirect or retry handling, stripping sensitive headers on cross-origin
    /// redirects if `strip_auth` is true.
    pub fn clone_for_redirect(&self, new_method: Method, new_url: Url, drop_body: bool) -> Self {
        let mut headers = self.headers.clone();
        let same_origin = self.url.scheme() == new_url.scheme()
            && self.url.host_str() == new_url.host_str()
            && self.url.port_or_known_default() == new_url.port_or_known_default();
        if !same_origin {
            headers.remove("authorization");
            headers.remove("proxy-authorization");
            headers.remove("cookie");
        }
        if drop_body {
            headers.remove("content-length");
            headers.remove("content-type");
            headers.remove("transfer-encoding");
        }
        Self {
            method: new_method,
            url: new_url,
            headers,
            body: if drop_body {
                RequestBody::Empty
            } else {
                self.body.clone()
            },
            extensions: self.extensions.clone(),
        }
    }
}

/// Fluent builder for constructing [`Request`] instances.
pub struct RequestBuilder {
    method: Method,
    url: Url,
    headers: Headers,
    body: RequestBody,
    params: Vec<(String, String)>,
    extensions: HashMap<String, serde_json::Value>,
}

impl RequestBuilder {
    /// Creates a [`RequestBuilder`] with optional `base_url` resolution.
    pub fn with_base_url(method: &str, url: &str, base_url: Option<&Url>) -> Result<Self> {
        let parsed_method = Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes())
            .map_err(|e| HttpxError::LocalProtocolError(format!("Invalid HTTP method '{method}': {e}")))?;

        let parsed_url = match base_url {
            Some(base) => base
                .join(url)
                .map_err(|e| HttpxError::InvalidUrl(format!("Failed to resolve '{url}' against '{base}': {e}")))?,
            None => Url::parse(url)
                .map_err(|e| HttpxError::InvalidUrl(format!("Invalid URL '{url}': {e}")))?,
        };

        validate_scheme(&parsed_url)?;

        Ok(Self {
            method: parsed_method,
            url: parsed_url,
            headers: Headers::new(),
            body: RequestBody::Empty,
            params: Vec::new(),
            extensions: HashMap::new(),
        })
    }

    /// Creates a [`RequestBuilder`] from an absolute URL.
    pub fn new(method: &str, url: &str) -> Result<Self> {
        Self::with_base_url(method, url, None)
    }

    /// Appends query parameters to the request URL.
    pub fn params(mut self, params: Vec<(String, String)>) -> Self {
        self.params.extend(params);
        self
    }

    /// Adds a single query parameter.
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }

    /// Sets or merges headers onto the request.
    pub fn headers(mut self, headers: Headers) -> Self {
        self.headers.update(&headers);
        self
    }

    /// Inserts a single header key/value.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self> {
        self.headers.insert(name, value)?;
        Ok(self)
    }

    /// Sets raw byte content as the request body.
    pub fn content(mut self, bytes: impl Into<Bytes>) -> Self {
        self.body = RequestBody::Bytes(bytes.into());
        self
    }

    /// Serializes `value` as JSON and sets `Content-Type: application/json` if absent.
    pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Self> {
        let vec = serde_json::to_vec(value)
            .map_err(|e| HttpxError::RequestError(format!("Failed to serialize JSON body: {e}")))?;
        self.body = RequestBody::Bytes(Bytes::from(vec));
        self.headers
            .insert_if_absent("content-type", "application/json")?;
        Ok(self)
    }

    /// Encodes form fields as `application/x-www-form-urlencoded`.
    pub fn form(mut self, fields: &[(String, String)]) -> Result<Self> {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in fields {
            serializer.append_pair(k, v);
        }
        let encoded = serializer.finish();
        self.body = RequestBody::Bytes(Bytes::from(encoded.into_bytes()));
        self.headers
            .insert_if_absent("content-type", "application/x-www-form-urlencoded")?;
        Ok(self)
    }

    /// Encodes multipart form fields and files and sets the multipart `Content-Type` header.
    pub fn multipart(mut self, encoder: &MultipartEncoder) -> Result<Self> {
        let ct = encoder.content_type();
        self.body = RequestBody::Bytes(encoder.encode());
        self.headers.insert("content-type", &ct)?;
        Ok(self)
    }

    /// Sets a streaming request body.
    pub fn stream(mut self, stream: BoxByteStream) -> Self {
        self.body = RequestBody::Stream(Arc::new(Mutex::new(Some(stream))));
        self
    }

    /// Attaches extensions metadata (such as per-request timeout overrides).
    pub fn extensions(mut self, ext: HashMap<String, serde_json::Value>) -> Self {
        self.extensions.extend(ext);
        self
    }

    /// Applies cookie jar and optional per-request cookies.
    pub fn apply_cookies(
        mut self,
        jar: &CookieJar,
        extra_cookies: Option<&HashMap<String, String>>,
    ) -> Result<Self> {
        if !self.headers.contains_key("cookie") {
            if let Some(cookie_hdr) = jar.cookie_header_for_url(&self.url, extra_cookies) {
                self.headers.insert("cookie", &cookie_hdr)?;
            }
        }
        Ok(self)
    }

    /// Finalizes the [`Request`], applying URL query parameters and default headers.
    pub fn build(mut self) -> Result<Request> {
        if !self.params.is_empty() {
            let mut existing: Vec<(String, String)> = self
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            existing.extend(self.params);
            let mut qp = self.url.query_pairs_mut();
            qp.clear();
            for (k, v) in existing {
                qp.append_pair(&k, &v);
            }
        }

        // Standard default headers matching HTTPX conventions
        self.headers
            .insert_if_absent("user-agent", concat!("HttpR/", env!("CARGO_PKG_VERSION")))?;
        self.headers.insert_if_absent("accept", "*/*")?;
        self.headers
            .insert_if_absent("accept-encoding", "gzip, deflate")?;

        if let Some(len) = self.body.content_length() {
            if len > 0 || matches!(self.method, Method::POST | Method::PUT | Method::PATCH) {
                self.headers
                    .insert_if_absent("content-length", &len.to_string())?;
            }
        }

        Ok(Request {
            method: self.method,
            url: self.url,
            headers: self.headers,
            body: self.body,
            extensions: self.extensions,
        })
    }
}

fn validate_scheme(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(HttpxError::UnsupportedProtocol(format!(
            "Request URL has an unsupported protocol '{other}://'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_builder_params_and_json() {
        let req = Request::builder("post", "https://example.com/items?a=1")
            .unwrap()
            .param("b", "2")
            .json(&serde_json::json!({"hello": "world"}))
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(req.method, Method::POST);
        assert_eq!(req.url.as_str(), "https://example.com/items?a=1&b=2");
        assert_eq!(
            req.headers.get("content-type").as_deref(),
            Some("application/json")
        );
    }
}
