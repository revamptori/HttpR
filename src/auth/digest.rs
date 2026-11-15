//! HTTP Digest Access Authentication (RFC 2617 / RFC 7616).

use crate::exceptions::{HttpxError, Result};
use crate::request::Request;
use md5::{Digest as _, Md5};
use parking_lot::Mutex;
use rand::Rng;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;

/// Parsed fields from a server `WWW-Authenticate: Digest ...` challenge.
#[derive(Debug, Clone)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    pub algorithm: String,
    pub stale: bool,
}

#[derive(Debug, Default)]
struct DigestState {
    last_challenge: Option<DigestChallenge>,
    nonce_count: u32,
}

/// Stateful HTTP Digest authentication handler.
#[derive(Debug, Clone)]
pub struct DigestAuth {
    pub username: String,
    pub password: String,
    state: Arc<Mutex<DigestState>>,
}

impl DigestAuth {
    /// Creates a new [`DigestAuth`] credential handler.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            state: Arc::new(Mutex::new(DigestState::default())),
        }
    }

    /// Preemptively attaches an `Authorization` header if a previous challenge was cached.
    pub fn apply_if_cached(&self, request: &mut Request) -> Result<()> {
        let mut guard = self.state.lock();
        if let Some(challenge) = guard.last_challenge.clone() {
            guard.nonce_count += 1;
            let nc = guard.nonce_count;
            let header = self.build_authorization_header(request, &challenge, nc)?;
            request.headers.insert("authorization", &header)?;
        }
        Ok(())
    }

    /// Handles a `401 Unauthorized` `WWW-Authenticate` header, updating internal nonce state
    /// and applying the computed `Authorization: Digest ...` header to `request`.
    /// Returns `true` if a Digest challenge was found and applied.
    pub fn handle_challenge(&self, www_authenticate: &str, request: &mut Request) -> Result<bool> {
        let challenge = match parse_digest_challenge(www_authenticate) {
            Some(c) => c,
            None => return Ok(false),
        };

        let mut guard = self.state.lock();
        let reset_nc = guard
            .last_challenge
            .as_ref()
            .map(|prev| prev.nonce != challenge.nonce)
            .unwrap_or(true);
        if reset_nc {
            guard.nonce_count = 1;
        } else {
            guard.nonce_count += 1;
        }
        let nc = guard.nonce_count;
        guard.last_challenge = Some(challenge.clone());

        let header = self.build_authorization_header(request, &challenge, nc)?;
        request.headers.insert("authorization", &header)?;
        Ok(true)
    }

    fn build_authorization_header(
        &self,
        request: &Request,
        challenge: &DigestChallenge,
        nc: u32,
    ) -> Result<String> {
        let uri = match request.url.query() {
            Some(q) => format!("{}?{q}", request.url.path()),
            None => request.url.path().to_string(),
        };
        let method = request.method.as_str();
        let cnonce = generate_cnonce();
        let nc_hex = format!("{nc:08x}");

        let alg_upper = challenge.algorithm.to_ascii_uppercase();
        let is_sess = alg_upper.ends_with("-SESS");

        let ha1_base = hash_hex(
            &alg_upper,
            &format!("{}:{}:{}", self.username, challenge.realm, self.password),
        )?;
        let ha1 = if is_sess {
            hash_hex(&alg_upper, &format!("{ha1_base}:{}:{cnonce}", challenge.nonce))?
        } else {
            ha1_base
        };

        let chosen_qop = choose_qop(challenge.qop.as_deref());
        let ha2 = match chosen_qop.as_deref() {
            Some("auth-int") => {
                let entity_hash = hash_bytes_hex(&alg_upper, &request.content())?;
                hash_hex(&alg_upper, &format!("{method}:{uri}:{entity_hash}"))?
            }
            _ => hash_hex(&alg_upper, &format!("{method}:{uri}"))?,
        };

        let response_digest = match chosen_qop.as_deref() {
            Some(qop) => hash_hex(
                &alg_upper,
                &format!(
                    "{ha1}:{}:{nc_hex}:{cnonce}:{qop}:{ha2}",
                    challenge.nonce
                ),
            )?,
            None => hash_hex(&alg_upper, &format!("{ha1}:{}:{ha2}", challenge.nonce))?,
        };

        let mut parts = vec![
            format!("username=\"{}\"", escape_quoted(&self.username)),
            format!("realm=\"{}\"", escape_quoted(&challenge.realm)),
            format!("nonce=\"{}\"", escape_quoted(&challenge.nonce)),
            format!("uri=\"{}\"", escape_quoted(&uri)),
            format!("algorithm={}", challenge.algorithm),
            format!("response=\"{response_digest}\""),
        ];

        if let Some(qop) = chosen_qop {
            parts.push(format!("qop={qop}"));
            parts.push(format!("nc={nc_hex}"));
            parts.push(format!("cnonce=\"{cnonce}\""));
        }

        if let Some(opaque) = &challenge.opaque {
            parts.push(format!("opaque=\"{}\"", escape_quoted(opaque)));
        }

        Ok(format!("Digest {}", parts.join(", ")))
    }
}

