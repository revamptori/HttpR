//! HTTP cookie storage, `Set-Cookie` parsing, and `Cookie` header generation.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use url::Url;

/// A single stored HTTP cookie with optional attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<SystemTime>,
    pub host_only: bool,
}

impl Cookie {
    /// Checks whether the cookie has expired relative to `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.expires_at.map(|exp| now >= exp).unwrap_or(false)
    }

    /// Checks whether this cookie matches the target URL according to RFC 6265 rules.
    pub fn matches_url(&self, url: &Url, now: SystemTime) -> bool {
        if self.is_expired(now) {
            return false;
        }
        if self.secure && url.scheme() != "https" {
            return false;
        }
        let host = match url.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return false,
        };
        if !domain_matches(&host, &self.domain, self.host_only) {
            return false;
        }
        let req_path = url.path();
        path_matches(req_path, &self.path)
    }
}

/// Thread-safe cookie jar compatible with HTTPX `Cookies`.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    inner: Arc<RwLock<HashMap<(String, String, String), Cookie>>>,
}

impl CookieJar {
    /// Creates an empty [`CookieJar`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or updates a cookie directly.
    pub fn set(&self, name: impl Into<String>, value: impl Into<String>, domain: Option<&str>, path: Option<&str>) {
        let name = name.into();
        let value = value.into();
        let domain_str = domain.unwrap_or("").trim_start_matches('.').to_ascii_lowercase();
        let path_str = path.unwrap_or("/").to_string();
        let host_only = domain.is_none();
        let cookie = Cookie {
            name: name.clone(),
            value,
            domain: domain_str.clone(),
            path: path_str.clone(),
            secure: false,
            http_only: false,
            expires_at: None,
            host_only,
        };
        self.inner
            .write()
            .insert((domain_str, path_str, name), cookie);
    }

    /// Gets the first cookie value matching `name` (optionally filtered by `domain` and `path`).
    pub fn get(&self, name: &str, domain: Option<&str>, path: Option<&str>) -> Option<String> {
        let now = SystemTime::now();
        let guard = self.inner.read();
        for cookie in guard.values() {
            if cookie.is_expired(now) {
                continue;
            }
            if cookie.name != name {
                continue;
            }
            if let Some(d) = domain {
                let d_norm = d.trim_start_matches('.').to_ascii_lowercase();
                if !cookie.domain.is_empty() && cookie.domain != d_norm {
                    continue;
                }
            }
            if let Some(p) = path {
                if cookie.path != p {
                    continue;
                }
            }
            return Some(cookie.value.clone());
        }
        None
    }

    /// Removes cookies matching `name` (and optional `domain`/`path`).
    pub fn delete(&self, name: &str, domain: Option<&str>, path: Option<&str>) {
        let mut guard = self.inner.write();
        guard.retain(|(d, p, n), _| {
            if n != name {
                return true;
            }
            if let Some(dom) = domain {
                if d != &dom.trim_start_matches('.').to_ascii_lowercase() {
                    return true;
                }
            }
            if let Some(pth) = path {
                if p != pth {
                    return true;
                }
            }
            false
        });
    }

    /// Clears all cookies from the jar.
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// Parses and stores a `Set-Cookie` header returned for `request_url`.
    pub fn extract_set_cookie(&self, header_value: &str, request_url: &Url) {
        if let Some(cookie) = parse_set_cookie(header_value, request_url) {
            let key = (
                cookie.domain.clone(),
                cookie.path.clone(),
                cookie.name.clone(),
            );
            let mut guard = self.inner.write();
            if cookie.is_expired(SystemTime::now()) {
                guard.remove(&key);
            } else {
                guard.insert(key, cookie);
            }
        }
    }

