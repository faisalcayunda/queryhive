//! The TLS decision for one MySQL connection.
//!
//! Kept in its own module, and public, for two reasons: the mapping from a
//! [`TlsMode`] to what the connection actually asks the server for is the part
//! worth pinning in a test, and the limits below are worth reading in one place
//! rather than as a comment beside a call.
//!
//! # What each mode means
//!
//! | [`TlsMode`] | Attempt | Certificate |
//! |---|---|---|
//! | `Disable` | clear from the first packet | — |
//! | `Prefer` | TLS first; plaintext only when the server has no TLS at all | **not checked** |
//! | `Require` | TLS only | checked |
//! | `RequireNoVerify` | TLS only | **not checked** |
//!
//! `Prefer` does not verify, and that is a deliberate parity decision rather than
//! an oversight. The Python engine this replaces handed `sslmode` to pymysql, whose
//! `prefer` (and `require`) encrypt **without** validating the peer; an internal
//! server with a self-signed certificate is the common case in this product's
//! deployments, and a `Prefer` that verified would break every one of them on
//! upgrade with an error that talks about a handshake rather than about
//! verification. `Require` is the mode the blueprint defines as verifying, and it
//! does.
//!
//! What `Prefer` gives up in authentication it keeps in this rule: it falls back to
//! plaintext for exactly one server answer, the handshake packet that does not
//! advertise `CLIENT_SSL`. That happens before a single TLS byte is sent, so the
//! fallback cannot be triggered by anyone who can merely break a handshake — and a
//! handshake that *fails* is an error in every mode. Reading everything off the wire
//! is what an attacker between the two ends wants, and continuing in clear after
//! the encryption failed would hand it to them for free.
//!
//! # Which roots a verified connection is checked against — and the gap here
//!
//! `rustls`, through `mysql_async`. The roots are the **`webpki-roots` bundle
//! compiled into `mysql_async`** (the Mozilla set), plus the name in the
//! certificate checked against the host that was dialled. That is a real
//! verification — a self-signed or privately signed certificate is refused — but it
//! is *not* the platform trust store, so **a corporate CA the user installed in the
//! macOS Keychain is not accepted by [`TlsMode::Require`]**.
//!
//! The blueprint's §5 (line 580) asks for `rustls-platform-verifier` so that it
//! would be. It cannot be wired into `mysql_async` 0.36, and the reason is not a
//! missing flag:
//!
//! 1. `mysql_async` builds the `rustls::ClientConfig` itself —
//!    `SslOpts::build_tls_connector` (`mysql_async-0.36.2/src/io/tls/rustls_io.rs`)
//!    is `pub(crate)`, and `SslOptsAndCachedConnector`
//!    (`src/opts/mod.rs:1151`) is `pub(crate)` too. There is no setter for a
//!    `ServerCertVerifier` or a `RootCertStore`, so neither
//!    `rustls_platform_verifier::Verifier` nor a custom verifier can be supplied.
//! 2. The one method that takes a root store, `SslOpts::with_root_certs`
//!    (`src/opts/mod.rs:237`), takes `PathOrBuf` — and `PathOrBuf`
//!    (`src/opts/mod.rs:145`) is not re-exported from a private `opts` module
//!    (`src/lib.rs:451`), so the type cannot even be named by a caller. Adding
//!    `use mysql_async::opts::PathOrBuf` fails to compile: `error[E0603]: module
//!    `opts` is private`. Passing a CA file is unreachable for the same reason,
//!    which is why there is no connection-scoped CA here to go with
//!    `RequireNoVerify`.
//!
//! `mysql_async` 0.37.1 has the same shape (`mod opts;` at `src/lib.rs:451`,
//! `with_root_certs` taking the same unexported type), so this is not a version
//! that a bump fixes. Two follow-ups would close it, and both are outside this
//! crate: an `ssl-ca` on `ConnectionConfig` (`crates/qh-driver`, deliberately not
//! touched here) *and* an upstream hook — or a local fork — that lets a
//! `ClientConfig` be handed in. Until one of them exists, `Require` means "verified
//! against the public roots", and a private CA is refused rather than approximated:
//! a pre-flight verification in front of a client that then connects without
//! checking would be a time-of-check/time-of-use hole, which is worse than a clear
//! refusal.

use mysql_async::{DriverError, Error as MysqlError, SslOpts};
use qh_driver::TlsMode;

