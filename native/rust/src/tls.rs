//! TLS client configuration (DECISIONS.md D35, D45).
//!
//! Certificates are checked against the operating system's trust store, as
//! `dart:io` does:
//!
//! * Apple and Windows: the OS verifier (`rustls-platform-verifier`:
//!   Security.framework, CryptoAPI), including enterprise, MDM and
//!   user-trusted CAs and the user's trust settings;
//! * Linux: the system CA files (`rustls-native-certs`; honours
//!   `SSL_CERT_FILE` / `SSL_CERT_DIR`);
//! * Android: the system CA directory (the Conscrypt APEX on Android 14+,
//!   `/system/etc/security/cacerts` before), minus the system CAs the user
//!   disabled; plus, with `user_roots`, the CAs the user or an MDM
//!   installed. That is what an app gets by default and with
//!   `<certificates src="user"/>` in `network_security_config`, which does
//!   not reach native code. `rustls-platform-verifier` would need a JavaVM
//!   and an app `Context`, which a Dart FFI library does not get.
//!
//! If no system root can be loaded (a minimal container without
//! `ca-certificates`), the bundled Mozilla roots (`webpki-roots`) are used.
//! `trustedRootsPem` adds roots on top of the system ones.
//!
//! Pins (SHA-256 of the server certificate's SubjectPublicKeyInfo) are
//! checked after the chain, also with `allowInsecure`.
//!
//! Configurations are built once per distinct setting and shared, so TLS
//! sessions are resumed across reconnects. A session is only resumed under
//! the configuration that verified it: a pinned connection never resumes a
//! session whose certificate was not checked against its pins.

use std::sync::{Arc, Mutex, OnceLock};

use tokio_rustls::rustls::{
    self, CertificateError, ClientConfig, DigitallySignedStruct, OtherError, SignatureScheme,
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

/// TLS settings of a connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// `allowInsecure`: no chain check (pins still apply).
    pub allow_insecure: bool,
    /// `trustedRootsPem`, added to the system roots.
    pub roots_pem: Option<String>,
    /// SHA-256 digests of accepted SubjectPublicKeyInfos; empty: no pinning.
    pub pins: Vec<[u8; 32]>,
    /// Android: also trust the user-installed CAs.
    pub user_roots: bool,
}

impl Settings {
    /// Equal settings share one configuration (and its session cache).
    fn normalized(mut self) -> Self {
        if self.roots_pem.as_deref() == Some("") {
            self.roots_pem = None;
        }
        self.pins.sort_unstable();
        self.pins.dedup();
        self.user_roots &= cfg!(target_os = "android");
        self
    }
}

/// Parses a pin as sent by Dart: 64 hex digits.
pub fn parse_pin(hex: &str) -> Result<[u8; 32], BoxError> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return Err(format!("invalid pin {hex:?}: expected 64 hex digits").into());
    }
    let mut out = [0u8; 32];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let s = std::str::from_utf8(pair).map_err(|_| format!("invalid pin {hex:?}"))?;
        out[i] = u8::from_str_radix(s, 16).map_err(|_| format!("invalid pin {hex:?}"))?;
    }
    Ok(out)
}

/// The configuration for `settings`, built once per distinct value.
pub fn client_config(settings: Settings) -> Result<Arc<ClientConfig>, BoxError> {
    /// Apps use one or two settings; the bound only stops a pathological
    /// caller (fresh roots on every connect) from growing it forever.
    const CACHED: usize = 16;
    static CACHE: Mutex<Vec<(Settings, Arc<ClientConfig>)>> = Mutex::new(Vec::new());

    let settings = settings.normalized();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, config)) = cache.iter().find(|(s, _)| *s == settings) {
        return Ok(config.clone());
    }
    let config = Arc::new(build(&settings)?);
    if cache.len() == CACHED {
        cache.remove(0);
    }
    cache.push((settings, config.clone()));
    Ok(config)
}

fn build(settings: &Settings) -> Result<ClientConfig, BoxError> {
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier(settings)?)
        .with_no_client_auth())
}

fn verifier(settings: &Settings) -> Result<Arc<dyn ServerCertVerifier>, BoxError> {
    let provider = provider();
    let mut verifier: Arc<dyn ServerCertVerifier> = if settings.allow_insecure {
        Arc::new(AcceptAnyCertificate(provider.clone()))
    } else {
        let extra = match settings.roots_pem.as_deref() {
            None => Vec::new(),
            Some(pem) => {
                let extra = parse_pem(pem)?;
                if extra.is_empty() {
                    return Err("trustedRootsPem contains no certificate".into());
                }
                extra
            }
        };
        system_verifier(extra, settings.user_roots, provider.clone())?
    };
    if !settings.pins.is_empty() {
        verifier = Arc::new(Pinned {
            inner: verifier,
            pins: settings.pins.clone(),
        });
    }
    Ok(verifier)
}