/// Parses a `Digest ...` challenge from a `WWW-Authenticate` header.
pub fn parse_digest_challenge(header: &str) -> Option<DigestChallenge> {
    let trimmed = header.trim();
    let rest = trimmed
        .strip_prefix("Digest ")
        .or_else(|| trimmed.strip_prefix("digest "))?;

    let params = parse_auth_params(rest);
    let realm = params.get("realm")?.clone();
    let nonce = params.get("nonce")?.clone();
    let qop = params.get("qop").cloned();
    let opaque = params.get("opaque").cloned();
    let algorithm = params
        .get("algorithm")
        .cloned()
        .unwrap_or_else(|| "MD5".to_string());
    let stale = params
        .get("stale")
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    Some(DigestChallenge {
        realm,
        nonce,
        qop,
        opaque,
        algorithm,
        stale,
    })
}

fn parse_auth_params(input: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(start, ch)) = chars.peek() {
        if ch.is_whitespace() || ch == ',' {
            chars.next();
            continue;
        }
        let mut eq_idx = None;
        while let Some(&(idx, c)) = chars.peek() {
            if c == '=' {
                eq_idx = Some(idx);
                chars.next();
                break;
            }
            if c == ',' {
                break;
            }
            chars.next();
        }
        let Some(eq) = eq_idx else {
            break;
        };
        let key = input[start..eq].trim().to_ascii_lowercase();

        // Skip whitespace before value
        while let Some(&(_, c)) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        let value = if let Some(&(_, '"')) = chars.peek() {
            chars.next(); // consume opening quote
            let mut val = String::new();
            let mut escaped = false;
            for (_, c) in chars.by_ref() {
                if escaped {
                    val.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                } else {
                    val.push(c);
                }
            }
            val
        } else {
            let mut val = String::new();
            while let Some(&(_, c)) = chars.peek() {
                if c == ',' {
                    chars.next();
                    break;
                }
                val.push(c);
                chars.next();
            }
            val.trim().to_string()
        };

        if !key.is_empty() {
            map.insert(key, value);
        }
    }

    map
}

fn choose_qop(qop: Option<&str>) -> Option<String> {
    let raw = qop?;
    let options: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    if options.iter().any(|o| o.eq_ignore_ascii_case("auth")) {
        Some("auth".to_string())
    } else if options.iter().any(|o| o.eq_ignore_ascii_case("auth-int")) {
        Some("auth-int".to_string())
    } else {
        None
    }
}

fn hash_hex(algorithm: &str, data: &str) -> Result<String> {
    hash_bytes_hex(algorithm, data.as_bytes())
}

fn hash_bytes_hex(algorithm: &str, bytes: &[u8]) -> Result<String> {
    match algorithm {
        "MD5" | "MD5-SESS" => {
            let mut hasher = Md5::new();
            hasher.update(bytes);
            Ok(hex::encode(hasher.finalize()))
        }
        "SHA-256" | "SHA-256-SESS" | "SHA256" => {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(hex::encode(hasher.finalize()))
        }
        other => Err(HttpxError::UnsupportedProtocol(format!(
            "Unsupported Digest auth algorithm: {other}"
        ))),
    }
}

fn generate_cnonce() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.gen();
    hex::encode(bytes)
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Request;

    #[test]
    fn test_digest_challenge_parse_and_response() {
        let auth = DigestAuth::new("Mufasa", "Circle Of Life");
        let mut req = Request::builder("GET", "http://example.org/dir/index.html")
            .unwrap()
            .build()
            .unwrap();
        let applied = auth
            .handle_challenge(
                r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
                &mut req,
            )
            .unwrap();
        assert!(applied);
        let hdr = req.headers.get("authorization").unwrap();
        assert!(hdr.starts_with("Digest "));
        assert!(hdr.contains("username=\"Mufasa\""));
        assert!(hdr.contains("qop=auth"));
    }
}