    /// Builds the `Cookie` header string for `url`, combining jar cookies and per-request overrides.
    pub fn cookie_header_for_url(
        &self,
        url: &Url,
        extra: Option<&HashMap<String, String>>,
    ) -> Option<String> {
        let now = SystemTime::now();
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        if let Some(extra_cookies) = extra {
            for (k, v) in extra_cookies {
                seen_names.insert(k.clone());
                pairs.push((k.clone(), v.clone()));
            }
        }

        let guard = self.inner.read();
        let mut matching: Vec<&Cookie> = guard
            .values()
            .filter(|c| {
                !seen_names.contains(&c.name)
                    && (c.domain.is_empty() || c.matches_url(url, now))
            })
            .collect();

        // RFC 6265: longer paths first
        matching.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        for c in matching {
            if seen_names.insert(c.name.clone()) {
                pairs.push((c.name.clone(), c.value.clone()));
            }
        }

        if pairs.is_empty() {
            None
        } else {
            Some(
                pairs
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    /// Returns all non-expired `(name, value)` pairs in the jar.
    pub fn items(&self) -> Vec<(String, String)> {
        let now = SystemTime::now();
        self.inner
            .read()
            .values()
            .filter(|c| !c.is_expired(now))
            .map(|c| (c.name.clone(), c.value.clone()))
            .collect()
    }

    /// Returns all non-expired [`Cookie`] structs in the jar.
    pub fn all_cookies(&self) -> Vec<Cookie> {
        let now = SystemTime::now();
        self.inner
            .read()
            .values()
            .filter(|c| !c.is_expired(now))
            .cloned()
            .collect()
    }

    /// Returns the number of stored non-expired cookies.
    pub fn len(&self) -> usize {
        let now = SystemTime::now();
        self.inner
            .read()
            .values()
            .filter(|c| !c.is_expired(now))
            .count()
    }

    /// Returns `true` if no non-expired cookies exist.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Parses a single `Set-Cookie` header according to RFC 6265 conventions.
pub fn parse_set_cookie(header: &str, request_url: &Url) -> Option<Cookie> {
    let mut parts = header.split(';');
    let name_value = parts.next()?.trim();
    let (name, value) = name_value.split_once('=')?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let value = value.trim().to_string();

    let default_host = request_url
        .host_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let default_path = default_cookie_path(request_url.path());

    let mut domain = default_host.clone();
    let mut host_only = true;
    let mut path = default_path;
    let mut secure = false;
    let mut http_only = false;
    let mut expires_at: Option<SystemTime> = None;

    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        if let Some((k, v)) = attr.split_once('=') {
            let k_lower = k.trim().to_ascii_lowercase();
            let v_trimmed = v.trim();
            match k_lower.as_str() {
                "domain" => {
                    let dom = v_trimmed.trim_start_matches('.').to_ascii_lowercase();
                    if !dom.is_empty() && domain_matches(&default_host, &dom, false) {
                        domain = dom;
                        host_only = false;
                    }
                }
                "path" => {
                    if v_trimmed.starts_with('/') {
                        path = v_trimmed.to_string();
                    }
                }
                "max-age" => {
                    if let Ok(secs) = v_trimmed.parse::<i64>() {
                        if secs <= 0 {
                            expires_at = Some(SystemTime::UNIX_EPOCH);
                        } else {
                            expires_at = SystemTime::now().checked_add(Duration::from_secs(secs as u64));
                        }
                    }
                }
                _ => {}
            }
        } else {
            match attr.to_ascii_lowercase().as_str() {
                "secure" => secure = true,
                "httponly" => http_only = true,
                _ => {}
            }
        }
    }

    Some(Cookie {
        name,
        value,
        domain,
        path,
        secure,
        http_only,
        expires_at,
        host_only,
    })
}

fn default_cookie_path(uri_path: &str) -> String {
    if uri_path.is_empty() || !uri_path.starts_with('/') {
        return "/".to_string();
    }
    match uri_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => uri_path[..idx].to_string(),
    }
}

fn domain_matches(request_host: &str, cookie_domain: &str, host_only: bool) -> bool {
    if cookie_domain.is_empty() {
        return true;
    }
    if host_only {
        return request_host == cookie_domain;
    }
    if request_host == cookie_domain {
        return true;
    }
    if request_host.ends_with(cookie_domain) {
        let prefix_len = request_host.len() - cookie_domain.len();
        return request_host.as_bytes().get(prefix_len.saturating_sub(1)) == Some(&b'.');
    }
    false
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if cookie_path == "/" || request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        return request_path.as_bytes().get(cookie_path.len()) == Some(&b'/');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_jar_extract_and_match() {
        let jar = CookieJar::new();
        let url = Url::parse("https://api.example.com/v1/users").unwrap();
        jar.extract_set_cookie("session=xyz; Path=/v1; Domain=example.com; Secure; HttpOnly", &url);

        let header = jar.cookie_header_for_url(&url, None);
        assert_eq!(header.as_deref(), Some("session=xyz"));

        let http_url = Url::parse("http://api.example.com/v1/users").unwrap();
        assert_eq!(jar.cookie_header_for_url(&http_url, None), None);
    }
}
