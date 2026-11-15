//! Asynchronous HTTP response body streaming and decompression utilities.

use crate::exceptions::{HttpxError, Result};
use bytes::{BufMut, Bytes, BytesMut};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use futures_util::Stream;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use std::io::Read;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Type-erased async stream yielding response body byte chunks.
pub type BoxIncomingStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;

/// Streaming response body handle supporting incremental reads and full buffering.
#[derive(Clone)]
pub struct ResponseStream {
    inner: Arc<Mutex<Option<BoxIncomingStream>>>,
    read_timeout: Option<Duration>,
    bytes_downloaded: Arc<AtomicUsize>,
    is_closed: Arc<AtomicBool>,
}

impl ResponseStream {
    /// Wraps a Hyper [`Incoming`] body into a [`ResponseStream`].
    pub fn from_hyper_incoming(incoming: Incoming, read_timeout: Option<Duration>) -> Self {
        let stream = futures_util::stream::unfold(incoming, |mut body| async move {
            loop {
                match body.frame().await {
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data() {
                            if !data.is_empty() {
                                return Some((Ok(data), body));
                            }
                        }
                    }
                    Some(Err(err)) => {
                        return Some((
                            Err(HttpxError::ReadError(format!(
                                "Error reading response stream: {err}"
                            ))),
                            body,
                        ));
                    }
                    None => return None,
                }
            }
        });

        Self {
            inner: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            read_timeout,
            bytes_downloaded: Arc::new(AtomicUsize::new(0)),
            is_closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a [`ResponseStream`] from an already buffered [`Bytes`] payload.
    pub fn from_bytes(bytes: Bytes) -> Self {
        let stream = if bytes.is_empty() {
            futures_util::stream::iter(Vec::new())
        } else {
            futures_util::stream::iter(vec![Ok(bytes)])
        };
        Self {
            inner: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            read_timeout: None,
            bytes_downloaded: Arc::new(AtomicUsize::new(0)),
            is_closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns total raw bytes read from the stream so far.
    pub fn num_bytes_downloaded(&self) -> usize {
        self.bytes_downloaded.load(Ordering::Relaxed)
    }

    /// Returns `true` if the stream has been closed or fully consumed.
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Relaxed)
    }

    /// Reads the next raw chunk from the stream, enforcing `read_timeout` if configured.
    pub async fn next_chunk(&self) -> Result<Option<Bytes>> {
        if self.is_closed() {
            return Ok(None);
        }
        let mut guard = self.inner.lock().await;
        let stream = match guard.as_mut() {
            Some(s) => s,
            None => {
                self.is_closed.store(true, Ordering::Relaxed);
                return Ok(None);
            }
        };

        use futures_util::StreamExt;
        let next_fut = stream.next();
        let item = match self.read_timeout {
            Some(dur) => match tokio::time::timeout(dur, next_fut).await {
                Ok(res) => res,
                Err(_) => {
                    self.is_closed.store(true, Ordering::Relaxed);
                    *guard = None;
                    return Err(HttpxError::ReadTimeout(format!(
                        "Timed out reading response chunk after {dur:?}"
                    )));
                }
            },
            None => next_fut.await,
        };

        match item {
            Some(Ok(chunk)) => {
                self.bytes_downloaded
                    .fetch_add(chunk.len(), Ordering::Relaxed);
                Ok(Some(chunk))
            }
            Some(Err(err)) => {
                self.is_closed.store(true, Ordering::Relaxed);
                *guard = None;
                Err(err)
            }
            None => {
                self.is_closed.store(true, Ordering::Relaxed);
                *guard = None;
                Ok(None)
            }
        }
    }

    /// Collects the entire remaining stream into a single [`Bytes`] buffer.
    pub async fn read_all(&self) -> Result<Bytes> {
        let mut buf = BytesMut::new();
        while let Some(chunk) = self.next_chunk().await? {
            buf.put(chunk);
        }
        Ok(buf.freeze())
    }

    /// Closes and drops the underlying stream without consuming remaining data.
    pub async fn close(&self) {
        self.is_closed.store(true, Ordering::Relaxed);
        let mut guard = self.inner.lock().await;
        *guard = None;
    }
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseStream")
            .field("bytes_downloaded", &self.num_bytes_downloaded())
            .field("is_closed", &self.is_closed())
            .finish()
    }
}

/// Decompresses `raw` bytes according to the `Content-Encoding` header value.
pub fn decompress_body(raw: Bytes, content_encoding: Option<&str>) -> Result<Bytes> {
    let enc = match content_encoding {
        Some(v) => v.trim().to_ascii_lowercase(),
        None => return Ok(raw),
    };
    if raw.is_empty() || enc.is_empty() || enc == "identity" {
        return Ok(raw);
    }

    match enc.as_str() {
        "gzip" | "x-gzip" => {
            let mut decoder = GzDecoder::new(&raw[..]);
            let mut out = Vec::with_capacity(raw.len().saturating_mul(2));
            decoder.read_to_end(&mut out).map_err(|e| {
                HttpxError::DecodingError(format!("Failed to decode gzip response body: {e}"))
            })?;
            Ok(Bytes::from(out))
        }
        "deflate" => {
            // Try zlib-wrapped deflate first (RFC 1950), fallback to raw deflate (RFC 1951).
            let mut zlib = ZlibDecoder::new(&raw[..]);
            let mut out = Vec::with_capacity(raw.len().saturating_mul(2));
            if zlib.read_to_end(&mut out).is_ok() {
                return Ok(Bytes::from(out));
            }
            out.clear();
            let mut raw_deflate = DeflateDecoder::new(&raw[..]);
            raw_deflate.read_to_end(&mut out).map_err(|e| {
                HttpxError::DecodingError(format!("Failed to decode deflate response body: {e}"))
            })?;
            Ok(Bytes::from(out))
        }
        _ => Ok(raw),
    }
}
