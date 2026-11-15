//! `httpx-rs`: A high-performance HTTPX-compatible HTTP client for Python powered by Rust.
//!
//! Architecture:
//! - [`client`]: Synchronous (`Client`) and asynchronous (`AsyncClient`) client implementations.
//! - [`transport`]: HTTP/1.1, HTTP/2, TLS (`rustls`), and connection pooling (`hyper`).
//! - [`request`]: Request building, case-insensitive headers, cookie jar, and multipart encoding.
//! - [`response`]: Response representation with lazy text decoding and async/sync streaming.
//! - [`auth`]: Basic, Bearer, and Digest authentication flows.
//! - [`proxy`]: HTTP, HTTPS, SOCKS5, and environment proxy discovery.
//! - [`middleware`]: Timeout, redirect, retry, and Tower service abstractions.
//! - [`exceptions`]: HTTPX-compatible Rust error and Python exception hierarchy.
//! - [`python`]: PyO3 native bindings exposing the `httpx` Python API.

pub mod auth;
pub mod client;
pub mod exceptions;
pub mod middleware;
pub mod proxy;
pub mod python;
pub mod request;
pub mod response;
pub mod transport;

pub use auth::{AuthMethod, BasicAuth, BearerAuth, DigestAuth};
pub use client::{shared_runtime, AsyncClient, Client, ClientBuilder, ClientConfig, SendOptions};
pub use exceptions::{HttpxError, Result};
pub use middleware::{RedirectConfig, RetryConfig, TimeoutConfig};
pub use proxy::{ProxyAuth, ProxyConfig, ProxyMounts, ProxyScheme};
pub use python::_httpx_rs;
pub use request::{
    BoxByteStream, Cookie, CookieJar, Headers, MultipartEncoder, MultipartField, MultipartFile,
    Request, RequestBody, RequestBuilder,
};
pub use response::{Response, ResponseStream};
pub use transport::{ClientCertConfig, ConnectionPool, HttpTransport, PoolLimits, TlsConfig, TlsVerify};
