//! The CA bundle a connection names for itself (FR-CON-08, blueprint W11 §7.2).
//!
//! A private CA is the usual reason a verifying mode fails: the platform store does not
//! hold it, and installing it machine-wide is not the user's call (or not wanted). A
//! connection can instead point at a PEM file, and from then on the server certificate is
//! checked against **that bundle and nothing else**, host name included.
//!
//! The bytes are read and validated **once**, here, and the parsed DER travels in
//! [`crate::ConnectionConfig`]. There is no second read at connect time, so a file swapped
//! between validation and use cannot change what is trusted, and whatever hashes the
//! config for the connection pool hashes exactly what will be trusted.
//!
//! What is refused, and why each is by name rather than skipped:
//!
//! - a file that holds a private key: the user pointed at the wrong file, and this program
//!   must not read a key it was not asked for;
//! - a certificate that is not a usable trust anchor: dropping it quietly (what
//!   `RootCertStore::add_parsable_certificates` does) would leave a smaller bundle than
//!   the user thinks they chose;
//! - an empty bundle, or more than [`MAX_CA_CERTIFICATES`]: both are a wrong file.
//!
//! Nothing here verifies a server. Whether the mode, the host and the bundle fit together
//! is [`crate::ConnectionConfig::ca_for_verifying_mode`]; the handshake is the driver's.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::CertificateDer;

/// The largest CA file read. A real bundle is a few kilobytes; a megabyte is a file
/// that is not one, and reading it all would be reading the wrong thing.
pub const MAX_CA_FILE_BYTES: u64 = 1024 * 1024;

/// The most certificates one bundle may hold.
pub const MAX_CA_CERTIFICATES: usize = 64;

/// A validated CA bundle: where it came from and the certificates in it.
///
/// The path is identity (two connections naming different files are different
/// connections), the DER is what is trusted, and neither is secret. `Debug` prints the
/// path and a count, never the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct TlsCa {
    path: PathBuf,
    certificates: Arc<Vec<Vec<u8>>>,
}

/// Why a CA file was refused. Every message names the file and says what to change; none
/// repeats the file's contents.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaError {
    #[error("cannot read the CA file {path}: {reason}")]
    Unreadable { path: String, reason: String },

    #[error(
        "the CA file {path} is larger than {MAX_CA_FILE_BYTES} bytes, which is not a CA bundle"
    )]
    TooLarge { path: String },

    #[error("the CA file {path} is not valid PEM: {reason}")]
    NotPem { path: String, reason: String },

    #[error(
        "the CA file {path} contains a private key; point it at the CA certificate, \
         never at a key file"
    )]
    HoldsPrivateKey { path: String },

    #[error(
        "the CA file {path} contains no PEM certificate (a DER file has to be converted \
         first, for example with `openssl x509 -inform der`)"
    )]
    NoCertificates { path: String },

    #[error(
        "the CA file {path} holds {found} certificates; at most {MAX_CA_CERTIFICATES} are accepted"
    )]
    TooMany { path: String, found: usize },

    #[error(
        "certificate {number} in the CA file {path} cannot be used as a trust anchor: {reason}"
    )]
    BadCertificate {
        path: String,
        number: usize,
        reason: String,
    },
}

impl TlsCa {
    /// Read and validate the file at `path`.
    ///
    /// `path` is used as given: expanding `~` is the caller's, because it is the caller
    /// that knows the environment.
    pub fn load(path: &Path) -> Result<Self, CaError> {
        let shown = path.display().to_string();
        let unreadable = |error: std::io::Error| CaError::Unreadable {
            path: shown.clone(),
            reason: error.to_string(),
        };
        let file = std::fs::File::open(path).map_err(unreadable)?;
        let mut bytes = Vec::new();
        // One byte past the limit is enough to know the file is over it.
        file.take(MAX_CA_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() as u64 > MAX_CA_FILE_BYTES {
            return Err(CaError::TooLarge { path: shown });
        }
        Self::from_pem(path, &bytes)
    }

    /// Validate PEM bytes that were read from `path`.
    pub fn from_pem(path: impl Into<PathBuf>, pem: &[u8]) -> Result<Self, CaError> {
        let path = path.into();
        let shown = path.display().to_string();

        // Every private-key label ends in `PRIVATE KEY-----`, encrypted ones included, and
        // the PEM reader below skips labels it does not know, so this has to be a text
        // check and not a section kind.
        if pem
            .windows(b"PRIVATE KEY-----".len())
            .any(|window| window == b"PRIVATE KEY-----")
        {
            return Err(CaError::HoldsPrivateKey { path: shown });
        }

        let mut certificates = Vec::new();
        for certificate in CertificateDer::pem_slice_iter(pem) {
            let certificate = certificate.map_err(|error| CaError::NotPem {
                path: shown.clone(),
                reason: error.to_string(),
            })?;
            if certificates.len() == MAX_CA_CERTIFICATES {
                // Counted past the limit so the message says how far over the file is.
                let found = MAX_CA_CERTIFICATES
                    + 1
                    + CertificateDer::pem_slice_iter(pem)
                        .skip(MAX_CA_CERTIFICATES + 1)
                        .count();
                return Err(CaError::TooMany { path: shown, found });
            }
            certificates.push(certificate);
        }
        if certificates.is_empty() {
            return Err(CaError::NoCertificates { path: shown });
        }
        for (index, certificate) in certificates.iter().enumerate() {
            webpki::anchor_from_trusted_cert(certificate).map_err(|error| {
                CaError::BadCertificate {
                    path: shown.clone(),
                    number: index + 1,
                    reason: error.to_string(),
                }
            })?;
        }

        Ok(Self {
            path,
            certificates: Arc::new(certificates.into_iter().map(|der| der.to_vec()).collect()),
        })
    }

    /// The file this bundle was read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The DER of every certificate, in file order. This is what a driver puts in its root
    /// store, and what the pool key hashes.
    pub fn der(&self) -> &[Vec<u8>] {
        &self.certificates
    }
}

impl fmt::Debug for TlsCa {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "TlsCa({}, {} certificate{})",
            self.path.display(),
            self.certificates.len(),
            if self.certificates.len() == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};

