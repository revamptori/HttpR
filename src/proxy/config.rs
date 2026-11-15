//! Proxy configuration, URL matching, and environment proxy discovery.

use crate::exceptions::{HttpxError, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::HashMap;
use url::Url;

/// Supported proxy transport protocols.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProxyScheme {
    Http,
    Https,
    Socks5 { remote_dns: bool },
}

/// Optional username/password authentication for a proxy server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

impl ProxyAuth {
    /// Returns the HTTP `Proxy-Authorization: Basic ...` header value.
    pub fn basic_header_value(&self) -> String {
        let raw = format!("{}:{}", self.username, self.password);
        format!("Basic {}", STANDARD.encode(raw.as_bytes()))
    }
}

/// Parsed single proxy endpoint configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProxyConfig {
    pub url: Url,
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub auth: Option<ProxyAuth>,
}

impl ProxyConfig {
    /// Parses a proxy URL string (`http://`, `https://`, `socks5://`, `socks5h://`).
    pub fn parse(raw_url: &str) -> Result<Self> {
        let parsed = Url::parse(raw_url).map_err(|e| {
            HttpxError::ProxyError(format!("Invalid proxy URL '{raw_url}': {e}"))
        })?;

        let scheme = match parsed.scheme().to_ascii_lowercase().as_str() {
            "http" => ProxyScheme::Http,
            "https" => ProxyScheme::Https,
            "socks5" => ProxyScheme::Socks5 { remote_dns: false },
            "socks5h" => ProxyScheme::Socks5 { remote_dns: true },
            other => {
                return Err(HttpxError::ProxyError(format!(
                    "Unsupported proxy protocol '{other}://'"
                )));
            }
        };

        let host = parsed
            .host_str()
            .ok_or_else(|| HttpxError::ProxyError(format!("Proxy URL missing host: '{raw_url}'")))?
            .to_string();

        let default_port = match scheme {
            ProxyScheme::Http => 80,
            ProxyScheme::Https => 443,
            ProxyScheme::Socks5 { .. } => 1080,
        };
        let port = parsed.port().unwrap_or(default_port);

        let auth = if !parsed.username().is_empty() || parsed.password().is_some() {
            Some(ProxyAuth {
                username: percent_encoding::percent_decode_str(parsed.username())
                    .decode_utf8_lossy()
                    .into_owned(),
                password: percent_encoding::percent_decode_str(parsed.password().unwrap_or(""))
                    .decode_utf8_lossy()
                    .into_owned(),
            })
        } else {
            None
        };

        Ok(Self {
            url: parsed,
            scheme,
            host,
            port,
            auth,
        })
    }
}

/// Routing table mapping target URL patterns (like `all://`, `http://`, `https://`,
/// or `https://example.com`) to [`ProxyConfig`] entries, plus `NO_PROXY` bypass rules.
#[derive(Debug, Clone, Default)]
pub struct ProxyMounts {
    mounts: Vec<(String, Option<ProxyConfig>)>,
    no_proxy: Vec<String>,
}

impl ProxyMounts {
    /// Creates an empty [`ProxyMounts`] table (direct connections).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a [`ProxyMounts`] applying a single proxy URL to all traffic (`all://`).
    pub fn single(proxy_url: &str) -> Result<Self> {
        let cfg = ProxyConfig::parse(proxy_url)?;
        Ok(Self {
            mounts: vec![("all://".to_string(), Some(cfg))],
            no_proxy: Vec::new(),
        })
    }

    /// Builds [`ProxyMounts`] from a map of mount patterns (`"http://"`, `"https://"`, `"all://"`, etc.)
    /// to optional proxy URLs (`None` disables proxying for that pattern).
    pub fn from_map(map: HashMap<String, Option<String>>) -> Result<Self> {
        let mut mounts = Vec::with_capacity(map.len());
        for (pattern, maybe_url) in map {
            let cfg = match maybe_url {
                Some(u) if !u.trim().is_empty() => Some(ProxyConfig::parse(&u)?),
                _ => None,
            };
            mounts.push((pattern, cfg));
        }
        // More specific patterns (longer) take precedence over generic ones like "all://".
        mounts.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Ok(Self {
            mounts,
            no_proxy: Vec::new(),
        })
    }

    /// Loads proxy configuration from standard environment variables
    /// (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and lowercase variants).
    pub fn from_env() -> Result<Self> {
        let mut mounts = Vec::new();
        if let Some(val) = get_env_ci("HTTPS_PROXY") {
            if let Ok(cfg) = ProxyConfig::parse(&val) {
                mounts.push(("https://".to_string(), Some(cfg)));
            }
        }
        if let Some(val) = get_env_ci("HTTP_PROXY") {
            if let Ok(cfg) = ProxyConfig::parse(&val) {
                mounts.push(("http://".to_string(), Some(cfg)));
            }
        }
        if let Some(val) = get_env_ci("ALL_PROXY") {
            if let Ok(cfg) = ProxyConfig::parse(&val) {
                mounts.push(("all://".to_string(), Some(cfg)));
            }
        }

        let no_proxy = get_env_ci("NO_PROXY")
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        mounts.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Ok(Self { mounts, no_proxy })
    }

