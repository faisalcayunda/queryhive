//! The error taxonomy of this crate.
//!
//! The host-key variants are the reason this crate exists, so they carry what the UI
//! needs to put a decision in front of a person: the fingerprint to show, the key they
//! would be accepting, and what is already on record. A caller never has to re-read the
//! file to build that prompt.

use std::path::PathBuf;
use std::time::Duration;

use crate::known_hosts::{remove_command, Origin, RecordedKey};
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
    #[error("host key for {host}:{port} changed: {}", changed_detail(.host, *.port, .key, .recorded))]
    HostKeyMismatch {
        host: String,
        port: u16,
        path: PathBuf,
        key: ServerKey,
        recorded: Vec<RecordedKey>,
    },

    /// The host is covered by a `@cert-authority` line in the user's own `known_hosts`
    /// and presented a plain key. That is a downgrade or a changed configuration, and
    /// no pin or prompt may turn it into trust: this build cannot verify certificates.
    #[error("{host}:{port} is covered by a @cert-authority line but presented a plain key ({key}); edit that line if the host is no longer CA-managed")]
    HostKeyCertificateExpected {
        host: String,
        port: u16,
        key: ServerKey,
        /// The file and line of a covering `@cert-authority` entry are not tracked; the
        /// message tells the user which kind of line to look for.
        path: PathBuf,
    },

    /// The key to trust was named by fingerprint, and the server presented another.
    #[error("{host}:{port} presented {presented}, not the pinned {pinned}")]
    HostKeyPinMismatch {
        host: String,
        port: u16,
        key: ServerKey,
        presented: String,
        pinned: String,
    },

    /// The pinned key matched but could not be recorded, so it was not accepted:
    /// no authentication is ever sent to a key that is not on record.
    #[error("could not record the host key for {host}:{port} in {path}: {reason}")]
    HostKeyRecordFailed {
        host: String,
        port: u16,
        key: ServerKey,
        path: PathBuf,
        reason: String,
    },

    /// The app's own `known_hosts` is a symlink, someone else's, or writable by others,
    /// so what it says cannot be trusted. Fix it with `chmod 600` and the right owner.
    #[error("{path} {reason}; it must be a regular file you own with mode 600 (chmod 600 on it)")]
    HostKeyStoreUnsafe { path: PathBuf, reason: &'static str },

    /// The key exchange finished without the host-key check having accepted anything.
    /// Authentication is never started on such a connection.
    #[error(
        "the bastion connection was not host-key verified, so authentication was not attempted"
    )]
    HostKeyNotVerified,

    /// The bastion did not accept the connection and finish the key exchange in time:
    /// a black-holed address, a port that accepts and stays silent, a server that
    /// stalls after its banner. Nothing was sent that identifies the user, because no
    /// host key had been checked yet. Authentication is not covered by this limit
    /// (an agent may be waiting on a person); a dead server there is closed by the
    /// keepalives.
    #[error("connecting to the bastion {host}:{port} timed out after {after:?}; the SSH handshake did not complete, so no credentials were sent")]
    ConnectTimeout {
        host: String,
        port: u16,
        after: Duration,
    },

    /// The caller's request cannot be honoured, found before any network use.
    #[error("{0}")]
    Usage(&'static str),

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
            Error::HostKeyUnknown { key, .. }
            | Error::HostKeyMismatch { key, .. }
            | Error::HostKeyCertificateExpected { key, .. } => Some(key.fingerprint()),
            Error::HostCertificateUnsupported { fingerprint, .. } => Some(fingerprint.clone()),
            Error::HostKeyPinMismatch { presented, .. } => Some(presented.clone()),
            _ => None,
        }
    }
}

