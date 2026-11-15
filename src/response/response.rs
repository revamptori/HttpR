//! HTTP response representation with lazy text decoding and zero-copy byte buffering.

use crate::exceptions::{HttpxError, Result};
use crate::request::cookies::CookieJar;
use crate::request::headers::Headers;
use crate::request::Request;
use crate::response::streaming::{decompress_body, ResponseStream};
use bytes::Bytes;
use encoding_rs::Encoding;
use http::{StatusCode, Version};
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// HTTP response object matching the HTTPX `Response` API.
#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: u16,
    pub reason_phrase: String,
    pub headers: Headers,
    pub cookies: CookieJar,
    pub url: Url,
    pub http_version: String,
    pub elapsed: Duration,
    pub request: Option<Request>,
    pub history: Vec<Response>,

    encoding_override: Arc<RwLock<Option<String>>>,
    content_cache: Arc<RwLock<Option<Bytes>>>,
    text_cache: Arc<RwLock<Option<String>>>,
    stream: Option<ResponseStream>,
}

impl Response {
    /// Constructs a [`Response`] from an already-buffered body.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        status: StatusCode,
        headers: Headers,
        cookies: CookieJar,
        url: Url,
        version: Version,
        elapsed: Duration,
        content: Bytes,
        request: Option<Request>,
        history: Vec<Response>,
    ) -> Self {
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        Self {
            status_code: status.as_u16(),
            reason_phrase,
            headers,
            cookies,
            url,
            http_version: format_version(version),
            elapsed,
            request,
            history,
            encoding_override: Arc::new(RwLock::new(None)),
            content_cache: Arc::new(RwLock::new(Some(content))),
            text_cache: Arc::new(RwLock::new(None)),
            stream: None,
        }
    }

    /// Constructs a streaming [`Response`] whose body has not yet been read into memory.
    #[allow(clippy::too_many_arguments)]
    pub fn from_stream(
        status: StatusCode,
        headers: Headers,
        cookies: CookieJar,
        url: Url,
        version: Version,
        elapsed: Duration,
        stream: ResponseStream,
        request: Option<Request>,
        history: Vec<Response>,
    ) -> Self {
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        Self {
            status_code: status.as_u16(),
            reason_phrase,
            headers,
            cookies,
            url,
            http_version: format_version(version),
            elapsed,
            request,
            history,
            encoding_override: Arc::new(RwLock::new(None)),
            content_cache: Arc::new(RwLock::new(None)),
            text_cache: Arc::new(RwLock::new(None)),
            stream: Some(stream),
        }
    }

    /// Reads and caches the full response body from the underlying stream, applying decompression.
    pub async fn read(&self) -> Result<Bytes> {
        if let Some(cached) = self.content_cache.read().clone() {
            return Ok(cached);
        }

        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| HttpxError::StreamError("Response stream is unavailable".into()))?;
        let raw = stream.read_all().await?;
        let content_encoding = self.headers.get("content-encoding");
        let decompressed = decompress_body(raw, content_encoding.as_deref())?;
        *self.content_cache.write() = Some(decompressed.clone());
        Ok(decompressed)
    }

    /// Reads the next raw chunk from the streaming response.
    pub async fn next_chunk(&self) -> Result<Option<Bytes>> {
        if let Some(stream) = &self.stream {
            stream.next_chunk().await
        } else {
            Ok(None)
        }
    }

    /// Closes the underlying response stream.
    pub async fn close(&self) {
        if let Some(stream) = &self.stream {
            stream.close().await;
        }
    }

    /// Returns total bytes downloaded so far.
    pub fn num_bytes_downloaded(&self) -> usize {
        if let Some(stream) = &self.stream {
            let n = stream.num_bytes_downloaded();
            if n > 0 {
                return n;
            }
        }
        self.content_cache
            .read()
            .as_ref()
            .map(|b| b.len())
            .unwrap_or(0)
    }

    /// Returns `true` if the body has been read into memory.
    pub fn is_stream_consumed(&self) -> bool {
        self.content_cache.read().is_some()
    }

    /// Returns `true` if the underlying stream is closed.
    pub fn is_closed(&self) -> bool {
        self.stream.as_ref().map(|s| s.is_closed()).unwrap_or(true)
    }

    /// Returns the buffered raw response body [`Bytes`], or an error if the stream wasn't read yet.
    pub fn content(&self) -> Result<Bytes> {
        self.content_cache.read().clone().ok_or_else(|| {
            HttpxError::StreamError(
                "Attempted to access streaming response content before calling read()".into(),
            )
        })
    }

    /// Returns the active character encoding (explicit override, `Content-Type` charset, or `"utf-8"`).
    pub fn encoding(&self) -> String {
        if let Some(enc) = self.encoding_override.read().clone() {
            return enc;
        }
        if let Some(ct_charset) = self.headers.content_type_charset() {
            return ct_charset;
        }
        "utf-8".to_string()
    }

    /// Overrides the character encoding and invalidates any cached decoded text.
    pub fn set_encoding(&self, encoding: impl Into<String>) {
        *self.encoding_override.write() = Some(encoding.into());
        *self.text_cache.write() = None;
    }

    /// Lazily decodes and caches the response body as text using the detected or overridden encoding.
    pub fn text(&self) -> Result<String> {
        if let Some(cached) = self.text_cache.read().clone() {
            return Ok(cached);
        }

        let bytes = self.content()?;
        let enc_label = self.encoding();
        let encoding = Encoding::for_label(enc_label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
        let (decoded, _, _) = encoding.decode(&bytes);
        let text = decoded.into_owned();
        *self.text_cache.write() = Some(text.clone());
        Ok(text)
    }

    /// Parses the response body as JSON into [`serde_json::Value`] or any deserializable type `T`.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        let bytes = self.content()?;
        serde_json::from_slice(&bytes)
            .map_err(|e| HttpxError::DecodingError(format!("Failed to decode JSON response: {e}")))
    }

    /// Returns `true` for 1xx status codes.
    pub fn is_informational(&self) -> bool {
        (100..200).contains(&self.status_code)
    }

    /// Returns `true` for 2xx status codes.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }

    /// Returns `true` for 3xx status codes.
    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status_code)
    }

    /// Returns `true` for 4xx status codes.
    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.status_code)
    }

    /// Returns `true` for 5xx status codes.
    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.status_code)
    }

    /// Returns `true` for 4xx or 5xx status codes.
    pub fn is_error(&self) -> bool {
        self.is_client_error() || self.is_server_error()
    }

    /// Returns `Ok(())` for non-error responses, or `Err(HttpxError::HTTPStatusError)` for 4xx/5xx.
    pub fn raise_for_status(&self) -> Result<()> {
        if self.is_error() {
            return Err(HttpxError::HTTPStatusError {
                status_code: self.status_code,
                reason: self.reason_phrase.clone(),
                url: self.url.to_string(),
            });
        }
        Ok(())
    }

    /// Returns `true` if the response has a `Location` header and a redirect status code.
    pub fn has_redirect_location(&self) -> bool {
        self.headers.contains_key("location")
            && matches!(self.status_code, 301 | 302 | 303 | 307 | 308)
    }
}

