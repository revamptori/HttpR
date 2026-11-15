//! HTTP redirect resolution and loop protection matching HTTPX semantics.

use crate::exceptions::{HttpxError, Result};
use crate::request::Request;
use crate::response::Response;
use http::Method;
use url::Url;

/// Redirect policy configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedirectConfig {
    pub follow_redirects: bool,
    pub max_redirects: usize,
}

impl Default for RedirectConfig {
    fn default() -> Self {
        Self {
            follow_redirects: false,
            max_redirects: 20,
        }
    }
}

/// Evaluates whether `response` triggers a redirect and builds the next [`Request`] if so.
pub fn next_redirect_request(
    current_request: &Request,
    response: &Response,
    redirects_followed: usize,
    max_redirects: usize,
) -> Result<Option<Request>> {
    if !response.has_redirect_location() {
        return Ok(None);
    }

    if redirects_followed >= max_redirects {
        return Err(HttpxError::TooManyRedirects {
            max: max_redirects,
            message: format!("Exceeded maximum of {max_redirects} redirects at {}", response.url),
        });
    }

    let location = response
        .headers
        .get("location")
        .ok_or_else(|| HttpxError::RemoteProtocolError("Redirect missing Location header".into()))?;

    let next_url = resolve_redirect_url(&response.url, &location)?;
    let (next_method, drop_body) = match response.status_code {
        303 => {
            if current_request.method == Method::HEAD {
                (Method::HEAD, true)
            } else {
                (Method::GET, true)
            }
        }
        301 | 302 => {
            if current_request.method == Method::POST {
                (Method::GET, true)
            } else {
                (current_request.method.clone(), false)
            }
        }
        307 | 308 => (current_request.method.clone(), false),
        _ => return Ok(None),
    };

    if !drop_body && !current_request.body.is_replayable() {
        return Err(HttpxError::LocalProtocolError(
            "Cannot replay a streaming request body across a 307/308 redirect".into(),
        ));
    }

    let mut next_req = current_request.clone_for_redirect(next_method, next_url, drop_body);

    // Preserve URL fragment if Location didn't specify one.
    if next_req.url.fragment().is_none() {
        if let Some(frag) = current_request.url.fragment() {
            next_req.url.set_fragment(Some(frag));
        }
    }

    Ok(Some(next_req))
}

fn resolve_redirect_url(base: &Url, location: &str) -> Result<Url> {
    let trimmed = location.trim();
    let next = base.join(trimmed).map_err(|e| {
        HttpxError::RemoteProtocolError(format!(
            "Invalid redirect Location '{trimmed}' from '{base}': {e}"
        ))
    })?;

    match next.scheme() {
        "http" | "https" => Ok(next),
        other => Err(HttpxError::UnsupportedProtocol(format!(
            "Redirect target uses unsupported scheme '{other}://'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{CookieJar, Headers};
    use bytes::Bytes;
    use http::{StatusCode, Version};
    use std::time::Duration;

    #[test]
    fn test_redirect_302_post_becomes_get() {
        let req = Request::builder("POST", "https://example.com/login")
            .unwrap()
            .content("user=a")
            .build()
            .unwrap();
        let mut h = Headers::new();
        h.insert("location", "/dashboard").unwrap();
        let resp = Response::new(
            StatusCode::FOUND,
            h,
            CookieJar::new(),
            req.url.clone(),
            Version::HTTP_11,
            Duration::from_millis(1),
            Bytes::new(),
            Some(req.clone()),
            vec![],
        );

        let next = next_redirect_request(&req, &resp, 0, 20).unwrap().unwrap();
        assert_eq!(next.method, Method::GET);
        assert_eq!(next.url.as_str(), "https://example.com/dashboard");
        assert!(next.body.is_empty());
    }
}
