//! The error taxonomy of this crate.
//!
//! The host-key variants are the reason this crate exists, so they carry what the UI
//! needs to put a decision in front of a person: the fingerprint to show, the key they
//! would be accepting, and what is already on record. A caller never has to re-read the
//! file to build that prompt.

use std::path::PathBuf;

use crate::known_hosts::RecordedKey;
use crate::ServerKey;

/// Everything that can go wrong checking a host key or opening a tunnel.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A `known_hosts` line is one `ssh` would refuse. We refuse it too: skipping it
    /// would mean silently not checking whatever it recorded.
    #[error("{file} line {line} is not a known_hosts line ssh would accept")]
    MalformedKnownHosts { file: String, line: usize },

    /// The file exists but could not be read. A file that does not exist is *not* this
    /// error: it means nothing is recorded yet, which is what TOFU is for.
    #[error("cannot read {path}: {source}")]
    KnownHostsUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Bytes that are not an SSH public key blob. Writing them out would corrupt the
    /// user's `known_hosts`.
    #[error("not an SSH public key blob")]
    MalformedKeyBlob,

    /// `$HOME` is unset, so `~/.ssh/known_hosts` has no location.
    #[error("$HOME is not set, so ~/.ssh/known_hosts cannot be located")]
    NoHomeDirectory,

    /// The host is not in `known_hosts`. This is the TOFU case: the fingerprint is what
    /// the human is asked about, and `key` is what gets appended if they say yes.
    #[error("host key for {host}:{port} is not known: {key}")]
    HostKeyUnknown {
        host: String,
        port: u16,
        path: PathBuf,
        key: ServerKey,
        /// True when the file has a `@cert-authority` line covering this host, so the
        /// person should be told a CA is trusted for it and this build still refuses
        /// certificates.
        covered_by_certificate_authority: bool,
    },

    /// The recorded key and the presented key are both valid keys and different. This
    /// is the case TOFU must never resolve on its own.
    #[error("host key for {host}:{port} changed: presented {key}, recorded {}", recorded_list(.recorded))]
    HostKeyMismatch {
        host: String,
        port: u16,
        path: PathBuf,
        key: ServerKey,
        recorded: Vec<RecordedKey>,
    },

    /// A `@revoked` line matches this host and this key. Nothing may override it.
    #[error("the key for {host}:{port} is revoked in {path} at line {line}")]
    HostKeyRevoked {
        host: String,
        port: u16,
        path: PathBuf,
        line: usize,
    },

    /// The server offered a host certificate. This crate verifies host keys and does
    /// not verify certificates, and says so rather than ignoring the offer.
    #[error("{host}:{port} presented a host certificate, which this build cannot verify")]
    HostCertificateUnsupported {
        host: String,
        port: u16,
        fingerprint: String,
    },

    /// No authentication method succeeded.
    #[error("authentication as {user} was rejected; the server still offers {methods}")]
    AuthenticationRejected { user: String, methods: String },

    /// A private key file could not be read.
    #[error("cannot read private key {path}: {source}")]
    KeyUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The SSH transport itself.
    #[error(transparent)]
    Ssh(#[from] russh::Error),

    /// Key decoding and SSH agents.
    #[error(transparent)]
    Keys(#[from] russh::keys::Error),

    /// A key that does not survive a round trip through the SSH key encoding.
    #[error(transparent)]
    SshKey(#[from] russh::keys::ssh_key::Error),

    /// A `ssh-agent` signing request.
    #[error(transparent)]
    Agent(#[from] russh::AgentAuthError),

    /// Local sockets and files.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The fingerprint to show a person, when the error is about a host key.
    #[must_use]
    pub fn fingerprint(&self) -> Option<String> {
        match self {
            Error::HostKeyUnknown { key, .. } | Error::HostKeyMismatch { key, .. } => {
                Some(key.fingerprint())
            }
            Error::HostCertificateUnsupported { fingerprint, .. } => Some(fingerprint.clone()),
            _ => None,
        }
    }
}

/// The recorded keys as fingerprints, because the message is read by a person deciding
/// whether their bastion was rotated or impersonated, and a blob dump helps neither.
fn recorded_list(keys: &[RecordedKey]) -> String {
    keys.iter()
        .map(|key| format!("{} on line {}", key.fingerprint(), key.line()))
        .collect::<Vec<_>>()
        .join(", ")
}
