//! Granular request/connect/read/write/pool timeout configuration and enforcement.

use crate::exceptions::{HttpxError, Result};
use std::future::Future;
use std::time::Duration;

/// HTTPX-compatible four-phase timeout configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutConfig {
    pub connect: Option<Duration>,
    pub read: Option<Duration>,
    pub write: Option<Duration>,
    pub pool: Option<Duration>,
}

impl Default for TimeoutConfig {
    /// Matches HTTPX's default 5.0 second timeout across all phases.
    fn default() -> Self {
        let five = Some(Duration::from_secs(5));
        Self {
            connect: five,
            read: five,
            write: five,
            pool: five,
        }
    }
}

impl TimeoutConfig {
    /// Creates a [`TimeoutConfig`] with the same duration for all phases.
    pub fn uniform(duration: Option<Duration>) -> Self {
        Self {
            connect: duration,
            read: duration,
            write: duration,
            pool: duration,
        }
    }

    /// Creates a [`TimeoutConfig`] with all timeouts disabled (`None`).
    pub fn none() -> Self {
        Self::uniform(None)
    }

    /// Creates a [`TimeoutConfig`] from floating-point seconds (with optional per-phase overrides).
    pub fn from_secs(
        default: Option<f64>,
        connect: Option<Option<f64>>,
        read: Option<Option<f64>>,
        write: Option<Option<f64>>,
        pool: Option<Option<f64>>,
    ) -> Result<Self> {
        let base = to_duration(default)?;
        Ok(Self {
            connect: match connect {
                Some(v) => to_duration(v)?,
                None => base,
            },
            read: match read {
                Some(v) => to_duration(v)?,
                None => base,
            },
            write: match write {
                Some(v) => to_duration(v)?,
                None => base,
            },
            pool: match pool {
                Some(v) => to_duration(v)?,
                None => base,
            },
        })
    }
}

fn to_duration(secs: Option<f64>) -> Result<Option<Duration>> {
    match secs {
        None => Ok(None),
        Some(v) if v.is_nan() || v < 0.0 => Err(HttpxError::RequestError(format!(
            "Timeout must be non-negative, got {v}"
        ))),
        Some(v) => Ok(Some(Duration::from_secs_f64(v))),
    }
}

/// Runs `fut` with an optional connect timeout, returning [`HttpxError::ConnectTimeout`] on expiry.
pub async fn with_connect_timeout<F, T>(dur: Option<Duration>, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match dur {
        Some(d) => match tokio::time::timeout(d, fut).await {
            Ok(res) => res,
            Err(_) => Err(HttpxError::ConnectTimeout(format!(
                "Timed out connecting after {d:?}"
            ))),
        },
        None => fut.await,
    }
}

/// Runs `fut` with an optional read timeout, returning [`HttpxError::ReadTimeout`] on expiry.
pub async fn with_read_timeout<F, T>(dur: Option<Duration>, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match dur {
        Some(d) => match tokio::time::timeout(d, fut).await {
            Ok(res) => res,
            Err(_) => Err(HttpxError::ReadTimeout(format!(
                "Timed out waiting for response after {d:?}"
            ))),
        },
        None => fut.await,
    }
}

/// Runs `fut` with an optional write timeout, returning [`HttpxError::WriteTimeout`] on expiry.
pub async fn with_write_timeout<F, T>(dur: Option<Duration>, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match dur {
        Some(d) => match tokio::time::timeout(d, fut).await {
            Ok(res) => res,
            Err(_) => Err(HttpxError::WriteTimeout(format!(
                "Timed out writing request after {d:?}"
            ))),
        },
        None => fut.await,
    }
}

/// Runs `fut` with an optional pool acquisition timeout, returning [`HttpxError::PoolTimeout`] on expiry.
pub async fn with_pool_timeout<F, T>(dur: Option<Duration>, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match dur {
        Some(d) => match tokio::time::timeout(d, fut).await {
            Ok(res) => res,
            Err(_) => Err(HttpxError::PoolTimeout(format!(
                "Timed out acquiring connection from pool after {d:?}"
            ))),
        },
        None => fut.await,
    }
}
