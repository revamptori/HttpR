//! Error and exception types compatible with Python's HTTPX exception hierarchy.

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError};
use pyo3::prelude::*;
use thiserror::Error;

/// Unified result alias used throughout `httpx-rs`.
pub type Result<T> = std::result::Result<T, HttpxError>;

/// Core Rust error type representing all HTTPX-compatible failure modes.
#[derive(Debug, Error)]
pub enum HttpxError {
    /// Invalid or malformed URL.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Unsupported URL scheme or protocol.
    #[error("Unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    /// Generic request error.
    #[error("Request error: {0}")]
    RequestError(String),

    /// Generic transport error.
    #[error("Transport error: {0}")]
    TransportError(String),

    /// Failed to establish a network or TLS connection.
    #[error("Connect error: {0}")]
    ConnectError(String),

    /// Timed out while establishing connection or TLS handshake.
    #[error("Connect timeout: {0}")]
    ConnectTimeout(String),

    /// Timed out while reading response headers or body.
    #[error("Read timeout: {0}")]
    ReadTimeout(String),

    /// Timed out while writing request headers or body.
    #[error("Write timeout: {0}")]
    WriteTimeout(String),

    /// Timed out waiting for an available connection from the pool.
    #[error("Pool timeout: {0}")]
    PoolTimeout(String),

    /// Generic timeout exception.
    #[error("Timeout: {0}")]
    TimeoutException(String),

    /// Error reading from socket.
    #[error("Read error: {0}")]
    ReadError(String),

    /// Error writing to socket.
    #[error("Write error: {0}")]
    WriteError(String),

    /// Error closing connection.
    #[error("Close error: {0}")]
    CloseError(String),

    /// Local HTTP protocol violation.
    #[error("Local protocol error: {0}")]
    LocalProtocolError(String),

    /// Remote HTTP protocol violation.
    #[error("Remote protocol error: {0}")]
    RemoteProtocolError(String),

    /// Error communicating with or tunneling through a proxy.
    #[error("Proxy error: {0}")]
    ProxyError(String),

    /// Exceeded maximum allowed redirect hops.
    #[error("Exceeded maximum redirects ({max}): {message}")]
    TooManyRedirects { max: usize, message: String },

    /// Failed to decode or decompress response content.
    #[error("Decoding error: {0}")]
    DecodingError(String),

    /// Non-2xx HTTP status error raised by `raise_for_status()`.
    #[error("HTTP {status_code} {reason}: {url}")]
    HTTPStatusError {
        status_code: u16,
        reason: String,
        url: String,
    },

    /// Stream has already been read or closed.
    #[error("Stream error: {0}")]
    StreamError(String),

    /// Generic HTTP error.
    #[error("HTTP error: {0}")]
    HttpError(String),
}

// Python exception hierarchy matching HTTPX:
// Exception
// ├── InvalidURL
// ├── StreamError (RuntimeError)
// └── HTTPError
//     ├── HTTPStatusError
//     └── RequestError
//         ├── DecodingError
//         ├── TooManyRedirects
//         └── TransportError
//             ├── UnsupportedProtocol
//             ├── ProxyError
//             ├── ProtocolError
//             │   ├── LocalProtocolError
//             │   └── RemoteProtocolError
//             ├── NetworkError
//             │   ├── ConnectError
//             │   ├── ReadError
//             │   ├── WriteError
//             │   └── CloseError
//             └── TimeoutException
//                 ├── ConnectTimeout
//                 ├── ReadTimeout
//                 ├── WriteTimeout
//                 └── PoolTimeout

#[allow(unexpected_cfgs)]
mod py_exceptions {
    use super::*;

    create_exception!(httpx, HTTPError, PyException);
    create_exception!(httpx, RequestError, HTTPError);
    create_exception!(httpx, TransportError, RequestError);
    create_exception!(httpx, TimeoutException, TransportError);
    create_exception!(httpx, ConnectTimeout, TimeoutException);
    create_exception!(httpx, ReadTimeout, TimeoutException);
    create_exception!(httpx, WriteTimeout, TimeoutException);
    create_exception!(httpx, PoolTimeout, TimeoutException);
    create_exception!(httpx, NetworkError, TransportError);
    create_exception!(httpx, ConnectError, NetworkError);
    create_exception!(httpx, ReadError, NetworkError);
    create_exception!(httpx, WriteError, NetworkError);
    create_exception!(httpx, CloseError, NetworkError);
    create_exception!(httpx, ProtocolError, TransportError);
    create_exception!(httpx, LocalProtocolError, ProtocolError);
    create_exception!(httpx, RemoteProtocolError, ProtocolError);
    create_exception!(httpx, ProxyError, TransportError);
    create_exception!(httpx, UnsupportedProtocol, TransportError);
    create_exception!(httpx, DecodingError, RequestError);
    create_exception!(httpx, TooManyRedirects, RequestError);
    create_exception!(httpx, HTTPStatusError, HTTPError);
    create_exception!(httpx, InvalidURL, PyException);
    create_exception!(httpx, StreamError, PyRuntimeError);
}

