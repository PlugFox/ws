//! Test PKI generated at startup: a CA and the leaf certificates it signs.
//!
//! * `good`: `localhost`, `127.0.0.1`, `::1` (the `wss` port).
//! * `badhost`: `wrong.invalid` only (the `wss_badhost` port).
//! * `expired`: like `good`, but valid only in 2020 (the `wss_expired` port).
//!
//! Clients trust the CA via `ca_pem` (ready line or `GET /ca.pem`). A
//! self-signed leaf would not do: webpki rejects an end-entity certificate
//! used as its own trust anchor.

use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PublicKeyData,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};

use crate::BoxError;

pub struct Pki {
    /// PEM of the CA certificate.
    pub ca_pem: String,
    /// DER of the CA certificate.
    pub ca_der: Vec<u8>,
    pub good: TlsAcceptor,
    /// SubjectPublicKeyInfo (DER) of the `good` leaf, for pin tests.
    pub good_spki: Vec<u8>,
    pub badhost: TlsAcceptor,
    pub expired: TlsAcceptor,
}

impl Pki {
    pub fn generate() -> Result<Pki, BoxError> {
        let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "ws_bench_server test CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = KeyPair::generate()?;
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let issuer = Issuer::new(ca_params, ca_key);
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (good, good_spki) = leaf(&issuer, names(&["localhost", "127.0.0.1", "::1"]), false)?;
        Ok(Pki {
            ca_pem: ca_cert.pem(),
            ca_der: ca_cert.der().to_vec(),
            good,
            good_spki,
            badhost: leaf(&issuer, names(&["wrong.invalid"]), false)?.0,
            expired: leaf(&issuer, names(&["localhost", "127.0.0.1", "::1"]), true)?.0,
        })
    }
}

/// Issues a server certificate for `names` and builds an acceptor for it;
/// also returns the certificate's SubjectPublicKeyInfo.
/// `expired`: valid from 2020-01-01 to 2021-01-01 only.
fn leaf(
    issuer: &Issuer<'_, KeyPair>,
    names: Vec<String>,
    expired: bool,
) -> Result<(TlsAcceptor, Vec<u8>), BoxError> {
    let mut params = CertificateParams::new(names.clone())?;
    if expired {
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(2021, 1, 1);
    }
    params
        .distinguished_name
        .push(DnType::CommonName, names[0].clone());
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate()?;
    let spki = key.subject_public_key_info();
    let cert = params.signed_by(&key, issuer)?;
    let chain = vec![CertificateDer::from(cert.der().to_vec())];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    Ok((TlsAcceptor::from(Arc::new(config)), spki))
}

/// An acceptor for a PEM chain and key from files (`--tls-cert`,
/// `--tls-key`).
pub fn from_files(cert: &str, key: &str) -> Result<TlsAcceptor, BoxError> {
    use rustls::pki_types::pem::PemObject;
    let chain = CertificateDer::pem_file_iter(cert)?.collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(key)?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}