fn parse_pem(pem: &str) -> Result<Vec<CertificateDer<'static>>, BoxError> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .map(|c| c.map_err(Into::into))
        .collect()
}

#[cfg(any(target_vendor = "apple", windows))]
fn system_verifier(
    extra: Vec<CertificateDer<'static>>,
    _user_roots: bool,
    provider: Arc<CryptoProvider>,
) -> Result<Arc<dyn ServerCertVerifier>, BoxError> {
    let verifier = if extra.is_empty() {
        rustls_platform_verifier::Verifier::new(provider)?
    } else {
        rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider)?
    };
    Ok(Arc::new(OffloadedVerifier(verifier)))
}

#[cfg(not(any(target_vendor = "apple", windows)))]
fn system_verifier(
    extra: Vec<CertificateDer<'static>>,
    user_roots: bool,
    provider: Arc<CryptoProvider>,
) -> Result<Arc<dyn ServerCertVerifier>, BoxError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(system_roots().iter().cloned());
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    #[cfg(target_os = "android")]
    if user_roots {
        roots.add_parsable_certificates(android::user_roots());
    }
    #[cfg(not(target_os = "android"))]
    let _ = user_roots;
    for cert in extra {
        roots.add(cert)?;
    }
    Ok(
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()?,
    )
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
    android::system_roots()
}

/// Android trust store files, as Conscrypt's `TrustedCertificateStore`
/// reads them. Paths take a base so that tests can run on any host.
#[cfg(any(test, target_os = "android"))]
mod android {
    use std::path::Path;

    use super::{CertificateDer, PemObject};

    /// The updatable Conscrypt APEX (Android 14+) first, then the system
    /// image.
    #[cfg(target_os = "android")]
    const SYSTEM: [&str; 2] = [
        "/apex/com.android.conscrypt/cacerts",
        "/system/etc/security/cacerts",
    ];

    #[cfg(target_os = "android")]
    pub fn system_roots() -> Vec<CertificateDer<'static>> {
        system_roots_in(&SYSTEM.map(Path::new), &user_dir())
    }

    #[cfg(target_os = "android")]
    pub fn user_roots() -> Vec<CertificateDer<'static>> {
        read_dir(&user_dir().join("cacerts-added"))
    }

    /// `/data/misc/user/<user id>`: per Android user (work profile,
    /// secondary users). App processes may read it (group `everybody`).
    #[cfg(target_os = "android")]
    fn user_dir() -> std::path::PathBuf {
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { getuid() };
        format!("/data/misc/user/{}", uid / 100_000).into()
    }

    /// The first non-empty system directory, minus the certificates the
    /// user disabled (`cacerts-removed` holds copies of them).
    pub fn system_roots_in(dirs: &[&Path], user: &Path) -> Vec<CertificateDer<'static>> {
        let removed = read_dir(&user.join("cacerts-removed"));
        for dir in dirs {
            let mut certs = read_dir(dir);
            if !certs.is_empty() {
                certs.retain(|c| !removed.contains(c));
                return certs;
            }
        }
        Vec::new()
    }

    /// Every certificate in `dir`. System files are PEM (with a text dump
    /// before it); the user store holds DER. Unreadable entries are
    /// skipped: a missing directory means no such certificates.
    pub fn read_dir(dir: &Path) -> Vec<CertificateDer<'static>> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut certs = Vec::new();
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if bytes.first() == Some(&0x30) {
                certs.push(CertificateDer::from(bytes));
            } else {
                certs.extend(CertificateDer::pem_slice_iter(&bytes).flatten());
            }
        }
        certs
    }
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

/// The server's key matched none of the pins.
#[derive(Debug)]
pub struct PinMismatch;

impl std::fmt::Display for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server certificate's public key matches none of the pins")
    }
}

impl std::error::Error for PinMismatch {}

/// Checks the chain with `inner`, then the leaf's key against the pins.
/// Only the leaf: `dart:io` exposes no other certificate, and both
/// transports must accept the same servers.
#[derive(Debug)]
struct Pinned {
    inner: Arc<dyn ServerCertVerifier>,
    pins: Vec<[u8; 32]>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verified = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        let digest = spki(end_entity).map(|k| ring::digest::digest(&ring::digest::SHA256, k));
        match digest {
            Some(d) if self.pins.iter().any(|p| p[..] == *d.as_ref()) => Ok(verified),
            _ => Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::new(PinMismatch)),
            ))),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The SubjectPublicKeyInfo of a DER certificate, tag and length included
