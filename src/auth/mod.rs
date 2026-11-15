//! HTTP authentication mechanisms (Basic, Bearer, Digest).

pub mod basic;
pub mod bearer;
pub mod digest;

pub use basic::BasicAuth;
pub use bearer::BearerAuth;
pub use digest::DigestAuth;

use crate::exceptions::Result;
use crate::request::Request;
use crate::response::Response;

/// Supported client and per-request authentication schemes.
#[derive(Debug, Clone)]
pub enum AuthMethod {
    Basic(BasicAuth),
    Bearer(BearerAuth),
    Digest(DigestAuth),
}

impl AuthMethod {
    /// Applies preemptive authentication headers before sending a request.
    pub fn prepare_request(&self, request: &mut Request) -> Result<()> {
        if request.headers.contains_key("authorization") {
            return Ok(());
        }
        match self {
            Self::Basic(b) => b.apply(request),
            Self::Bearer(b) => b.apply(request),
            Self::Digest(d) => d.apply_if_cached(request),
        }
    }

    /// Inspects a response for a 401 challenge (for example Digest auth) and updates `request`
    /// if a retry with authentication credentials should be attempted.
    pub fn handle_response(&self, response: &Response, request: &mut Request) -> Result<bool> {
        if response.status_code != 401 {
            return Ok(false);
        }
        match self {
            Self::Digest(d) => {
                if let Some(www_auth) = response.headers.get("www-authenticate") {
                    d.handle_challenge(&www_auth, request)
                } else {
                    Ok(false)
                }
            }
            Self::Basic(_) | Self::Bearer(_) => Ok(false),
        }
    }
}
