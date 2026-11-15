//! TLS configuration, certificate verification, and Rustls connector setup.

use crate::exceptions::{HttpxError, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

/// Certificate verification mode for TLS connections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TlsVerify {
    /// Verify against system root CAs + Mozilla WebPKI roots.
    Default,
    /// Verify against a custom PEM CA bundle file or directory.
    CustomCa(PathBuf),
    /// Disable TLS certificate and hostname verification (insecure).
    Disabled,
}

impl Default for TlsVerify {
    fn default() -> Self {
        Self::Default
    }
}

/// Client-side mTLS certificate and private key paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientCertConfig {
    pub cert_path: PathBuf,
    pub key_path: Option<PathBuf>,
}

/// Complete TLS configuration for a client or transport.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TlsConfig {
    pub verify: TlsVerify,
    pub client_cert: Option<ClientCertConfig>,
    pub http2: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            verify: TlsVerify::Default,
            client_cert: None,
            http2: true,
        }
    }
}

impl TlsConfig {
    /// Builds a [`rustls::ClientConfig`] and wraps it in a [`tokio_rustls::TlsConnector`].
    pub fn build_connector(&self) -> Result<TlsConnector> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| HttpxError::ConnectError(format!("Failed to initialize TLS protocols: {e}")))?;

        let mut config = match &self.verify {
            TlsVerify::Default => {
                let roots = load_default_roots()?;
                let with_roots = builder.with_root_certificates(roots);
                apply_client_cert(with_roots, self.client_cert.as_ref())?
            }
            TlsVerify::CustomCa(path) => {
                let roots = load_custom_ca_roots(path)?;
                let with_roots = builder.with_root_certificates(roots);
                apply_client_cert(with_roots, self.client_cert.as_ref())?
            }
            TlsVerify::Disabled => {
                let dangerous = builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(NoCertificateVerification));
                apply_client_cert(dangerous, self.client_cert.as_ref())?
            }
        };

        if self.http2 {
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        } else {
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
        }

        Ok(TlsConnector::from(Arc::new(config)))
    }

    /// Performs a TLS client handshake over `stream` for `server_host`.
    pub async fn connect<S>(
        connector: &TlsConnector,
        server_host: &str,
        stream: S,
    ) -> Result<(TlsStream<S>, Option<Vec<u8>>)>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let clean_host = server_host.trim_start_matches('[').trim_end_matches(']');
        let server_name = ServerName::try_from(clean_host.to_string()).map_err(|e| {
            HttpxError::InvalidUrl(format!("Invalid TLS server name '{server_host}': {e}"))
        })?;

        let tls_stream = connector
            .connect(server_name, stream)
            .await
            .map_err(|e| HttpxError::ConnectError(format!("TLS handshake failed for '{server_host}': {e}")))?;

        let alpn = tls_stream
            .get_ref()
            .1
            .alpn_protocol()
            .map(|proto| proto.to_vec());

        Ok((tls_stream, alpn))
    }
}

fn load_default_roots() -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = store.add(cert);
    }

    if store.is_empty() {
        return Err(HttpxError::ConnectError(
            "No root CA certificates available for TLS verification".into(),
        ));
    }
    Ok(store)
}

fn load_custom_ca_roots(path: &PathBuf) -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    let mut files = Vec::new();
    if path.is_dir() {
        let entries = std::fs::read_dir(path).map_err(|e| {
            HttpxError::ConnectError(format!(
                "Failed to read CA directory '{}': {e}",
                path.display()
            ))
        })?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                files.push(p);
            }
        }
    } else {
        files.push(path.clone());
    }

    for file_path in files {
        let file = File::open(&file_path).map_err(|e| {
            HttpxError::ConnectError(format!(
                "Failed to open CA certificate file '{}': {e}",
                file_path.display()
            ))
        })?;
        let mut reader = BufReader::new(file);
        for cert_res in rustls_pemfile::certs(&mut reader) {
            let cert = cert_res.map_err(|e| {
                HttpxError::ConnectError(format!(
                    "Failed to parse PEM certificate '{}': {e}",
                    file_path.display()
                ))
            })?;
            store.add(cert).map_err(|e| {
                HttpxError::ConnectError(format!(
                    "Invalid CA certificate in '{}': {e}",
                    file_path.display()
                ))
            })?;
        }
    }

    if store.is_empty() {
        return Err(HttpxError::ConnectError(format!(
            "No valid CA certificates loaded from '{}'",
            path.display()
        )));
    }
    Ok(store)
}

fn apply_client_cert(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>,
    client_cert: Option<&ClientCertConfig>,
) -> Result<ClientConfig> {
    let Some(cc) = client_cert else {
        return Ok(builder.with_no_client_auth());
    };

    let cert_file = File::open(&cc.cert_path).map_err(|e| {
        HttpxError::ConnectError(format!(
            "Failed to open client certificate '{}': {e}",
            cc.cert_path.display()
        ))
    })?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            HttpxError::ConnectError(format!(
                "Failed to parse client certificate '{}': {e}",
                cc.cert_path.display()
            ))
        })?;

    let key_path = cc.key_path.as_ref().unwrap_or(&cc.cert_path);
    let key_file = File::open(key_path).map_err(|e| {
        HttpxError::ConnectError(format!(
            "Failed to open client private key '{}': {e}",
            key_path.display()
        ))
    })?;
    let mut key_reader = BufReader::new(key_file);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| {
            HttpxError::ConnectError(format!(
                "Failed to parse client private key '{}': {e}",
                key_path.display()
            ))
        })?
        .ok_or_else(|| {
            HttpxError::ConnectError(format!(
                "No private key found in '{}'",
                key_path.display()
            ))
        })?;

    builder.with_client_auth_cert(certs, key).map_err(|e| {
        HttpxError::ConnectError(format!("Invalid client TLS certificate/key pair: {e}"))
    })
}

/// Permissive certificate verifier used only when `verify=False` is explicitly configured.
#[derive(Debug)]
struct NoCertificateVerification;

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA1,
            SignatureScheme::ECDSA_SHA1_Legacy,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}
