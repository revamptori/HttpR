//! Efficient case-insensitive HTTP header storage and manipulation.

use crate::exceptions::{HttpxError, Result};
use http::header::{HeaderMap, HeaderName, HeaderValue};
use std::fmt;

/// Case-insensitive multi-value HTTP header container backed by [`http::HeaderMap`].
#[derive(Clone, Default)]
pub struct Headers {
    inner: HeaderMap,
}

impl Headers {
    /// Creates an empty [`Headers`] container.
    pub fn new() -> Self {
        Self {
            inner: HeaderMap::new(),
        }
    }

    /// Creates a [`Headers`] container with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HeaderMap::with_capacity(capacity),
        }
    }

    /// Wraps an existing [`http::HeaderMap`].
    pub fn from_header_map(map: HeaderMap) -> Self {
        Self { inner: map }
    }

    /// Returns a reference to the underlying [`http::HeaderMap`].
    pub fn as_header_map(&self) -> &HeaderMap {
        &self.inner
    }

    /// Consumes self and returns the underlying [`http::HeaderMap`].
    pub fn into_header_map(self) -> HeaderMap {
        self.inner
    }

    /// Inserts or replaces a header value (case-insensitive key).
    pub fn insert(&mut self, name: &str, value: &str) -> Result<()> {
        let h_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| HttpxError::LocalProtocolError(format!("Invalid header name '{name}': {e}")))?;
        let h_val = HeaderValue::from_str(value)
            .map_err(|e| HttpxError::LocalProtocolError(format!("Invalid header value for '{name}': {e}")))?;
        self.inner.insert(h_name, h_val);
        Ok(())
    }

    /// Appends a header value without removing existing values for the same key.
    pub fn append(&mut self, name: &str, value: &str) -> Result<()> {
        let h_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| HttpxError::LocalProtocolError(format!("Invalid header name '{name}': {e}")))?;
        let h_val = HeaderValue::from_str(value)
            .map_err(|e| HttpxError::LocalProtocolError(format!("Invalid header value for '{name}': {e}")))?;
        self.inner.append(h_name, h_val);
        Ok(())
    }

    /// Inserts a header only if the key is not already present.
    pub fn insert_if_absent(&mut self, name: &str, value: &str) -> Result<()> {
        if !self.contains_key(name) {
            self.insert(name, value)?;
        }
        Ok(())
    }

    /// Returns the first value for `name`, joined/decoded as UTF-8 if valid, or joined via `, ` when
    /// multiple values exist (matching HTTPX `Headers.get` behavior).
    pub fn get(&self, name: &str) -> Option<String> {
        let mut iter = self.inner.get_all(name).iter().peekable();
        let first = iter.next()?;
        let first_str = String::from_utf8_lossy(first.as_bytes());
        if iter.peek().is_none() {
            return Some(first_str.into_owned());
        }
        let mut combined = first_str.into_owned();
        for val in iter {
            combined.push_str(", ");
            combined.push_str(&String::from_utf8_lossy(val.as_bytes()));
        }
        Some(combined)
    }

    /// Returns all values associated with `name`.
    pub fn get_list(&self, name: &str, split_commas: bool) -> Vec<String> {
        let mut out = Vec::new();
        for val in self.inner.get_all(name) {
            let s = String::from_utf8_lossy(val.as_bytes());
            if split_commas {
                for part in s.split(',') {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                }
            } else {
                out.push(s.into_owned());
            }
        }
        out
    }

    /// Checks whether a header exists (case-insensitive).
    pub fn contains_key(&self, name: &str) -> bool {
        self.inner.contains_key(name)
    }

    /// Removes all values for `name`.
    pub fn remove(&mut self, name: &str) -> bool {
        self.inner.remove(name).is_some()
    }

    /// Number of header entries (counting multi-value entries individually).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if there are no headers.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterates over `(name, value)` pairs including duplicate keys.
    pub fn multi_items(&self) -> Vec<(String, String)> {
        self.inner
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect()
    }

    /// Iterates over unique header keys with combined values.
    pub fn items(&self) -> Vec<(String, String)> {
        let mut seen = Vec::new();
        for name in self.inner.keys() {
            let k = name.as_str();
            if let Some(v) = self.get(k) {
                seen.push((k.to_string(), v));
            }
        }
        seen
    }

    /// Merges headers from `other`, overwriting existing keys unless `other` has multi-values.
    pub fn update(&mut self, other: &Headers) {
        let mut cleared = std::collections::HashSet::new();
        for (name, val) in other.inner.iter() {
            if cleared.insert(name.clone()) {
                self.inner.remove(name);
            }
            self.inner.append(name.clone(), val.clone());
        }
    }

    /// Extracts charset from the `content-type` header if present.
    pub fn content_type_charset(&self) -> Option<String> {
        let ct = self.get("content-type")?;
        for param in ct.split(';').skip(1) {
            let trimmed = param.trim();
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim().eq_ignore_ascii_case("charset") {
                    let cleaned = v.trim().trim_matches('"').trim_matches('\'');
                    if !cleaned.is_empty() {
                        return Some(cleaned.to_string());
                    }
                }
            }
        }
        None
    }
}

impl fmt::Debug for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.multi_items()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_headers_case_insensitive_and_multivalue() {
        let mut h = Headers::new();
        h.insert("Content-Type", "text/html; charset=utf-8").unwrap();
        assert_eq!(
            h.get("content-type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(h.content_type_charset().as_deref(), Some("utf-8"));

        h.append("Set-Cookie", "a=1").unwrap();
        h.append("Set-Cookie", "b=2").unwrap();
        assert_eq!(h.get_list("set-cookie", false), vec!["a=1", "b=2"]);
    }
}
