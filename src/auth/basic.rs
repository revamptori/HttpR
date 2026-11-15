//! HTTP Basic authentication (RFC 7617).

use crate::exceptions::Result;
use crate::request::Request;
use base64::{engine::general_purpose::STANDARD, Engine as _};

/// HTTP Basic authentication credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl BasicAuth {
    /// Creates a new [`BasicAuth`] credential pair.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Computes the `Authorization` header value (`Basic <base64>`).
    pub fn header_value(&self) -> String {
        let raw = format!("{}:{}", self.username, self.password);
        format!("Basic {}", STANDARD.encode(raw.as_bytes()))
    }

    /// Applies the `Authorization` header to `request`.
    pub fn apply(&self, request: &mut Request) -> Result<()> {
        request
            .headers
            .insert("authorization", &self.header_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_auth_header() {
        let auth = BasicAuth::new("alice", "secret");
        assert_eq!(auth.header_value(), "Basic YWxpY2U6c2VjcmV0");
    }
}