    /// A fresh self-signed CA, as PEM.
    fn ca_pem(name: &str) -> String {
        let key = KeyPair::generate().expect("a key pair");
        let mut params = CertificateParams::new(Vec::new()).expect("parameters");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        CertifiedIssuer::self_signed(params, key)
            .expect("a CA")
            .pem()
    }

    fn load(pem: &str) -> Result<TlsCa, CaError> {
        TlsCa::from_pem("/tmp/ca.pem", pem.as_bytes())
    }

    #[test]
    fn a_bundle_keeps_every_certificate_in_file_order() {
        let first = ca_pem("first");
        let second = ca_pem("second");
        let ca = load(&format!("{first}\n{second}")).expect("a valid bundle");
        assert_eq!(ca.der().len(), 2);
        assert_eq!(ca.path(), Path::new("/tmp/ca.pem"));
        let expected: Vec<Vec<u8>> = [&first, &second]
            .iter()
            .map(|pem| {
                CertificateDer::from_pem_slice(pem.as_bytes())
                    .expect("one certificate")
                    .to_vec()
            })
            .collect();
        assert_eq!(ca.der(), expected.as_slice());
    }

    #[test]
    fn text_around_the_certificates_is_ignored() {
        // `openssl x509 -text` and hand-edited bundles carry prose between the blocks.
        let pem = format!(
            "# my corporate CA\nSubject: whatever\n{}\ntrailing\n",
            ca_pem("a")
        );
        assert_eq!(load(&pem).expect("valid").der().len(), 1);
    }

    #[test]
    fn a_file_with_a_private_key_is_refused_before_anything_is_parsed() {
        for label in [
            "PRIVATE KEY",
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
        ] {
            let pem = format!(
                "{}\n-----BEGIN {label}-----\nAAAA\n-----END {label}-----\n",
                ca_pem("a")
            );
            assert!(
                matches!(load(&pem), Err(CaError::HoldsPrivateKey { .. })),
                "{label}"
            );
        }
    }

    #[test]
    fn nothing_that_is_not_a_certificate_makes_a_bundle() {
        for text in [
            "",
            "not pem at all\n",
            "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n",
        ] {
            assert!(
                matches!(load(text), Err(CaError::NoCertificates { .. })),
                "{text:?}"
            );
        }
        // Valid framing, broken base64.
        let broken = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        assert!(matches!(load(broken), Err(CaError::NotPem { .. })));
    }

    #[test]
    fn a_certificate_that_is_not_x509_is_refused_by_number_not_dropped() {
        // Well-formed PEM whose payload is not a certificate: a store that skipped it
        // would leave a bundle one smaller than the user believes they chose.
        let pem = format!(
            "{}\n-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
            ca_pem("good")
        );
        match load(&pem) {
            Err(CaError::BadCertificate { number, .. }) => assert_eq!(number, 2),
            other => panic!("expected a bad certificate, got {other:?}"),
        }
    }

    #[test]
    fn the_sixty_fifth_certificate_is_one_too_many() {
        let one = ca_pem("c");
        let pem = std::iter::repeat_n(one.as_str(), MAX_CA_CERTIFICATES + 1)
            .collect::<Vec<_>>()
            .join("\n");
        match load(&pem) {
            Err(CaError::TooMany { found, .. }) => assert_eq!(found, MAX_CA_CERTIFICATES + 1),
            other => panic!("expected too many, got {other:?}"),
        }
        let at_the_limit = std::iter::repeat_n(one.as_str(), MAX_CA_CERTIFICATES)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(load(&at_the_limit).expect("64 is fine").der().len(), 64);
    }

    #[test]
    fn a_file_is_read_from_disk_and_a_missing_or_huge_one_is_named() {
        let dir = std::env::temp_dir().join(format!("qh-ca-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");

        let good = dir.join("ca.pem");
        std::fs::write(&good, ca_pem("disk")).expect("write");
        assert_eq!(TlsCa::load(&good).expect("loads").der().len(), 1);

        let missing = dir.join("absent.pem");
        match TlsCa::load(&missing) {
            Err(CaError::Unreadable { path, .. }) => {
                assert!(path.ends_with("absent.pem"), "{path}")
            }
            other => panic!("expected unreadable, got {other:?}"),
        }

        let huge = dir.join("huge.pem");
        std::fs::write(&huge, vec![b'#'; (MAX_CA_FILE_BYTES + 1) as usize]).expect("write");
        assert!(matches!(TlsCa::load(&huge), Err(CaError::TooLarge { .. })));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bundle_prints_its_path_and_a_count_never_its_bytes() {
        let ca = load(&ca_pem("printed")).expect("valid");
        let shown = format!("{ca:?}");
        assert_eq!(shown, "TlsCa(/tmp/ca.pem, 1 certificate)");
    }

    #[test]
    fn an_error_names_the_file_and_never_quotes_it() {
        let secret =
            "-----BEGIN RSA PRIVATE KEY-----\nTOPSECRETMATERIAL\n-----END RSA PRIVATE KEY-----\n";
        let message = load(secret).expect_err("refused").to_string();
        assert!(message.contains("/tmp/ca.pem"), "{message}");
        assert!(!message.contains("TOPSECRETMATERIAL"), "{message}");
    }
}