pub use py_exceptions::*;

impl From<HttpxError> for PyErr {
    fn from(err: HttpxError) -> PyErr {
        let msg = err.to_string();
        match err {
            HttpxError::InvalidUrl(_) => InvalidURL::new_err(msg),
            HttpxError::UnsupportedProtocol(_) => UnsupportedProtocol::new_err(msg),
            HttpxError::RequestError(_) => RequestError::new_err(msg),
            HttpxError::TransportError(_) => TransportError::new_err(msg),
            HttpxError::ConnectError(_) => ConnectError::new_err(msg),
            HttpxError::ConnectTimeout(_) => ConnectTimeout::new_err(msg),
            HttpxError::ReadTimeout(_) => ReadTimeout::new_err(msg),
            HttpxError::WriteTimeout(_) => WriteTimeout::new_err(msg),
            HttpxError::PoolTimeout(_) => PoolTimeout::new_err(msg),
            HttpxError::TimeoutException(_) => TimeoutException::new_err(msg),
            HttpxError::ReadError(_) => ReadError::new_err(msg),
            HttpxError::WriteError(_) => WriteError::new_err(msg),
            HttpxError::CloseError(_) => CloseError::new_err(msg),
            HttpxError::LocalProtocolError(_) => LocalProtocolError::new_err(msg),
            HttpxError::RemoteProtocolError(_) => RemoteProtocolError::new_err(msg),
            HttpxError::ProxyError(_) => ProxyError::new_err(msg),
            HttpxError::TooManyRedirects { .. } => TooManyRedirects::new_err(msg),
            HttpxError::DecodingError(_) => DecodingError::new_err(msg),
            HttpxError::HTTPStatusError { .. } => HTTPStatusError::new_err(msg),
            HttpxError::StreamError(_) => StreamError::new_err(msg),
            HttpxError::HttpError(_) => HTTPError::new_err(msg),
        }
    }
}

/// Register all HTTPX exception classes onto the given Python module.
pub fn register_exceptions(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("HTTPError", m.py().get_type_bound::<HTTPError>())?;
    m.add("RequestError", m.py().get_type_bound::<RequestError>())?;
    m.add("TransportError", m.py().get_type_bound::<TransportError>())?;
    m.add(
        "TimeoutException",
        m.py().get_type_bound::<TimeoutException>(),
    )?;
    m.add("ConnectTimeout", m.py().get_type_bound::<ConnectTimeout>())?;
    m.add("ReadTimeout", m.py().get_type_bound::<ReadTimeout>())?;
    m.add("WriteTimeout", m.py().get_type_bound::<WriteTimeout>())?;
    m.add("PoolTimeout", m.py().get_type_bound::<PoolTimeout>())?;
    m.add("NetworkError", m.py().get_type_bound::<NetworkError>())?;
    m.add("ConnectError", m.py().get_type_bound::<ConnectError>())?;
    m.add("ReadError", m.py().get_type_bound::<ReadError>())?;
    m.add("WriteError", m.py().get_type_bound::<WriteError>())?;
    m.add("CloseError", m.py().get_type_bound::<CloseError>())?;
    m.add("ProtocolError", m.py().get_type_bound::<ProtocolError>())?;
    m.add(
        "LocalProtocolError",
        m.py().get_type_bound::<LocalProtocolError>(),
    )?;
    m.add(
        "RemoteProtocolError",
        m.py().get_type_bound::<RemoteProtocolError>(),
    )?;
    m.add("ProxyError", m.py().get_type_bound::<ProxyError>())?;
    m.add(
        "UnsupportedProtocol",
        m.py().get_type_bound::<UnsupportedProtocol>(),
    )?;
    m.add("DecodingError", m.py().get_type_bound::<DecodingError>())?;
    m.add(
        "TooManyRedirects",
        m.py().get_type_bound::<TooManyRedirects>(),
    )?;
    m.add(
        "HTTPStatusError",
        m.py().get_type_bound::<HTTPStatusError>(),
    )?;
    m.add("InvalidURL", m.py().get_type_bound::<InvalidURL>())?;
    m.add("StreamError", m.py().get_type_bound::<StreamError>())?;
    Ok(())
}
