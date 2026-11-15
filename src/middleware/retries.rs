//! Transport-level retry policy with exponential backoff and Tower layer support.

use crate::exceptions::HttpxError;
use std::time::Duration;

/// Retry policy for transient connection/network failures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub backoff_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 0,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
            backoff_factor: 2.0,
        }
    }
}

impl RetryConfig {
    /// Creates a retry configuration with `max_retries` attempts.
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    /// Determines whether an [`HttpxError`] is safe and eligible for retry.
    pub fn is_retryable(&self, err: &HttpxError) -> bool {
        matches!(
            err,
            HttpxError::ConnectError(_) | HttpxError::ConnectTimeout(_)
        )
    }

    /// Computes the delay before retry attempt `attempt` (0-indexed).
    pub fn backoff_duration(&self, attempt: usize) -> Duration {
        let mult = self.backoff_factor.powi(attempt as i32);
        let secs = self.initial_backoff.as_secs_f64() * mult;
        let capped = secs.min(self.max_backoff.as_secs_f64());
        Duration::from_secs_f64(capped)
    }
}
