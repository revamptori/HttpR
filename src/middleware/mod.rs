//! Middleware abstractions for timeouts, redirects, retries, and Tower service integration.

pub mod redirects;
pub mod retries;
pub mod timeout;

pub use redirects::{next_redirect_request, RedirectConfig};
pub use retries::RetryConfig;
pub use timeout::{
    with_connect_timeout, with_pool_timeout, with_read_timeout, with_write_timeout, TimeoutConfig,
};

use crate::exceptions::HttpxError;
use crate::request::Request;
use crate::response::Response;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;

/// A boxed [`tower::Service`] accepting [`Request`] and returning [`Response`].
pub type BoxHttpService = tower::util::BoxCloneService<Request, Response, HttpxError>;

/// Wraps an async request-execution closure as a [`tower::Service`].
#[derive(Clone)]
pub struct FnService<F> {
    func: F,
}

impl<F> FnService<F> {
    /// Creates a new [`FnService`].
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

impl<F, Fut> Service<Request> for FnService<F>
where
    F: Fn(Request) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = crate::exceptions::Result<Response>> + Send + 'static,
{
    type Response = Response;
    type Error = HttpxError;
    type Future = Pin<Box<dyn Future<Output = crate::exceptions::Result<Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request) -> Self::Future {
        Box::pin((self.func)(req))
    }
}