/// What a person needs to judge a changed host key (blueprint W11 §5.7): the key that was
/// presented, every key on record with the file and line it is on, and the one way
/// out, a command they run themselves. It never offers to take the new key: a rotation
/// is confirmed with the host's administrator first, outside this network path.
fn changed_detail(host: &str, port: u16, key: &ServerKey, recorded: &[RecordedKey]) -> String {
    let mut text = format!(
        "the server presented {} {}, but what is on record is different: ",
        key.key_type(),
        key.fingerprint()
    );
    let described: Vec<String> = recorded
        .iter()
        .map(|record| {
            let place = match (record.file(), record.origin()) {
                (Some(file), Some(Origin::System)) => format!(
                    " in {} line {}, which only an administrator can change",
                    file.display(),
                    record.line()
                ),
                (Some(file), _) => format!(" in {} line {}", file.display(), record.line()),
                (None, _) => String::new(),
            };
            format!("{} {}{place}", record.key_type(), record.fingerprint())
        })
        .collect();
    text.push_str(&described.join("; "));
    text.push('.');

    // One command per file we may edit; `ssh-keygen -R` keeps a `.old` copy beside it.
    let mut files: Vec<&std::path::Path> = Vec::new();
    for record in recorded {
        if let (Some(file), Some(Origin::App | Origin::User)) = (record.file(), record.origin()) {
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    if !files.is_empty() {
        text.push_str(
            " If the administrator confirms the key was replaced, check the new fingerprint \
             with them first, then remove the old record yourself (a copy is kept as \
             known_hosts.old): ",
        );
        let commands: Vec<String> = files
            .iter()
            .map(|file| remove_command(host, port, file))
            .collect();
        text.push_str(&commands.join("  ;  "));
    }
    text
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;
    use crate::known_hosts::{check_all, HostKeyVerdict, StoreFile};

    fn blob(seed: u8) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 11];
        out.extend_from_slice(b"ssh-ed25519");
        out.extend_from_slice(&[seed; 32]);
        out
    }

    fn line(seed: u8) -> String {
        format!(
            "bastion.corp ssh-ed25519 {}\n",
            base64::engine::general_purpose::STANDARD.encode(blob(seed))
        )
    }

    #[test]
    fn a_changed_key_message_names_every_record_and_the_one_way_out() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temp dir");
        // A user file, an app file in a directory whose name needs shell quoting, and
        // a system file only an administrator can change.
        let quoted = dir.path().join("O'Brien");
        std::fs::create_dir(&quoted).expect("mkdir");
        let (user, app, system) = (
            dir.path().join("user"),
            quoted.join("known_hosts"),
            dir.path().join("system"),
        );
        std::fs::write(&user, line(2)).unwrap();
        std::fs::write(&app, line(3)).unwrap();
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&system, line(4)).unwrap();

        let files = [
            StoreFile::new(&user, Origin::User),
            StoreFile::new(&system, Origin::System),
            StoreFile::new(&app, Origin::App),
        ];
        let presented = ServerKey::from_blob(blob(1)).expect("a key");
        let HostKeyVerdict::Mismatch { recorded } =
            check_all(&files, "bastion.corp", 22, presented.blob()).expect("checks")
        else {
            panic!("expected a mismatch");
        };
        let error = Error::HostKeyMismatch {
            host: "bastion.corp".to_owned(),
            port: 22,
            path: user.clone(),
            key: presented.clone(),
            recorded: recorded.clone(),
        };
        let message = error.to_string();

        assert!(message.contains(&presented.fingerprint()), "{message}");
        assert!(message.contains("ssh-ed25519"), "{message}");
        for (record, file) in recorded.iter().zip([&user, &system, &app]) {
            assert!(message.contains(&record.fingerprint()), "{message}");
            assert!(
                message.contains(&format!("{} line 1", file.display())),
                "{message}"
            );
        }
        assert!(
            message.contains("which only an administrator can change"),
            "{message}"
        );
        // A command for the two files we may edit, quoted for a shell, and none for the
        // system file.
        assert!(
            message.contains(&remove_command("bastion.corp", 22, &user)),
            "{message}"
        );
        let app_command = remove_command("bastion.corp", 22, &app);
        assert!(message.contains(&app_command), "{message}");
        assert!(app_command.contains("O'\\''Brien"), "{app_command}");
        assert!(
            !message.contains(&format!("-f '{}'", system.display())),
            "{message}"
        );
        // Nothing here invites accepting the new key.
        let lower = message.to_ascii_lowercase();
        for word in ["accept", "trust", "continue"] {
            assert!(!lower.contains(word), "{word}: {message}");
        }
    }

    #[test]
    fn a_changed_key_with_no_file_on_record_has_no_command() {
        let presented = ServerKey::from_blob(blob(1)).expect("a key");
        let error = Error::HostKeyMismatch {
            host: "bastion.corp".to_owned(),
            port: 22,
            path: PathBuf::from("/app"),
            key: presented.clone(),
            recorded: vec![RecordedKey::pinned(&ServerKey::from_blob(blob(2)).unwrap())],
        };
        let message = error.to_string();
        assert!(!message.contains("ssh-keygen"), "{message}");
        assert!(message.contains("changed"), "{message}");
    }
}
