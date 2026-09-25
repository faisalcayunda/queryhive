//! TLS for the PostgreSQL driver: one place that turns a [`TlsMode`] into a
//! `rustls` configuration.
//!
//! Why `rustls`, and why the platform verifier
//! -------------------------------------------
//! The blueprint's §5 table (line 580) names `rustls` plus
//! `rustls-platform-verifier`, and the reason is the second half: the platform
//! verifier checks certificates against the operating system's own root store,
//! which on macOS is the Keychain. A corporate CA the user installed there is
//! therefore trusted without shipping a copy of it with the app — which is
//! exactly what a user on a corporate network needs, and what a bundled root
//! list cannot give them.
//!
//! What each mode means here
//! -------------------------
//! - [`TlsMode::Disable`] never negotiates TLS. Handled in [`crate`], not here.
//! - [`TlsMode::Prefer`] encrypts when the server offers it and does not verify
//!   the certificate, which is what psycopg did and what an internal,
//!   self-signed server needs. It differs from `RequireNoVerify` in one thing
//!   only: when the server answers the SSLRequest with "no", it is allowed to
//!   speak in clear, and `RequireNoVerify` is not. A server that says "yes" and
//!   then fails the handshake is an error in **both** modes, because that is the
//!   downgrade an attacker on the network would trigger.
//! - [`TlsMode::Require`] is the verifying mode: the certificate has to chain to
//!   the platform's own root store.
//! - [`TlsMode::RequireNoVerify`] encrypts with no verification at all. It is
//!   reachable only by naming that mode on one connection, and it is what an
//!   attacker on the path needs to read everything — see
//!   [`unverified_client_config`].
//!
//! The seam the tests use
//! ----------------------
//! [`platform_verifier`] is what a real connection uses, and it cannot be
//! exercised in CI: no test can install a CA into the machine's Keychain.
//! [`verifier_with_roots`] is the same machinery with a root store the caller
//! names, so a test can sign a server certificate with a CA it generated and
//! prove both halves — that the configured path trusts what the store holds,
//! and that it refuses what the store does not.

use std::sync::Arc;

use qh_core::{EngineError, FailureKind};
use qh_driver::TlsMode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

/// The verifier a real connection uses: the platform's own root store.
///
/// On macOS that is the Keychain, so a root the user installed — a corporate CA,
/// most often — is trusted here without this program carrying a copy of it. That
/// property is the whole reason `rustls-platform-verifier` was chosen over a
/// bundled root list (blueprint §5, line 580).
pub fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, EngineError> {
    let verifier =
        rustls_platform_verifier::Verifier::new(crypto_provider()).map_err(tls_config_error)?;
    Ok(Arc::new(verifier))
}

/// A verifier that trusts exactly `roots`, and nothing else.
///
/// This is the seam the tests call: a certificate signed by a CA no platform
/// store has ever seen can still be verified, because the caller names the store
/// to verify against. Production passes [`platform_verifier`] instead; nothing
/// in the driver calls this.
pub fn verifier_with_roots(
    roots: RootCertStore,
) -> Result<Arc<dyn ServerCertVerifier>, EngineError> {
    let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), crypto_provider())
        .build()
        .map_err(|error| EngineError::Connect {
            // The one way this fails in practice is an empty root store, which is
            // "trust nothing" — a configuration that must not be mistaken for a
            // verifier that accepts everything.
            message: format!("could not build a TLS verifier from that root store: {error}"),
            kind: FailureKind::Permanent,
        })?;
    Ok(verifier)
}

/// The rustls configuration for the two modes that verify a certificate.
pub(crate) fn verified_client_config(
    verifier: Arc<dyn ServerCertVerifier>,
) -> Result<ClientConfig, EngineError> {
    Ok(ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(tls_config_error)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// The rustls configuration that verifies **nothing**.
///
/// Only [`TlsMode::RequireNoVerify`] selects this, and only for the one
/// connection whose configuration asked for it by name — it is not a fallback
/// any other mode can reach. An attacker who can answer for the server can
/// present any certificate at all here, so everything on the connection is
/// readable by whoever is on the path. The UI is expected to warn in exactly
/// those words before a user turns it on (`TlsMode::RequireNoVerify` carries the
/// same warning).
pub(crate) fn unverified_client_config() -> Result<ClientConfig, EngineError> {
    Ok(ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(tls_config_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TrustsAnything))
        .with_no_client_auth())
}

/// The `MakeTlsConnect` for one already-built configuration.
///
/// `tokio-postgres` reaches TLS only through this trait, which
/// `tokio-postgres-rustls` implements over a `ClientConfig`.
pub(crate) fn connector(config: ClientConfig) -> MakeRustlsConnect {
    MakeRustlsConnect::new(config)
}

/// The `tokio-postgres` spelling of what a mode asks for on the wire.
///
/// The other half of the mode: `Prefer` may continue in clear **only** when the
/// server answers the SSLRequest with "no" (`SslMode::Prefer` in
/// `tokio-postgres`); a failed handshake is an error in both modes, which is the
/// distinction this driver exists to keep.
pub(crate) fn ssl_mode(mode: TlsMode) -> SslMode {
    match mode {
        TlsMode::Disable => SslMode::Disable,
        TlsMode::Prefer => SslMode::Prefer,
        TlsMode::Require | TlsMode::RequireNoVerify => SslMode::Require,
    }
}

/// The crypto `rustls` uses, named explicitly.
///
/// `ClientConfig::builder()` would take the process-wide default provider, which
/// is installed lazily from whichever provider features happen to be enabled —
/// and this workspace already links a second `rustls` user, so which provider is
/// enabled is not this crate's to decide. Naming `ring` here keeps the connector
/// independent of that, and keeps it from panicking when two providers are in the
/// graph.
fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// A handshake failure can never be fixed by trying again, so it must not be
/// handed to the retry loop as a transient condition.
fn tls_config_error(error: rustls::Error) -> EngineError {
    EngineError::Connect {
        message: format!("could not set up TLS: {error}"),
        // A trust store that cannot be built, or an invalid configuration, will
        // not become valid on the next attempt.
        kind: FailureKind::Permanent,
    }
}

/// Accepts every certificate. See [`unverified_client_config`].
#[derive(Debug)]
struct TrustsAnything;

impl ServerCertVerifier for TrustsAnything {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
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
        crypto_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_store_with_nothing_in_it_is_refused_rather_than_trusting_nothing() {
        // Worth pinning: "trust nothing" is not expressible as an empty root
        // store, so the tests that check a refusal name a different CA instead of
        // an empty store. If this ever starts succeeding, every refusal test would
        // silently become a test of nothing.
        assert!(verifier_with_roots(RootCertStore::empty()).is_err());
    }

    #[test]
    fn the_unverified_configuration_can_be_built_without_a_trust_store() {
        // The NoVerify path must not depend on the platform store at all: it is
        // the configuration that ignores certificates.
        assert!(unverified_client_config().is_ok());
    }

    #[test]
    fn the_wire_mode_follows_the_tls_mode() {
        // `Prefer` is the only mode allowed to continue in clear, and only when
        // the server declines TLS; `RequireNoVerify` must still require the TLS
        // negotiation, or it would be a plaintext mode with extra steps.
        assert_eq!(ssl_mode(TlsMode::Disable), SslMode::Disable);
        assert_eq!(ssl_mode(TlsMode::Prefer), SslMode::Prefer);
        assert_eq!(ssl_mode(TlsMode::Require), SslMode::Require);
        assert_eq!(ssl_mode(TlsMode::RequireNoVerify), SslMode::Require);
    }
}