    /// Merges explicit mounts over environment mounts if `trust_env` is enabled.
    pub fn merge_with_env(explicit: Option<Self>, trust_env: bool) -> Result<Self> {
        match (explicit, trust_env) {
            (Some(mut exp), true) => {
                let env = Self::from_env()?;
                if exp.no_proxy.is_empty() {
                    exp.no_proxy = env.no_proxy;
                }
                for (pat, cfg) in env.mounts {
                    if !exp.mounts.iter().any(|(p, _)| p == &pat) {
                        exp.mounts.push((pat, cfg));
                    }
                }
                exp.mounts.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
                Ok(exp)
            }
            (Some(exp), false) => Ok(exp),
            (None, true) => Self::from_env(),
            (None, false) => Ok(Self::new()),
        }
    }

    /// Selects the matching [`ProxyConfig`] for `target_url`, respecting `NO_PROXY`.
    pub fn select_for_url(&self, target_url: &Url) -> Option<&ProxyConfig> {
        let host = target_url.host_str().unwrap_or("").to_ascii_lowercase();
        let port = target_url.port_or_known_default();
        if self.is_bypassed(&host, port) {
            return None;
        }

        for (pattern, maybe_cfg) in &self.mounts {
            if pattern_matches(pattern, target_url) {
                return maybe_cfg.as_ref();
            }
        }
        None
    }

    fn is_bypassed(&self, host: &str, port: Option<u16>) -> bool {
        if host.is_empty() || self.no_proxy.is_empty() {
            return false;
        }
        for rule in &self.no_proxy {
            if rule == "*" {
                return true;
            }
            let (rule_host, rule_port) = match rule.rsplit_once(':') {
                Some((h, p)) if !h.contains(']') => match p.parse::<u16>() {
                    Ok(parsed_p) => (h, Some(parsed_p)),
                    Err(_) => (rule.as_str(), None),
                },
                _ => (rule.as_str(), None),
            };
            if let Some(rp) = rule_port {
                if Some(rp) != port {
                    continue;
                }
            }
            let clean_rule = rule_host.trim_start_matches('.');
            if !clean_rule.is_empty()
                && (host == clean_rule
                    || (host.ends_with(clean_rule)
                        && host
                            .len()
                            .checked_sub(clean_rule.len() + 1)
                            .and_then(|idx| host.as_bytes().get(idx))
                            == Some(&b'.')))
            {
                return true;
            }
        }
        false
    }
}

fn pattern_matches(pattern: &str, url: &Url) -> bool {
    let pat = pattern.trim().to_ascii_lowercase();
    if pat == "all://" || pat == "all" || pat == "*" {
        return true;
    }
    let scheme = url.scheme().to_ascii_lowercase();
    if pat == format!("{scheme}://") || pat == scheme {
        return true;
    }
    if let Ok(pat_url) = Url::parse(&pat) {
        let scheme_ok = pat_url.scheme() == "all" || pat_url.scheme() == scheme;
        let host_ok = match pat_url.host_str() {
            Some(ph) => url
                .host_str()
                .map(|uh| uh.eq_ignore_ascii_case(ph))
                .unwrap_or(false),
            None => true,
        };
        let port_ok = match pat_url.port() {
            Some(pp) => url.port_or_known_default() == Some(pp),
            None => true,
        };
        return scheme_ok && host_ok && port_ok;
    }
    false
}

fn get_env_ci(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| std::env::var(key.to_ascii_lowercase()).ok())
        .filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_mounts_selection_and_no_proxy() {
        let mut map = HashMap::new();
        map.insert("http://".to_string(), Some("http://127.0.0.1:8080".to_string()));
        map.insert(
            "https://".to_string(),
            Some("socks5h://user:pass@127.0.0.1:1080".to_string()),
        );
        let mut mounts = ProxyMounts::from_map(map).unwrap();
        mounts.no_proxy = vec!["localhost".to_string(), ".internal.local".to_string()];

        let u1 = Url::parse("http://example.com/a").unwrap();
        assert_eq!(mounts.select_for_url(&u1).unwrap().port, 8080);

        let u2 = Url::parse("https://example.com/a").unwrap();
        let p2 = mounts.select_for_url(&u2).unwrap();
        assert_eq!(p2.port, 1080);
        assert_eq!(
            p2.scheme,
            ProxyScheme::Socks5 { remote_dns: true }
        );

        let u3 = Url::parse("https://svc.internal.local/health").unwrap();
        assert!(mounts.select_for_url(&u3).is_none());
    }
}
