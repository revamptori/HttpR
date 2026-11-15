//! HTTP/2 multiplexed connection support using Hyper.

use crate::exceptions::{HttpxError, Result};
use crate::middleware::timeout::{with_read_timeout, with_write_timeout, TimeoutConfig};
use crate::request::Request;
use crate::response::Response;
use crate::transport::http1::{
    build_hyper_request, convert_hyper_response, map_hyper_error, HyperReqBody,
};
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};

/// Multiplexed HTTP/2 client connection handle.
///
/// Cloning [`Http2Connection`] is inexpensive and allows concurrent streams
/// to share the same underlying TCP/TLS socket.
#[derive(Clone)]
pub struct Http2Connection {
    sender: http2::SendRequest<HyperReqBody>,
    created_at: Instant,
}

impl Http2Connection {
    /// Performs an HTTP/2 handshake over `io` and spawns the background H2 driver.
    pub async fn handshake<S>(io: S) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (sender, conn) = http2::Builder::new(TokioExecutor::new())
            .initial_stream_window_size(2 * 1024 * 1024)
            .initial_connection_window_size(4 * 1024 * 1024)
            .keep_alive_interval(Some(Duration::from_secs(30)))
            .keep_alive_timeout(Duration::from_secs(10))
            .keep_alive_while_idle(false)
            .adaptive_window(true)
            .handshake::<_, HyperReqBody>(TokioIo::new(io))
            .await
            .map_err(|e| HttpxError::ConnectError(format!("HTTP/2 handshake failed: {e}")))?;

        tokio::spawn(async move {
            let _ = conn.await;
        });

        Ok(Self {
            sender,
            created_at: Instant::now(),
        })
    }

    /// Returns `true` if the HTTP/2 session is still open and able to accept streams.
    pub fn is_usable(&self) -> bool {
        !self.sender.is_closed()
    }

    /// Returns the timestamp when this HTTP/2 session was established.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Sends an HTTP/2 request over a newly multiplexed stream.
    pub async fn send_request(
        &mut self,
        request: &Request,
        timeout: &TimeoutConfig,
        start_time: Instant,
    ) -> Result<Response> {
        self.sender.ready().await.map_err(|e| {
            HttpxError::ConnectError(format!("HTTP/2 connection not ready: {e}"))
        })?;

        let hyper_req = build_hyper_request(request, true, None)?;
        let send_fut = self.sender.send_request(hyper_req);

        let hyper_resp = with_write_timeout(timeout.write, async {
            with_read_timeout(timeout.read, async {
                send_fut.await.map_err(map_hyper_error)
            })
            .await
        })
        .await?;

        Ok(convert_hyper_response(
            hyper_resp,
            request,
            timeout.read,
            start_time,
        ))
    }
}