/// (what `openssl x509 -pubkey | openssl pkey -pubin -outform der`
/// prints). The Dart transport has the same walk
/// (`lib/src/transport/dart/certificate.dart`).
pub fn spki(cert: &[u8]) -> Option<&[u8]> {
    /// One DER element at the start of `d`: (tag, whole element, rest).
    fn element(d: &[u8]) -> Option<(u8, &[u8], &[u8], usize)> {
        let tag = *d.first()?;
        let first = *d.get(1)?;
        let (header, len) = if first < 0x80 {
            (2, first as usize)
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *d.get(2 + i)? as usize;
            }
            (2 + n, len)
        };
        let end = header.checked_add(len)?;
        if d.len() < end {
            return None;
        }
        Some((tag, &d[..end], &d[end..], header))
    }
    const SEQUENCE: u8 = 0x30;
    let (tag, certificate, _, header) = element(cert)?;
    if tag != SEQUENCE {
        return None;
    }
    let (tag, tbs, _, header2) = element(&certificate[header..])?;
    if tag != SEQUENCE {
        return None;
    }
    let mut rest = &tbs[header2..];
    // version [0] EXPLICIT, optional.
    if rest.first() == Some(&0xa0) {
        rest = element(rest)?.2;
    }
    // serialNumber, signature, issuer, validity, subject.
    for _ in 0..5 {
        rest = element(rest)?.2;
    }
    let (tag, key, _, _) = element(rest)?;
    (tag == SEQUENCE).then_some(key)
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
    use std::path::Path;

    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, PublicKeyData};

    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    /// Tests that look at the shared cache must not run while another one
    /// fills it.
    fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn equal_settings_share_a_config() {
        let _lock = cache_lock();
        let a = client_config(settings()).unwrap();
        let b = client_config(Settings {
            roots_pem: Some(String::new()),
            ..settings()
        })
        .unwrap();
        assert!(Arc::ptr_eq(&a, &b), "sessions resume across reconnects");
        let pins = |p: Vec<[u8; 32]>| Settings {
            pins: p,
            ..settings()
        };
        let c = client_config(pins(vec![[1; 32], [2; 32]])).unwrap();
        let d = client_config(pins(vec![[2; 32], [1; 32], [2; 32]])).unwrap();
        assert!(
            Arc::ptr_eq(&c, &d),
            "pin order and duplicates do not matter"
        );
        assert!(!Arc::ptr_eq(&a, &c), "a pinned session is never shared");
    }

    #[test]
    fn rejects_pem_without_certificates() {
        let e = client_config(Settings {
            roots_pem: Some("not a pem".into()),
            ..settings()
        })
        .unwrap_err();
        assert!(e.to_string().contains("no certificate"), "{e}");
    }

    #[test]
    fn rejects_broken_pem() {
        let pem = "-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n";
        assert!(
            client_config(Settings {
                roots_pem: Some(pem.into()),
                ..settings()
            })
            .is_err()
        );
    }

    #[test]
    fn insecure_has_its_own_config() {
        let a = client_config(Settings {
            allow_insecure: true,
            ..settings()
        })
        .unwrap();
        let b = client_config(settings()).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn the_cache_is_bounded() {
        let _lock = cache_lock();
        let first = client_config(Settings {
            pins: vec![[200; 32]],
            ..settings()
        })
        .unwrap();
        for i in 0..20u8 {
            client_config(Settings {
                pins: vec![[i; 32]],
                ..settings()
            })
            .unwrap();
        }
        let again = client_config(Settings {
            pins: vec![[200; 32]],
            ..settings()
        })
        .unwrap();
        assert!(!Arc::ptr_eq(&first, &again), "evicted and rebuilt");
    }

    #[test]
    fn parses_pins() {
        let hex = "00ff".repeat(16);
        let pin = parse_pin(&hex).unwrap();
        assert_eq!(pin[0], 0);
        assert_eq!(pin[1], 255);
        for bad in ["", "00", &"zz".repeat(32), &"0".repeat(65), &"é".repeat(32)] {
            assert!(parse_pin(bad).is_err(), "{bad:?}");
        }
    }

    struct Pki {
        ca: CertificateDer<'static>,
        leaf: CertificateDer<'static>,
        /// SPKI DER as rcgen encodes it: an independent implementation.
        leaf_spki: Vec<u8>,
    }

    fn pki(ca_name: &str) -> Pki {
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.distinguished_name
            .push(rcgen::DnType::CommonName, ca_name);
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca, ca_key);
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["localhost".to_owned()])
            .unwrap()
            .signed_by(&leaf_key, &issuer)
            .unwrap();
        Pki {
            ca: ca_cert.der().clone(),
            leaf: leaf.der().clone(),
            leaf_spki: leaf_key.subject_public_key_info(),
        }
    }

    fn sha256(data: &[u8]) -> [u8; 32] {
        ring::digest::digest(&ring::digest::SHA256, data)
            .as_ref()
            .try_into()
            .unwrap()
    }

    #[test]
    fn spki_matches_rcgen() {
        let pki = pki("spki");
        assert_eq!(spki(&pki.leaf).unwrap(), &pki.leaf_spki[..]);
        assert!(spki(&pki.ca).is_some());
        assert_eq!(spki(&[]), None);
        assert_eq!(spki(&pki.leaf[..pki.leaf.len() - 1]), None, "truncated");
        assert_eq!(spki(&[0x31, 0x00]), None, "not a SEQUENCE");
        assert_eq!(spki(&[0x30, 0x85, 0, 0, 0, 0, 0]), None, "length > 4 bytes");
    }

    /// Same fixture as `test/transport/certificate_test.dart`; the pin is
    /// OpenSSL's.
    const EC_FIXTURE: &str = "-----BEGIN CERTIFICATE-----\nMIIBxDCCAUqgAwIBAgIUS4qhSpo//2srBawTAEovhx3gJ98wCgYIKoZIzj0EAwIw\nGTEXMBUGA1UEAwwOcGluIGZpeHR1cmUgZWMwHhcNMjYxMTEyMTgyMTM4WhcNMzYx\nMTA5MTgyMTM4WjAZMRcwFQYDVQQDDA5waW4gZml4dHVyZSBlYzB2MBAGByqGSM49\nAgEGBSuBBAAiA2IABNTLfMSMPOh3gILIDKYXn92GsAlRxmkelcnQ9+kNhAGRVUEt\nzx7RfXy7iTKNy7j4wBKcCFoTL86ve9fY3esG+80Gvg18KV5eE0KjOzRw01HQHKKg\nNmbMyEJdyiQY0p7xTaNTMFEwHQYDVR0OBBYEFE7G1XIpwvlMyeGUo5u9kkogVgr5\nMB8GA1UdIwQYMBaAFE7G1XIpwvlMyeGUo5u9kkogVgr5MA8GA1UdEwEB/wQFMAMB\nAf8wCgYIKoZIzj0EAwIDaAAwZQIxAJH/+zyIFCkYp9r1V0kdc71Jc4llaWPz7U/I\nKyDziNS2KmENJT20dqTs8GRXaT0kPQIwUhFfLxd2Ug4O3fASQigfD0EXFKi+I287\n4abGmh/fjf6Wk15xwImPn78iODZrbR3e\n-----END CERTIFICATE-----";
    const EC_PIN: &str = "89fdd4ec98b0977ff632119102dfc968525a6cef0afab1a78e643f9f2e03dcbe";

    #[test]
    fn spki_matches_openssl_also_without_version() {
        let der = parse_pem(EC_FIXTURE).unwrap().remove(0);
        let key = spki(&der).unwrap();
        assert_eq!(sha256(key), parse_pin(EC_PIN).unwrap());

        // The same certificate as X.509 v1: no `[0] version` element. The
        // lengths here are short or two-byte forms.
        fn element(tag: u8, content: &[u8]) -> Vec<u8> {
            let mut out = vec![tag];
            match content.len() {
                n if n < 0x80 => out.push(n as u8),
                n if n < 0x100 => out.extend([0x81, n as u8]),
                n => out.extend([0x82, (n >> 8) as u8, n as u8]),
            }
            out.extend_from_slice(content);
            out
        }
        fn content(e: &[u8]) -> &[u8] {
            &e[if e[1] < 0x80 {
                2
            } else {
                2 + (e[1] & 0x7f) as usize
            }..]
        }
        let cert = content(&der);
        // TBSCertificate is the first element of `cert`.
        let header = if cert[1] < 0x80 {
            2
        } else {
            2 + (cert[1] & 0x7f) as usize
        };
        let tbs_content_len = match cert[1] {
            n if n < 0x80 => n as usize,
            0x81 => cert[2] as usize,
            _ => (cert[2] as usize) << 8 | cert[3] as usize,
        };
        let tbs = &cert[..header + tbs_content_len];
        let rest = &cert[tbs.len()..];
        let fields = content(tbs);
        assert_eq!(fields[0], 0xa0, "the fixture is v3");
        let version_len = 2 + fields[1] as usize;
        let v1 = element(
            0x30,
            &[element(0x30, &fields[version_len..]), rest.to_vec()].concat(),
        );
        assert_eq!(spki(&v1), Some(key));
    }

    fn verify(
        verifier: &Arc<dyn ServerCertVerifier>,
        pki: &Pki,
    ) -> Result<ServerCertVerified, rustls::Error> {
        verifier.verify_server_cert(
            &pki.leaf,
            &[],
            &ServerName::try_from("localhost").unwrap(),
            &[],
            UnixTime::now(),
        )
    }

    fn pem(der: &CertificateDer<'_>) -> String {
        use std::fmt::Write;
        const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
        for chunk in der.chunks(3) {
            let n = chunk.iter().fold(0u32, |a, b| a << 8 | *b as u32) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        writeln!(out, "\n-----END CERTIFICATE-----").unwrap();
        out
    }

    fn is_pin_mismatch(e: &rustls::Error) -> bool {
        matches!(e, rustls::Error::InvalidCertificate(CertificateError::Other(o))
            if o.0.downcast_ref::<PinMismatch>().is_some())
    }

    #[test]
    fn pins_are_checked_after_the_chain() {
        let pki = pki("pins");
        let roots = Some(pem(&pki.ca));
        let pinned = |pins: Vec<[u8; 32]>, allow_insecure: bool| {
            verifier(&Settings {
                allow_insecure,
                roots_pem: if allow_insecure { None } else { roots.clone() },
                pins,
                user_roots: false,
            })
            .unwrap()
        };
        let good = sha256(&pki.leaf_spki);

        assert!(verify(&pinned(vec![], false), &pki).is_ok());
        assert!(verify(&pinned(vec![[0; 32], good], false), &pki).is_ok());
        let e = verify(&pinned(vec![[0; 32]], false), &pki).unwrap_err();
        assert!(is_pin_mismatch(&e), "{e:?}");
        // The chain first: an untrusted chain is reported as such.
        let other = self::pki("other");
        let e = verify(&pinned(vec![sha256(&other.leaf_spki)], false), &other).unwrap_err();
        assert!(!is_pin_mismatch(&e), "{e:?}");
        // allowInsecure skips the chain, never the pins.
        assert!(verify(&pinned(vec![sha256(&other.leaf_spki)], true), &other).is_ok());
        let e = verify(&pinned(vec![good], true), &other).unwrap_err();
        assert!(is_pin_mismatch(&e), "{e:?}");
    }

    #[test]
    fn android_store_layout() {
        let dir = std::env::temp_dir().join(format!("ws_tls_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let apex = dir.join("apex");
        let system = dir.join("system");
        let user = dir.join("user");
        for d in [
            &apex,
            &system,
            &user.join("cacerts-added"),
            &user.join("cacerts-removed"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let a = pki("a");
        let b = pki("b");
        let c = pki("c");
        // System files: a text dump, then PEM (as on the device).
        let write_pem = |d: &Path, name: &str, der: &CertificateDer<'_>| {
            std::fs::write(
                d.join(name),
                format!("Certificate:\n  Data: ...\n{}", pem(der)),
            )
            .unwrap()
        };
        write_pem(&system, "old.0", &c.ca);
        write_pem(&apex, "aaaa.0", &a.ca);
        write_pem(&apex, "bbbb.0", &b.ca);
        std::fs::write(apex.join("junk.0"), b"not a certificate").unwrap();
        // The user store holds DER.
        std::fs::write(user.join("cacerts-removed/bbbb.0"), b.ca.as_ref()).unwrap();
        std::fs::write(user.join("cacerts-added/cccc.0"), c.ca.as_ref()).unwrap();

        let roots = android::system_roots_in(&[&apex, &system], &user);
        assert_eq!(roots, vec![a.ca.clone()], "APEX first, disabled CA removed");
        let roots = android::system_roots_in(&[&dir.join("missing"), &system], &user);
        assert_eq!(roots, vec![c.ca.clone()], "falls back to the system image");
        assert_eq!(
            android::read_dir(&user.join("cacerts-added")),
            vec![c.ca.clone()]
        );
        assert!(android::read_dir(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
