//! HTTP Bearer token authentication (RFC 6750).

use crate::exceptions::Result;
use crate::request::Request;

/// HTTP Bearer token credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerAuth {
    pub token: String,
}

impl BearerAuth {
    /// Creates a new [`BearerAuth`] instance.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Returns the `Authorization` header value (`Bearer <token>`).
    pub fn header_value(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// Applies the `Authorization` header to `request`.
    pub fn apply(&self, request: &mut Request) -> Result<()> {
        request
            .headers
            .insert("authorization", &self.header_value())
    }
}
