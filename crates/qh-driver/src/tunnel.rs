//! A description of the SSH bastion a connection should be reached through.
//!
//! This type lives in `qh-driver`, not in `qh-tunnel`, for one reason: it is part of
//! [`crate::ConnectionConfig`], and `qh-tunnel` depends on `qh-driver` only indirectly
//! today — making the config depend on the tunnel crate would put the tunnel's whole
//! dependency tree (`russh`, the SSH key machinery) in every driver. The description
//! carries no SSH type; mapping it onto `qh-tunnel`'s `BastionConfig` is the engine's
//! job, at the one place a tunnel is actually opened.
//!
//! The tunnel is part of *connecting*, not a feature of its own: the blueprint lists
//! the tunnel as the crate that `connection(settings)` reaches when a bastion is
//! configured (§3.2, `qh-tunnel/` at line 618, "Fase 1 mengerjakan tunnel sebelum UI
//! menyentuhnya" at line 1037). Nothing here changes how the drivers themselves
//! connect: they keep receiving a host and a port, which the engine points at the
//! tunnel's loopback endpoint when one is configured.

use std::path::PathBuf;

/// How to authenticate to the bastion, as the caller spelled it in settings.
///
/// The SSH agent is the default when a bastion is configured but no method is named,
/// for the same reason `ssh` tries it first: a user who runs one has already decided
/// where their keys live, and asking them to name a key file they never load would be
/// the only setting that duplicates what the agent already knows.
#[derive(Clone, PartialEq, Eq)]
pub enum TunnelAuth {
    /// `ssh-agent` (`SSH_AUTH_SOCK` must be set).
    Agent,
    /// A private key file, with `SSH_KEY_PASSPHRASE` applied when it is encrypted.
    Key(PathBuf),
    /// A password, read from `SSH_PASSWORD` rather than stored on the config.
    Password(String),
}

/// Where the bastion is and how to reach it.
///
/// The target is not in this struct: it is the database host and port from the
/// connection itself, which is what the tunnel forwards to. Keeping it out means one
/// value cannot disagree with the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: TunnelAuth,
    /// The `known_hosts` file the host key is checked against. Read by the caller's
    /// own code, which is why the app ships without a sandbox (ADR-0007). When no
    /// path is set, `~/.ssh/known_hosts` is used — the file `ssh` itself writes.
    pub known_hosts: Option<PathBuf>,
    /// The app's own `known_hosts` (`SSH_APP_KNOWN_HOSTS`): the one file the engine both
    /// reads and writes, and the only place a host key accepted by a person is recorded
    /// (blueprint W11 §5.2). `None` means the engine never writes a `known_hosts` and
    /// there is no trust-on-first-use path at all, which is what the command line and the
    /// MCP server get.
    pub app_known_hosts: Option<PathBuf>,
    /// The fingerprint a person confirmed for this one run (`SSH_HOST_KEY_ACCEPT`), as
    /// `SHA256:<base64>`. Applies only to a host that is not on record, only when it
    /// matches what the bastion presents, and is never stored on a connection.
    pub host_key_accept: Option<String>,
    /// The `~/.ssh/config` alias `host` was resolved from, for display only. It is not
    /// part of what identifies the tunnel: `host`, `port` and `user` already are.
    pub alias: Option<String>,
}

impl TunnelConfig {
    /// A default bastion port: the one `ssh` itself assumes.
    pub const DEFAULT_PORT: u16 = 22;
}

impl std::fmt::Debug for TunnelAuth {
    /// Never prints the password: a `{:?}` on a config is the easiest way to write a
    /// live credential into a log file, which is the same rule
    /// [`crate::ConnectionConfig`]'s manual `Debug` follows.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelAuth::Agent => formatter.write_str("Agent"),
            TunnelAuth::Key(path) => formatter.debug_tuple("Key").field(path).finish(),
            TunnelAuth::Password(_) => formatter.write_str("Password(<redacted>)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_never_prints() {
        let auth = TunnelAuth::Password("hunter2".to_owned());
        let rendered = format!("{auth:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }
}