/// The TLS configuration a connection for `mode` is opened with; `None` opens in
/// clear.
///
/// Public so a test can assert what each mode produces instead of trusting the
/// match below by eye. What it returns is the whole of what a caller can set: see
/// the module notes for the root store that is *not* settable.
pub fn ssl_opts(mode: TlsMode) -> Option<SslOpts> {
    match mode {
        // No `SslOpts` means the capability is never offered, so the server's
        // answer cannot turn this into an encrypted connection behind the user's
        // back either.
        TlsMode::Disable => None,
        // `Require` is the one mode that verifies: `SslOpts::default()` is
        // `webpki-roots` as the root store, hostname checked, and neither of the
        // two `danger` switches set.
        TlsMode::Require => Some(SslOpts::default()),
        // Encryption without authentication — `accept_invalid_certs` turns the
        // certificate check off and leaves the hostname unchecked with it. This is
        // what the Python engine's pymysql did for `prefer`, and what `Prefer` is
        // here for: the alternative is an internal server with a self-signed
        // certificate failing to connect at all.
        //
        // `RequireNoVerify` is the same configuration reached a second way, and the
        // one an attacker on the network needs in order to read every row. It is
        // therefore only ever reached from a mode the user selected for this
        // connection, while `Prefer` is the default — which is worth knowing when
        // reading a bug report about a connection that was not authenticated.
        TlsMode::Prefer | TlsMode::RequireNoVerify => {
            Some(SslOpts::default().with_danger_accept_invalid_certs(true))
        }
    }
}

/// Whether a failed attempt may be retried in clear.
///
/// True for `Prefer` and one error only: the server's handshake answered that it
/// has no `CLIENT_SSL` capability. That is the server *saying* it cannot do TLS,
/// known before a single TLS byte is sent, and it is the only thing that separates
/// `Prefer` from `RequireNoVerify` in what it does on the wire.
///
/// Every other failure — a handshake that broke, a socket that closed — is an error
/// the caller sees, in every mode, because those are also what an attacker between
/// the two ends can produce. Turning verification off does not weaken that rule:
/// what it protects is not authenticity but the fact of encryption at all.
pub fn may_fall_back(mode: TlsMode, error: &MysqlError) -> bool {
    mode == TlsMode::Prefer
        && matches!(
            error,
            MysqlError::Driver(DriverError::NoClientSslFlagFromServer)
        )
}

