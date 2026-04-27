//! TLS plumbing for the login server's three TCP ports.
//!
//! LSB/Phoenix login generates a **self-signed cert per server instance** at
//! startup (see `server/src/login/cert_helpers.h`), so we cannot rely on the
//! webpki root store. We use a Trust-On-First-Use verifier: accept any cert
//! the first time, remember the leaf SHA-256 fingerprint, refuse if it later
//! changes (warns the user — the server probably regenerated the cert).

use std::sync::{Arc, Mutex};

use rustls::{
    DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct TofuVerifier {
    pinned: Mutex<Option<[u8; 32]>>,
}

impl TofuVerifier {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pinned: Mutex::new(None),
        })
    }

    /// Last seen leaf cert fingerprint, hex-encoded. Useful for surfacing in
    /// the JSON sidechannel so an agent can see what it pinned.
    pub fn fingerprint_hex(&self) -> Option<String> {
        let g = self.pinned.lock().ok()?;
        g.map(|fp| fp.iter().map(|b| format!("{b:02x}")).collect())
    }
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        let mut g = self.pinned.lock().expect("tofu mutex poisoned");
        match *g {
            None => {
                *g = Some(fp);
                Ok(ServerCertVerified::assertion())
            }
            Some(prev) if prev == fp => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(rustls::Error::General(
                "TOFU pin mismatch — server certificate changed since first connection".into(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        // Match what the server (OpenSSL) typically presents.
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

/// Build a rustls client config that uses our TOFU verifier.
pub fn make_client_config(verifier: Arc<TofuVerifier>) -> Arc<rustls::ClientConfig> {
    // Empty root store; the verifier short-circuits anyway.
    let _ = RootCertStore::empty();
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(config)
}
