//! TLS client configuration (DECISIONS.md D35).
//!
//! Certificates are checked against the operating system's trust store, as
//! `dart:io` does. On Apple and Windows that includes enterprise, MDM and
//! user-trusted CAs; on Android only the system CAs (user and MDM CAs live
//! in the user store, which apps ignore by default and
//! `network_security_config` does not reach native code):
//!
//! * Apple and Windows: the OS verifier (`rustls-platform-verifier`:
//!   Security.framework, CryptoAPI), including the user's trust settings;
//! * Linux: the system CA files (`rustls-native-certs`; honours
//!   `SSL_CERT_FILE` / `SSL_CERT_DIR`);
//! * Android: the system CA directory (the Conscrypt APEX on Android 14+,
//!   `/system/etc/security/cacerts` before). `rustls-platform-verifier`
//!   would need a JavaVM and an app `Context`, which a Dart FFI library does
//!   not get.
//!
//! If no system root can be loaded (a minimal container without
//! `ca-certificates`), the bundled Mozilla roots (`webpki-roots`) are used.
//! `trustedRootsPem` adds roots on top of the system ones.
//!
//! The default configuration is built once per process and shared, so TLS
//! sessions are resumed across reconnects.

use std::sync::{Arc, OnceLock};

use tokio_rustls::rustls::{
    self, ClientConfig, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The configuration for a connection.
pub fn client_config(
    allow_insecure: bool,
    roots_pem: Option<&str>,
) -> Result<Arc<ClientConfig>, BoxError> {
    if allow_insecure {
        let provider = provider();
        let config = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
            .with_no_client_auth();
        return Ok(Arc::new(config));
    }
    match roots_pem {
        None | Some("") => {
            static DEFAULT: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
            DEFAULT
                .get_or_init(|| build(Vec::new()).map(Arc::new).map_err(|e| e.to_string()))
                .clone()
                .map_err(Into::into)
        }
        Some(pem) => {
            let extra = parse_pem(pem)?;
            if extra.is_empty() {
                return Err("trustedRootsPem contains no certificate".into());
            }
            Ok(Arc::new(build(extra)?))
        }
    }
}

fn parse_pem(pem: &str) -> Result<Vec<CertificateDer<'static>>, BoxError> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .map(|c| c.map_err(Into::into))
        .collect()
}

#[cfg(any(target_vendor = "apple", windows))]
fn build(extra: Vec<CertificateDer<'static>>) -> Result<ClientConfig, BoxError> {
    let provider = provider();
    let verifier = if extra.is_empty() {
        rustls_platform_verifier::Verifier::new(provider.clone())?
    } else {
        rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider.clone())?
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(OffloadedVerifier(verifier)))
        .with_no_client_auth())
}

#[cfg(not(any(target_vendor = "apple", windows)))]
fn build(extra: Vec<CertificateDer<'static>>) -> Result<ClientConfig, BoxError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(system_roots().iter().cloned());
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    for cert in extra {
        roots.add(cert)?;
    }
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// System roots, read once per process.
#[cfg(not(any(target_vendor = "apple", windows)))]
fn system_roots() -> &'static [CertificateDer<'static>] {
    static ROOTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    ROOTS.get_or_init(load_system_roots)
}

#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
fn load_system_roots() -> Vec<CertificateDer<'static>> {
    rustls_native_certs::load_native_certs().certs
}

#[cfg(target_os = "android")]
fn load_system_roots() -> Vec<CertificateDer<'static>> {
    const DIRS: [&str; 2] = [
        "/apex/com.android.conscrypt/cacerts",
        "/system/etc/security/cacerts",
    ];
    for dir in DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut certs = Vec::new();
        for entry in entries.flatten() {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                certs.extend(CertificateDer::pem_slice_iter(&bytes).flatten());
            }
        }
        if !certs.is_empty() {
            return certs;
        }
    }
    Vec::new()
}

#[cfg(not(any(unix, windows)))]
fn load_system_roots() -> Vec<CertificateDer<'static>> {
    Vec::new()
}

/// Runs the OS verifier with `block_in_place`: Security.framework and
/// CryptoAPI may block (revocation and intermediate fetching), and the
/// runtime has only a few worker threads.
#[cfg(any(target_vendor = "apple", windows))]
#[derive(Debug)]
struct OffloadedVerifier(rustls_platform_verifier::Verifier);

#[cfg(any(target_vendor = "apple", windows))]
impl ServerCertVerifier for OffloadedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verify = || {
            self.0
                .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        };
        let multi_thread = tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
        let result = if multi_thread {
            tokio::task::block_in_place(verify)
        } else {
            verify()
        };
        result.map_err(classify)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

/// The Apple verifier maps only some Security.framework codes and reports
/// the rest as text ending in `: <OSStatus>`. Validity errors matter to
/// apps ("check the device clock"), so map them too (DECISIONS.md D42;
/// codes from `SecBase.h`).
#[cfg(any(target_vendor = "apple", windows))]
fn classify(error: rustls::Error) -> rustls::Error {
    #[cfg(target_vendor = "apple")]
    {
        use rustls::CertificateError as C;
        if let rustls::Error::InvalidCertificate(C::Other(other)) = &error {
            let text = other.to_string();
            if text.ends_with(": -67818") {
                return rustls::Error::InvalidCertificate(C::Expired);
            }
            if text.ends_with(": -67819") {
                return rustls::Error::InvalidCertificate(C::NotValidYet);
            }
        }
    }
    error
}

/// `allowInsecure` only (development): accepts any certificate but still
/// checks handshake signatures.
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_shared() {
        let a = client_config(false, None).unwrap();
        let b = client_config(false, Some("")).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "sessions resume across reconnects");
    }

    #[test]
    fn rejects_pem_without_certificates() {
        let e = client_config(false, Some("not a pem")).unwrap_err();
        assert!(e.to_string().contains("no certificate"), "{e}");
    }

    #[test]
    fn rejects_broken_pem() {
        let pem = "-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n";
        assert!(client_config(false, Some(pem)).is_err());
    }

    #[test]
    fn insecure_is_never_shared() {
        let a = client_config(true, None).unwrap();
        let b = client_config(false, None).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }
}