/// The certificate verification failure `error` is, when that is what it is.
///
/// `None` for anything else, including the other ways a TLS handshake fails, and
/// `None` for anything `Prefer` or `RequireNoVerify` can produce, since neither
/// asks for a certificate to be checked. The distinction is worth making because
/// the two want different things from the user: a rejected certificate is settled —
/// trust the CA that signed it, or ask for `RequireNoVerify` — while a protocol
/// failure may be a fluke.
///
/// The chain is walked rather than one shape matched, because `rustls`'s error
/// reaches a caller here in two of them: `mysql_async`'s own `IoError::Tls` for the
/// paths it wraps itself, and — the shape a live server actually produces —
/// `IoError::Io`, because `tokio-rustls` turns a failed handshake into an
/// `std::io::Error` carrying the `rustls::Error` rather than a variant of its own.
/// Matching only the first left a refused certificate reported as a transient
/// I/O error, which a caller would have retried; `tests/integration.rs` caught it.
pub fn certificate_rejection(error: &MysqlError) -> Option<&rustls::CertificateError> {
    let mut link: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = link {
        if let Some(rustls::Error::InvalidCertificate(why)) =
            current.downcast_ref::<rustls::Error>()
        {
            return Some(why);
        }
        link = match current.downcast_ref::<std::io::Error>() {
            // `io::Error::source()` answers with the source of the error it was
            // built around, never that error itself (`std`'s `Repr::Custom`
            // implementation), so the payload has to be reached through `get_ref`
            // — and that payload is where `tokio-rustls` leaves the `rustls::Error`.
            Some(io_error) => io_error.get_ref().map(|inner| {
                let inner: &(dyn std::error::Error + 'static) = inner;
                inner
            }),
            None => current.source(),
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use mysql_async::{IoError, TlsError};

    /// `Require` is the verifying mode, and it keeps both `danger` switches alone.
    #[test]
    fn require_asks_for_a_verified_tls_connection() {
        let opts = ssl_opts(TlsMode::Require).expect("Require asks for TLS");
        assert!(
            !opts.accept_invalid_certs(),
            "Require accepts any certificate"
        );
        assert!(!opts.skip_domain_validation(), "Require skips the name");
        assert!(
            !opts.disable_built_in_roots(),
            "Require turned the built-in roots off, and no others are set, which \
             would trust nothing"
        );
        // Which store this is, pinned deliberately: the built-in public roots, not
        // the platform store. See the module notes — the day this list stops being
        // empty, a private CA has become configurable.
        assert!(
            opts.root_certs().is_empty(),
            "Require gained configured root certificates"
        );
    }

    /// The parity rule, pinned where it can be seen: `Prefer` encrypts without
    /// authenticating, so an internal server whose certificate no store has ever
    /// seen still connects. `RequireNoVerify` is the same configuration reached by
    /// name; the difference between the two is only the fallback below.
    #[test]
    fn prefer_encrypts_without_checking_the_certificate() {
        for mode in [TlsMode::Prefer, TlsMode::RequireNoVerify] {
            let opts = ssl_opts(mode).expect("both ask for TLS");
            assert!(
                opts.accept_invalid_certs(),
                "{mode:?} verifies, which would break every internal server with a \
                 self-signed certificate"
            );
            // And it does not take the second dangerous switch with it: the name is
            // still checked when verification is on. (With `accept_invalid_certs`
            // the chain check is skipped before the name is ever consulted, so this
            // flag stays false and says what the code asked for.)
            assert!(!opts.skip_domain_validation());
        }
    }

    /// `Disable` must not offer the capability at all. Offering it and then
    /// ignoring the server's answer would be a connection that encrypts without
    /// anyone having asked for it.
    #[test]
    fn disable_never_offers_tls() {
        assert!(ssl_opts(TlsMode::Disable).is_none());
    }

    /// The rule that keeps `Prefer`'s missing verification from mattering: no TLS
    /// on offer is not the same as TLS that failed.
    #[test]
    fn only_a_server_that_says_it_has_no_tls_allows_a_retry_in_clear() {
        let no_tls = MysqlError::Driver(DriverError::NoClientSslFlagFromServer);
        assert!(may_fall_back(TlsMode::Prefer, &no_tls));

        // A handshake that broke, or a socket that closed, is not that answer.
        // Constructed as an I/O error because the TLS variants cannot be built
        // from outside `mysql_async`; the real broken handshake is produced by a
        // stub server in `tests/tls.rs`, which is where this rule is checked
        // against a genuine one.
        let broken = MysqlError::Io(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset by peer",
        )));
        assert!(!may_fall_back(TlsMode::Prefer, &broken));

        // And no other mode retries in clear, whatever the server said.
        assert!(!may_fall_back(TlsMode::Require, &no_tls));
        assert!(!may_fall_back(TlsMode::RequireNoVerify, &no_tls));
        assert!(!may_fall_back(TlsMode::Disable, &no_tls));
    }

    /// A rejected certificate is recognised in both shapes it arrives in, and
    /// nothing else is mistaken for one. Pinned because the failure the integration
    /// suite asserts on — an untrusted CA — is exactly this, and a pin that
    /// silently stopped matching would report a refusal as a transient I/O error.
    #[test]
    fn a_rejected_certificate_is_recognised_in_the_shapes_it_arrives_in() {
        let rejected =
            || rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);

        // `mysql_async`'s own TLS variant, which is what the paths it wraps itself
        // produce.
        let direct = MysqlError::Io(IoError::Tls(TlsError::Tls(rejected())));
        assert_eq!(
            certificate_rejection(&direct),
            Some(&rustls::CertificateError::UnknownIssuer)
        );

        // And the shape a live server produced: `tokio-rustls` reports a failed
        // handshake as an `io::Error` carrying the `rustls::Error`
        // (`tokio-rustls-0.26.5/src/client.rs:89`), which is where the first
        // version of this function stopped matching.
        let wrapped = MysqlError::Io(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rejected(),
        )));
        assert_eq!(
            certificate_rejection(&wrapped),
            Some(&rustls::CertificateError::UnknownIssuer)
        );

        // Not a certificate problem at all.
        let broken = MysqlError::Io(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset by peer",
        )));
        assert!(certificate_rejection(&broken).is_none());
    }
}