fn format_version(version: Version) -> String {
    match version {
        Version::HTTP_09 => "HTTP/0.9".to_string(),
        Version::HTTP_10 => "HTTP/1.0".to_string(),
        Version::HTTP_11 => "HTTP/1.1".to_string(),
        Version::HTTP_2 => "HTTP/2".to_string(),
        Version::HTTP_3 => "HTTP/3".to_string(),
        _ => "HTTP/1.1".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_lazy_text_and_status_helpers() {
        let mut headers = Headers::new();
        headers
            .insert("content-type", "application/json; charset=utf-8")
            .unwrap();
        let resp = Response::new(
            StatusCode::OK,
            headers,
            CookieJar::new(),
            Url::parse("https://example.com/api").unwrap(),
            Version::HTTP_2,
            Duration::from_millis(12),
            Bytes::from_static(br#"{"ok":true}"#),
            None,
            vec![],
        );

        assert!(resp.is_success());
        assert!(!resp.is_error());
        assert!(resp.raise_for_status().is_ok());
        assert_eq!(resp.http_version, "HTTP/2");
        assert_eq!(resp.text().unwrap(), r#"{"ok":true}"#);
        let parsed: serde_json::Value = resp.json().unwrap();
        assert_eq!(parsed["ok"], serde_json::json!(true));
    }

    #[test]
    fn test_raise_for_status_error() {
        let resp = Response::new(
            StatusCode::NOT_FOUND,
            Headers::new(),
            CookieJar::new(),
            Url::parse("https://example.com/missing").unwrap(),
            Version::HTTP_11,
            Duration::from_millis(5),
            Bytes::new(),
            None,
            vec![],
        );
        assert!(resp.is_client_error());
        assert!(resp.is_error());
        assert!(matches!(
            resp.raise_for_status(),
            Err(HttpxError::HTTPStatusError {
                status_code: 404,
                ..
            })
        ));
    }
}
