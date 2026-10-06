//! The SSH bastion a connection goes through, from settings to an open tunnel.
//!
//! The blueprint's placement, taken literally: `qh-tunnel` is the crate a
//! connection reaches when a bastion is configured (§3.2 line 618), built in phase 1
//! "before the UI touches it" (§7 line 1037), and it is part of *connecting* — there
//! is no tunnel command, and none was added. This module is the engine half of that:
//! it turns `SSH_*` settings into [`TunnelConfig`] ([`settings`]), and — in
//! [`open`] — opens the tunnel and hands the driver the
//! loopback endpoint it listens on.
//!
//! # The `SSH_*` vocabulary, and why these names
//!
//! Checked first, per the engine's own rule that existing vocabulary wins
//! (`config.rs`'s `DB_*`/`TRINO_*` aliases): neither the Python engine
//! (`app/engine/queryhive_engine.py`) nor the app ever defined any SSH setting —
//! the app says so itself where it imports connections that have one
//! (`Sources/TrinoExporter/Support/NavicatImport.swift`: "QueryHive cannot open an
//! SSH tunnel", reading Navicat's `SSH_Host` field). The names here are Navicat's
//! own spelling, snake-cased to the setting style the engine already uses, because
//! that is the one SSH vocabulary this product has ever contained:
//!
//! | Setting | Meaning |
//! |---|---|
//! | `SSH_HOST` | the bastion; unset or blank means no tunnel |
//! | `SSH_PORT` | default 22 |
//! | `SSH_USER` | the bastion account |
//! | `SSH_AUTH_METHOD` | `agent` (default), `key`, `password` |
//! | `SSH_KEY_PATH` | the private key, for `key` |
//! | `SSH_KEY_PASSPHRASE` | its passphrase, when it has one |
//! | `SSH_PASSWORD` | the bastion password, for `password` |
//! | `SSH_KNOWN_HOSTS` | the file to check against; default `~/.ssh/known_hosts` |
//! | `SSH_USE_CONFIG` | `1`: `SSH_HOST` is a `Host` alias in `~/.ssh/config` |
//! | `SSH_CONFIG_PATH` | the config file for that; default `~/.ssh/config` |
//! | `SSH_APP_KNOWN_HOSTS` | the app's own `known_hosts`: read, and the only file ever written |
//! | `SSH_HOST_KEY_ACCEPT` | one `SHA256:` fingerprint a person confirmed, for this run only |
//! | `SSH_HOST_KEY_DETAIL` | `1`: a refused host key adds a `host_key` object to the `error` event |
//!
//! # What an unknown host does here, and why
//!
//! The security rule of `qh-tunnel` is kept whole: the host key is checked during
//! the key exchange, before anything authenticating is sent, and trust-on-first-use
//! is two calls with a person's decision between them — never a silent accept. The
//! NDJSON event protocol is one-way (an engine process emits events; there is no
//! channel back, blueprint §1.2), so the decision cannot round-trip inside one run.
//! An unknown host is therefore refused, and the refusal says what to decide:
//!
//! - **On the command line and over MCP** the `connect` error's message carries the
//!   fingerprint and the `known_hosts` path, telling the user to add the key out of
//!   band (`ssh-keyscan`, or connecting once with `ssh` itself). There is no accept path
//!   at all: nothing writes a `known_hosts` there.
//! - **In the app** the same refusal is an [`EngineError::HostKey`], and with
//!   `SSH_HOST_KEY_DETAIL=1` the `error` event carries a `host_key` object (blueprint W11
//!   §5.8). The app shows the fingerprint to a person and, on "yes", runs again with
//!   `SSH_HOST_KEY_ACCEPT=<that fingerprint>` and `SSH_APP_KNOWN_HOSTS=<its own file>`.
//!   `qh-tunnel` then accepts only an *unknown* host whose key has exactly that
//!   fingerprint, and only after recording it in the app's file. A changed, revoked or
//!   CA-covered host is refused whatever the pin says.

use std::path::{Path, PathBuf};

use qh_core::{EngineError, FailureKind, HostKeyFailure, HostKeyState, RecordedHostKey};
use qh_driver::{ConnectionConfig, TunnelAuth, TunnelConfig};
use qh_tunnel::known_hosts::{self, Origin};
use thiserror::Error;

use crate::env::{SettingError, Settings};

/// Why a tunnel description could not be built from the settings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TunnelConfigError {
    #[error("{0} is required when SSH_HOST is set")]
    Missing(&'static str),

    #[error("unknown SSH_AUTH_METHOD '{0}'; expected agent, key or password")]
    BadAuthMethod(String),

    #[error("SSH_AUTH_METHOD=key needs SSH_KEY_PATH")]
    KeyPathMissing,

    #[error("SSH_AUTH_METHOD=password needs SSH_PASSWORD")]
    PasswordMissing,

    #[error("$HOME is not set and SSH_KNOWN_HOSTS is not set either, so there is no known_hosts file to check the bastion against")]
    NoKnownHosts,

    /// Deliberately without the value: it came from settings, a Navicat file or an ssh config,
    /// and it is about to be printed into a shell command and written to a `known_hosts` line.
    #[error("SSH_HOST is not a valid host name or address")]
    BadHost,

    #[error("SSH_HOST_KEY_ACCEPT must be a SHA256:<43 base64 characters> fingerprint")]
    BadHostKeyAccept,

    #[error("SSH_HOST_KEY_ACCEPT needs SSH_APP_KNOWN_HOSTS, the file the accepted key is recorded in before anything is sent to the bastion")]
    PinNeedsAppFile,

    #[error("$HOME is not set and SSH_CONFIG_PATH is not set either, so there is no ssh config to read the alias from")]
    NoSshConfig,

    /// The ssh config could not be read or the alias cannot be used. The message is the one
    /// `qh-tunnel` wrote: it names the file, the line and the directive, never a value.
    #[error("{0}")]
    SshConfig(String),

    #[error(transparent)]
    Setting(#[from] SettingError),
}

/// The `~/.ssh/config` to read an alias from: `SSH_CONFIG_PATH`, or `~/.ssh/config`.
fn ssh_config_path(settings: &Settings) -> Result<PathBuf, TunnelConfigError> {
    match settings.text("SSH_CONFIG_PATH", "") {
        path if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => ssh_config_default_path(),
    }
}

/// `~/.ssh/config`, the file the form's alias list and preview read.
pub(crate) fn ssh_config_default_path() -> Result<PathBuf, TunnelConfigError> {
    home_file(&[".ssh", "config"]).ok_or(TunnelConfigError::NoSshConfig)
}

fn home_file(parts: &[&str]) -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("HOME")?);
    path.extend(parts);
    Some(path)
}

/// Resolve `alias` in the ssh config at `path`, the one way the connect path, the form's preview
/// and the alias list all read it, so what a person is shown is what a connect will use.
pub(crate) fn resolve_alias(
    path: &Path,
    alias: &str,
) -> Result<qh_tunnel::ssh_config::Resolved, TunnelConfigError> {
    qh_tunnel::ssh_config::load(path)
        .and_then(|config| config.resolve(alias))
        .map_err(|error| TunnelConfigError::SshConfig(error.to_string()))
}

/// Every alias a person can pick from the ssh config at `path`. A config that cannot be read has
/// none to offer: listing never fails, because the resolve step is where a bad one is explained.
pub(crate) fn alias_list(path: &Path) -> Vec<String> {
    qh_tunnel::ssh_config::load(path)
        .map(|config| config.aliases())
        .unwrap_or_default()
}

/// The [`TunnelConfig`] the settings describe, or `None` when `SSH_HOST` is absent.
///
/// `SSH_HOST` is the single switch: it is the one value without which there is no
/// bastion at all, so a blank or missing one means "no tunnel" whatever the other
/// `SSH_*` settings say. That mirrors how `DB_HOST` decides there is a connection.
///
/// With `SSH_USE_CONFIG=1`, `SSH_HOST` is an alias in the ssh config and its `HostName`, `User`,
/// `Port` and first `IdentityFile` fill in what the settings leave blank (an explicit setting
/// always wins; `SSH_AUTH_METHOD` is never taken from the config). Whatever host is contacted,
/// from the alias or not, is checked before the network is touched.
pub fn settings(settings: &Settings) -> Result<Option<TunnelConfig>, TunnelConfigError> {
    let named = settings.text("SSH_HOST", "");
    if named.is_empty() {
        return Ok(None);
    }

    let from_config = if settings.flag("SSH_USE_CONFIG", false) {
        Some(resolve_alias(&ssh_config_path(settings)?, &named)?)
    } else {
        None
    };
    let (host, alias) = match &from_config {
        Some(resolved) => (resolved.host_name.clone(), Some(named)),
        None => (named, None),
    };
    if !known_hosts::is_valid_host(&host) {
        return Err(TunnelConfigError::BadHost);
    }

    let user = match settings.text("SSH_USER", "") {
        user if !user.is_empty() => user,
        _ => from_config
            .as_ref()
            .and_then(|resolved| resolved.user.clone())
            .ok_or(TunnelConfigError::Missing("SSH_USER"))?,
    };

    let default_port = from_config
        .as_ref()
        .and_then(|resolved| resolved.port)
        .unwrap_or(TunnelConfig::DEFAULT_PORT);
    let port = settings
        .number("SSH_PORT", i64::from(default_port))
        .map_err(TunnelConfigError::Setting)?;
    let port = u16::try_from(port).map_err(|_| SettingError::NotANumber {
        key: "SSH_PORT".to_owned(),
        value: port.to_string(),
    })?;

    let auth = match settings.text("SSH_AUTH_METHOD", "agent").as_str() {
        "agent" => TunnelAuth::Agent,
        "key" => {
            let path = settings.text("SSH_KEY_PATH", "");
            if !path.is_empty() {
                TunnelAuth::Key(PathBuf::from(path))
            } else {
                // OpenSSH tries every IdentityFile in turn; this tries one, the first that is
                // there, and says so in the docs rather than guessing at the rest.
                let first = from_config.as_ref().and_then(|resolved| {
                    resolved.identity_files.iter().find(|file| file.is_file())
                });
                match first {
                    Some(file) => TunnelAuth::Key(file.clone()),
                    None => return Err(TunnelConfigError::KeyPathMissing),
                }
            }
        }
        "password" => {
            // Carried as data, not read from the process environment at connect
            // time, for the same reason the FFI surface passes settings as data: a
            // secret in the environment is a secret in `ps`.
            let password = settings.raw("SSH_PASSWORD", "");
            if password.is_empty() {
                return Err(TunnelConfigError::PasswordMissing);
            }
            TunnelAuth::Password(password)
        }
        other => return Err(TunnelConfigError::BadAuthMethod(other.to_owned())),
    };

    let path_setting = |key: &str| match settings.text(key, "") {
        path if !path.is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };
    let known_hosts = path_setting("SSH_KNOWN_HOSTS");
    let app_known_hosts = path_setting("SSH_APP_KNOWN_HOSTS");

    // Shape-checked here, before any network, so a typo is a usage error and not a refusal
    // that looks like an attack. The value is never echoed.
    let host_key_accept = match settings.text("SSH_HOST_KEY_ACCEPT", "") {
        pin if pin.is_empty() => None,
        pin if !is_fingerprint(&pin) => return Err(TunnelConfigError::BadHostKeyAccept),
        pin => Some(pin),
    };
    if host_key_accept.is_some() && app_known_hosts.is_none() {
        return Err(TunnelConfigError::PinNeedsAppFile);
    }

    Ok(Some(TunnelConfig {
        host,
        port,
        user,
        auth,
        known_hosts,
        app_known_hosts,
        host_key_accept,
        alias,
    }))
}

/// `SHA256:` and the 43 base64 characters of an unpadded SHA-256, as `ssh-keygen -l` prints it.
fn is_fingerprint(text: &str) -> bool {
    text.strip_prefix("SHA256:").is_some_and(|rest| {
        rest.len() == 43
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
    })
}

/// The `known_hosts` file to check against: the one the caller named, or the one
/// `ssh` itself uses.
///
/// Resolved here rather than inside `qh-tunnel` so that the refusal message can
/// name the file a person should edit, and so a missing `$HOME` is a usage error
/// with our wording rather than a tunnel error discovered mid-handshake.
pub fn known_hosts_path(config: &TunnelConfig) -> Result<PathBuf, TunnelConfigError> {
    match &config.known_hosts {
        Some(path) => Ok(path.clone()),
        None => home_file(&[".ssh", "known_hosts"]).ok_or(TunnelConfigError::NoKnownHosts),
    }
}

/// Every read-only file whose contents decide whether a bastion's key is trusted, in the order
/// they are read: the user's (or `SSH_KNOWN_HOSTS`), then the system's.
///
/// The pool key is built from this list: a tunnel verified against one set of files must not be
/// reused by a run that names another, or choosing a file would be a way around verification.
pub fn trust_files(config: &TunnelConfig) -> Vec<PathBuf> {
    known_hosts_path(config)
        .into_iter()
        .chain([PathBuf::from(known_hosts::SYSTEM_KNOWN_HOSTS)])
        .collect()
}

/// Point the connection at the tunnel's loopback endpoint.
///
/// The tunnel forwards to the database host and port as configured, so the target
/// is read off the config *before* it is rewritten — the one order that cannot
/// disagree.
///
/// **The name the certificate is checked against stays the database's own** (blueprint W11
/// §7.3): the driver is handed `127.0.0.1`, and a certificate is issued for the database's
/// name, so [`ConnectionConfig::tls_server_name`] carries the original host. Without it every
/// verifying mode fails through a tunnel. MySQL cannot take a name, so it is left alone.
///
/// Not set for a Trino host that is an IP literal: Trino's driver resolves the name to the
/// loopback endpoint with a DNS override, and a literal address is never looked up, so naming it
/// would send the request (and its credentials) to that address directly instead of through the
/// tunnel. Such a connection verifies against `127.0.0.1` and fails in the verifying mode, which
/// is a refusal and not a bypass.
pub fn retarget(config: &mut ConnectionConfig, local_port: u16) -> qh_tunnel::Target {
    let target = qh_tunnel::Target::new(config.host.clone(), config.port);
    let literal = config
        .host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok();
    config.tls_server_name = match config.kind {
        qh_driver::DriverKind::Postgres => Some(config.host.clone()),
        qh_driver::DriverKind::Trino if !literal => Some(config.host.clone()),
        _ => None,
    };
    config.host = "127.0.0.1".to_owned();
    config.port = local_port;
    target
}

/// The `qh-tunnel` bastion description for one connection.
///
/// The passphrase and password move from plain settings values into
/// [`qh_tunnel::SecretString`] here, at the boundary where they stop being
/// configuration and start being credentials. The passphrase comes from `settings`
/// rather than from [`TunnelConfig`] so the shared config type never holds one: the
/// description may be logged or persisted one day, and the day it is, the passphrase
/// is not in it.
pub fn bastion(
    config: &TunnelConfig,
    settings: &Settings,
) -> Result<qh_tunnel::BastionConfig, TunnelConfigError> {
    let auth = match &config.auth {
        TunnelAuth::Agent => qh_tunnel::Auth::Agent,
        TunnelAuth::Key(path) => qh_tunnel::Auth::Key {
            path: path.clone(),
            passphrase: match settings.raw("SSH_KEY_PASSPHRASE", "") {
                value if value.is_empty() => None,
                value => Some(qh_tunnel::SecretString::new(value.into_boxed_str())),
            },
        },
        TunnelAuth::Password(password) => qh_tunnel::Auth::Password(qh_tunnel::SecretString::new(
            password.clone().into_boxed_str(),
        )),
    };
    let mut bastion = qh_tunnel::BastionConfig::new(
        config.host.clone(),
        config.port,
        config.user.clone(),
        auth,
        known_hosts_path(config)?,
    )
    .with_system_known_hosts();
    // The app's own file is the only one ever written, and only for a pinned unknown host.
    if let Some(app) = &config.app_known_hosts {
        bastion = bastion.record_to(app);
    }
    if let Some(pin) = &config.host_key_accept {
        bastion = bastion.trust_fingerprint(pin);
    }
    Ok(bastion)
}

/// Open the tunnel a connection's bastion description asks for, towards `target`.
///
/// The one place `SSH_*` settings become a running tunnel, shared by a connection that owns its
/// tunnel ([`crate::RealEngine`]) and by a pool that shares one across sessions
/// ([`crate::host`]), so the two cannot disagree about what an unknown host or a bad key means.
/// A refused host key is an [`EngineError::HostKey`], which is [`FailureKind::Permanent`]:
/// retrying without a person's answer would present the same fingerprint to the same refusal, so
/// the retry layer must not see it as transient. Every other tunnel failure stays a permanent
/// `connect` error, as it always was.
pub async fn open(
    description: &TunnelConfig,
    settings: &Settings,
    target: qh_tunnel::Target,
) -> Result<qh_tunnel::Tunnel, EngineError> {
    let bastion = bastion(description, settings).map_err(|error| EngineError::Usage {
        message: error.to_string(),
    })?;
    qh_tunnel::Tunnel::open(&bastion, target)
        .await
        .map_err(|error| refused(&error, description))
}

/// A tunnel failure as the engine reports it: structured when it is about the host key.
fn refused(error: &qh_tunnel::Error, description: &TunnelConfig) -> EngineError {
    match host_key_failure(error, description) {
        Some(failure) => EngineError::HostKey(Box::new(failure)),
        None => EngineError::Connect {
            message: describe(error),
            kind: FailureKind::Permanent,
        },
    }
}

/// What a person needs to decide about a refused host key, or `None` when the error is not one.
///
/// `HostKeyNotVerified` is deliberately `None`: it means the key exchange finished without any
/// check having run, which is a fault in the program and not a prompt, so it stays a plain
/// `connect` error that no sheet is built for.
pub fn host_key_failure(
    error: &qh_tunnel::Error,
    description: &TunnelConfig,
) -> Option<HostKeyFailure> {
    use qh_tunnel::Error as E;

    let mut failure = HostKeyFailure {
        message: describe(error),
        state: HostKeyState::Unknown,
        host: description.host.clone(),
        port: description.port,
        alias: description.alias.clone(),
        key_type: None,
        fingerprint: None,
        app_known_hosts: description
            .app_known_hosts
            .as_ref()
            .map(|path| path.display().to_string()),
        ca_covered: false,
        recorded: Vec::new(),
        pinned: None,
    };
    let key = |failure: &mut HostKeyFailure, key: &qh_tunnel::ServerKey| {
        failure.key_type = Some(key.key_type().to_owned());
        failure.fingerprint = Some(key.fingerprint());
    };
    match error {
        E::HostKeyUnknown {
            host,
            port,
            key: presented,
            covered_by_certificate_authority,
            ..
        } => {
            failure.state = HostKeyState::Unknown;
            (failure.host, failure.port) = (host.clone(), *port);
            failure.ca_covered = *covered_by_certificate_authority;
            key(&mut failure, presented);
        }
        E::HostKeyMismatch {
            host,
            port,
            key: presented,
            recorded,
            ..
        } => {
            failure.state = HostKeyState::Changed;
            (failure.host, failure.port) = (host.clone(), *port);
            key(&mut failure, presented);
            failure.recorded = recorded
                .iter()
                .map(|record| RecordedHostKey {
                    fingerprint: record.fingerprint(),
                    key_type: record.key_type().to_owned(),
                    source: record.origin().unwrap_or(Origin::App).as_str().to_owned(),
                    path: record
                        .file()
                        .map(|file| file.display().to_string())
                        .unwrap_or_default(),
                    line: u32::try_from(record.line()).unwrap_or(u32::MAX),
                })
                .collect();
        }
        E::HostKeyRevoked { host, port, .. } => {
            failure.state = HostKeyState::Revoked;
            (failure.host, failure.port) = (host.clone(), *port);
        }
        E::HostCertificateUnsupported {
            host,
            port,
            fingerprint,
        } => {
            failure.state = HostKeyState::Certificate;
            (failure.host, failure.port) = (host.clone(), *port);
            failure.fingerprint = Some(fingerprint.clone());
        }
        E::HostKeyCertificateExpected {
            host,
            port,
            key: presented,
            ..
        } => {
            failure.state = HostKeyState::CertificateExpected;
            (failure.host, failure.port) = (host.clone(), *port);
            failure.ca_covered = true;
            key(&mut failure, presented);
        }
        E::HostKeyPinMismatch {
            host,
            port,
            key: presented,
            presented: fingerprint,
            pinned,
        } => {
            failure.state = HostKeyState::PinMismatch;
            (failure.host, failure.port) = (host.clone(), *port);
            key(&mut failure, presented);
            failure.fingerprint = Some(fingerprint.clone());
            failure.pinned = Some(pinned.clone());
        }
        E::HostKeyRecordFailed {
            host,
            port,
            key: presented,
            ..
        } => {
            failure.state = HostKeyState::RecordFailed;
            (failure.host, failure.port) = (host.clone(), *port);
            key(&mut failure, presented);
        }
        E::HostKeyStoreUnsafe { .. } => failure.state = HostKeyState::StoreUnsafe,
        _ => return None,
    }
    Some(failure)
}

/// The message a refused connection carries.
///
/// The tunnel errors that a person acts on — an unknown host, a changed one, a
/// revoked one — are worded for exactly that: what was presented, what to do about
/// it, and never a silent fallback. The fingerprint is in the message because the
/// NDJSON protocol has no structured field for it yet; the app reads `error`
/// events, and the message is the field it shows.
pub fn describe(error: &qh_tunnel::Error) -> String {
    match error {
        qh_tunnel::Error::HostKeyUnknown {
            host,
            port,
            path,
            key,
            covered_by_certificate_authority,
        } => {
            let ca = if *covered_by_certificate_authority {
                " (a @cert-authority line covers this host, but this build does not verify host certificates)"
            } else {
                ""
            };
            format!(
                "the host key of the bastion {host}:{port} is not in {}: {} {}{ca}. \
                 Trust-on-first-use needs a person to confirm the fingerprint, and this engine's \
                 protocol cannot ask: verify the fingerprint out of band, add the key to that file \
                 (e.g. `ssh-keyscan -p {port} {host}`), or set SSH_KNOWN_HOSTS to a file that has it, \
                 then connect again",
                path.display(),
                key.key_type(),
                key.fingerprint()
            )
        }
        // The tunnel crate's own wording is kept for the other failures: it already
        // names the host, the fingerprint, and what is on record.
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> Settings {
        Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
    }

    #[test]
    fn no_ssh_host_means_no_tunnel() {
        assert_eq!(settings(&with(&[])).unwrap(), None);
        // The other SSH_* settings alone do not turn it on: SSH_HOST is the switch.
        assert_eq!(
            settings(&with(&[
                ("SSH_USER", "deploy"),
                ("SSH_AUTH_METHOD", "password"),
                ("SSH_PASSWORD", "x"),
            ]))
            .unwrap(),
            None
        );
    }

    #[test]
    fn a_blank_ssh_host_means_no_tunnel() {
        assert_eq!(settings(&with(&[("SSH_HOST", "  ")])).unwrap(), None);
    }

    #[test]
    fn a_bastion_defaults_to_agent_on_port_22() {
        let config = settings(&with(&[
            ("SSH_HOST", "bastion.internal"),
            ("SSH_USER", "deploy"),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!(config.host, "bastion.internal");
        assert_eq!(config.port, 22);
        assert_eq!(config.user, "deploy");
        assert_eq!(config.auth, TunnelAuth::Agent);
        assert_eq!(config.known_hosts, None);
    }

    #[test]
    fn every_part_maps() {
        let config = settings(&with(&[
            ("SSH_HOST", "bastion.internal"),
            ("SSH_PORT", "2222"),
            ("SSH_USER", "deploy"),
            ("SSH_AUTH_METHOD", "key"),
            ("SSH_KEY_PATH", "/keys/id_ed25519"),
            ("SSH_KNOWN_HOSTS", "/etc/ssh/known_hosts"),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!(config.port, 2222);
        assert_eq!(
            config.auth,
            TunnelAuth::Key(PathBuf::from("/keys/id_ed25519"))
        );
        assert_eq!(
            config.known_hosts,
            Some(PathBuf::from("/etc/ssh/known_hosts"))
        );
    }

    #[test]
    fn password_auth_reads_the_password_as_a_secret_setting() {
        let config = settings(&with(&[
            ("SSH_HOST", "bastion.internal"),
            ("SSH_USER", "deploy"),
            ("SSH_AUTH_METHOD", "password"),
            ("SSH_PASSWORD", "s3cret"),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!(config.auth, TunnelAuth::Password("s3cret".to_owned()));
    }

    #[test]
    fn a_missing_user_is_refused_before_anything_is_built() {
        let error = settings(&with(&[("SSH_HOST", "bastion.internal")])).unwrap_err();
        assert_eq!(error, TunnelConfigError::Missing("SSH_USER"));
        assert_eq!(
            error.to_string(),
            "SSH_USER is required when SSH_HOST is set"
        );
    }

    #[test]
    fn an_unknown_auth_method_is_refused_by_spelling() {
        let error = settings(&with(&[
            ("SSH_HOST", "bastion.internal"),
            ("SSH_USER", "deploy"),
            ("SSH_AUTH_METHOD", "pubkey"),
        ]))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "unknown SSH_AUTH_METHOD 'pubkey'; expected agent, key or password"
        );
    }

    #[test]
    fn key_and_password_methods_refuse_to_guess_their_secret() {
        let base = [("SSH_HOST", "bastion.internal"), ("SSH_USER", "deploy")];

        let error = settings(&with(&[base[0], base[1], ("SSH_AUTH_METHOD", "key")])).unwrap_err();
        assert_eq!(error.to_string(), "SSH_AUTH_METHOD=key needs SSH_KEY_PATH");

        let error =
            settings(&with(&[base[0], base[1], ("SSH_AUTH_METHOD", "password")])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "SSH_AUTH_METHOD=password needs SSH_PASSWORD"
        );
    }

    #[test]
    fn a_bad_port_is_refused_by_name() {
        let error = settings(&with(&[
            ("SSH_HOST", "bastion.internal"),
            ("SSH_USER", "deploy"),
            ("SSH_PORT", "lots"),
        ]))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "SSH_PORT must be a whole number, got 'lots'"
        );
    }

    #[test]
    fn retarget_points_the_driver_at_the_loopback_and_keeps_the_target() {
        let mut config =
            ConnectionConfig::new(qh_driver::DriverKind::Postgres, "db.internal", 5432, "app");
        let target = retarget(&mut config, 44_100);
        assert_eq!(target, qh_tunnel::Target::new("db.internal", 5432));
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 44_100);
    }

    #[test]
    fn the_unknown_host_message_carries_the_fingerprint_and_the_fix() {
        let key = qh_tunnel::ServerKey::from_blob(
            // A real ed25519 blob: the one qh-tunnel's own fingerprint test uses.
            base64_decode("AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ"),
        )
        .expect("a valid blob");
        let message = describe(&qh_tunnel::Error::HostKeyUnknown {
            host: "bastion.internal".to_owned(),
            port: 22,
            path: PathBuf::from("/home/u/.ssh/known_hosts"),
            key,
            covered_by_certificate_authority: false,
        });
        // Everything the person needs is in the one message the protocol carries.
        assert!(
            message.contains("SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog"),
            "{message}"
        );
        assert!(message.contains("bastion.internal:22"), "{message}");
        assert!(message.contains("/home/u/.ssh/known_hosts"), "{message}");
        assert!(message.contains("ssh-keyscan"), "{message}");
        assert!(message.contains("SSH_KNOWN_HOSTS"), "{message}");
    }

    #[test]
    fn the_certificate_authority_case_says_so() {
        let key = qh_tunnel::ServerKey::from_blob(base64_decode(
            "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ",
        ))
        .expect("a valid blob");
        let message = describe(&qh_tunnel::Error::HostKeyUnknown {
            host: "bastion.internal".to_owned(),
            port: 22,
            path: PathBuf::from("known_hosts"),
            key,
            covered_by_certificate_authority: true,
        });
        assert!(message.contains("@cert-authority"), "{message}");
    }

    fn key_blob(seed: u8) -> qh_tunnel::ServerKey {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[seed; 32]);
        qh_tunnel::ServerKey::from_blob(blob).expect("a valid ed25519 blob")
    }

    fn bastion_description() -> TunnelConfig {
        settings(&with(&[
            ("SSH_HOST", "bastion.corp"),
            ("SSH_USER", "deploy"),
            ("SSH_APP_KNOWN_HOSTS", "/app/known_hosts"),
        ]))
        .unwrap()
        .expect("a tunnel")
    }

    const PIN: &str = "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog";

    #[test]
    fn the_new_settings_map_and_default_to_nothing() {
        let plain = bastion_description();
        assert_eq!(
            plain.app_known_hosts,
            Some(PathBuf::from("/app/known_hosts"))
        );
        assert_eq!(plain.host_key_accept, None);
        assert_eq!(plain.alias, None);

        let pinned = settings(&with(&[
            ("SSH_HOST", "bastion.corp"),
            ("SSH_USER", "deploy"),
            ("SSH_APP_KNOWN_HOSTS", "/app/known_hosts"),
            ("SSH_HOST_KEY_ACCEPT", PIN),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!(pinned.host_key_accept.as_deref(), Some(PIN));
    }

    #[test]
    fn a_pin_must_have_the_shape_and_a_file_to_be_recorded_in_and_is_never_echoed() {
        let base = [("SSH_HOST", "bastion.corp"), ("SSH_USER", "deploy")];
        for bad in [
            "abc",
            "SHA256:short",
            "MD5:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog",
            "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuo!",
        ] {
            let error = settings(&with(&[
                base[0],
                base[1],
                ("SSH_APP_KNOWN_HOSTS", "/a"),
                ("SSH_HOST_KEY_ACCEPT", bad),
            ]))
            .unwrap_err();
            assert_eq!(error, TunnelConfigError::BadHostKeyAccept, "{bad}");
            assert!(!error.to_string().contains(bad), "{error}");
        }
        // A well-formed pin with nowhere to record the key is refused before the network.
        let error = settings(&with(&[base[0], base[1], ("SSH_HOST_KEY_ACCEPT", PIN)])).unwrap_err();
        assert_eq!(error, TunnelConfigError::PinNeedsAppFile);
    }

    #[test]
    fn a_host_that_could_inject_into_a_command_or_a_known_hosts_line_is_refused_unechoed() {
        for hostile in [
            "bad host",
            "a'b",
            "a,b",
            "a*b",
            "-oProxyCommand=x",
            "a\nb",
            "",
            "a;b",
            "a$(b)",
        ] {
            let result = settings(&with(&[("SSH_HOST", hostile), ("SSH_USER", "deploy")]));
            if hostile.trim().is_empty() {
                assert_eq!(result.unwrap(), None);
                continue;
            }
            let error = result.unwrap_err();
            assert_eq!(error, TunnelConfigError::BadHost, "{hostile:?}");
            assert!(!error.to_string().contains(hostile), "{error}");
        }
        // Names, addresses and IPv6 literals pass.
        for fine in [
            "bastion.corp",
            "10.0.0.5",
            "bastion_1.corp-x",
            "::1",
            "fe80::1",
        ] {
            assert!(
                settings(&with(&[("SSH_HOST", fine), ("SSH_USER", "d")]))
                    .unwrap()
                    .is_some(),
                "{fine}"
            );
        }
    }

    fn config_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config");
        std::fs::write(&path, text).expect("write");
        (dir, path)
    }

    #[test]
    fn an_alias_fills_in_what_the_settings_leave_blank_and_explicit_settings_win() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = dir.path().join("id_alias");
        std::fs::write(&key, "k").unwrap();
        let (_keep, path) = config_file(&format!(
            "Host qh-dev\n  HostName 127.0.0.1\n  Port 52222\n  User qh\n  IdentityFile /missing/id\n  IdentityFile {}\n",
            key.display()
        ));
        let path_text = path.to_string_lossy().into_owned();
        let base = [
            ("SSH_HOST", "qh-dev"),
            ("SSH_USE_CONFIG", "1"),
            ("SSH_CONFIG_PATH", path_text.as_str()),
        ];

        let config = settings(&with(&base)).unwrap().expect("a tunnel");
        assert_eq!(
            config.host, "127.0.0.1",
            "the host is the resolved HostName"
        );
        assert_eq!(config.alias.as_deref(), Some("qh-dev"));
        assert_eq!((config.port, config.user.as_str()), (52222, "qh"));
        // `SSH_AUTH_METHOD` is never taken from the config.
        assert_eq!(config.auth, TunnelAuth::Agent);

        // `key` with no `SSH_KEY_PATH` takes the first IdentityFile that exists on disk.
        let with_key = settings(&with(&[
            base[0],
            base[1],
            base[2],
            ("SSH_AUTH_METHOD", "key"),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!(with_key.auth, TunnelAuth::Key(key));

        // Explicit settings beat the config.
        let explicit = settings(&with(&[
            base[0],
            base[1],
            base[2],
            ("SSH_USER", "other"),
            ("SSH_PORT", "2200"),
        ]))
        .unwrap()
        .expect("a tunnel");
        assert_eq!((explicit.port, explicit.user.as_str()), (2200, "other"));
    }

    #[test]
    fn an_alias_that_does_not_exist_or_a_config_that_cannot_be_honoured_is_refused_by_name() {
        let (_keep, path) = config_file("Host known\n  HostName 10.0.0.1\n");
        let path_text = path.to_string_lossy().into_owned();
        let error = settings(&with(&[
            ("SSH_HOST", "typo"),
            ("SSH_USER", "d"),
            ("SSH_USE_CONFIG", "1"),
            ("SSH_CONFIG_PATH", path_text.as_str()),
        ]))
        .unwrap_err();
        // A typo does not quietly become a host name.
        assert!(
            matches!(error, TunnelConfigError::SshConfig(_)),
            "{error:?}"
        );
        assert!(error.to_string().contains("typo"), "{error}");

        // Without the flag the same name is a literal host, as before.
        assert!(settings(&with(&[("SSH_HOST", "typo"), ("SSH_USER", "d")]))
            .unwrap()
            .is_some());

        // A resolved HostName is checked like any other host.
        let (_keep, path) = config_file("Host evil\n  HostName a'b\n");
        let error = settings(&with(&[
            ("SSH_HOST", "evil"),
            ("SSH_USER", "d"),
            ("SSH_USE_CONFIG", "1"),
            ("SSH_CONFIG_PATH", &path.to_string_lossy()),
        ]))
        .unwrap_err();
        assert_eq!(error, TunnelConfigError::BadHost);
    }

    #[test]
    fn the_alias_list_and_the_resolution_read_the_same_file() {
        let (_keep, path) =
            config_file("Host a b\n  HostName 10.0.0.1\nHost *\n  User x\nHost !c\n");
        assert_eq!(alias_list(&path), ["a", "b"]);
        assert_eq!(resolve_alias(&path, "a").unwrap().host_name, "10.0.0.1");
        // Listing never fails.
        assert!(alias_list(std::path::Path::new("/nonexistent/dir/config")).is_empty());
    }

    #[test]
    fn the_bastion_reads_the_system_file_records_in_the_app_file_and_pins_only_when_asked() {
        let mut description = bastion_description();
        let plain = bastion(&description, &with(&[])).unwrap();
        assert_eq!(
            plain.record_to.as_deref(),
            Some(Path::new("/app/known_hosts"))
        );
        assert_eq!(
            plain.known_hosts.last().map(PathBuf::as_path),
            Some(Path::new("/etc/ssh/ssh_known_hosts"))
        );
        assert!(matches!(
            plain.host_key_policy,
            qh_tunnel::HostKeyPolicy::Strict
        ));

        description.host_key_accept = Some(PIN.to_owned());
        let pinned = bastion(&description, &with(&[])).unwrap();
        assert!(
            matches!(pinned.host_key_policy, qh_tunnel::HostKeyPolicy::TrustFingerprint(ref p) if p == PIN)
        );

        // The trust list is what the pool key is built from.
        description.known_hosts = Some(PathBuf::from("/u/kh"));
        assert_eq!(
            trust_files(&description),
            [
                PathBuf::from("/u/kh"),
                PathBuf::from("/etc/ssh/ssh_known_hosts")
            ]
        );
    }

    #[test]
    fn every_host_key_refusal_is_structured_and_never_retried() {
        use qh_tunnel::Error as E;
        let description = bastion_description();
        let presented = key_blob(1);

        let unknown = E::HostKeyUnknown {
            host: "bastion.corp".into(),
            port: 22,
            path: PathBuf::from("/u/kh"),
            key: presented.clone(),
            covered_by_certificate_authority: true,
        };
        let cases: Vec<(E, HostKeyState)> = vec![
            (unknown, HostKeyState::Unknown),
            (
                E::HostKeyMismatch {
                    host: "bastion.corp".into(),
                    port: 22,
                    path: PathBuf::from("/u/kh"),
                    key: presented.clone(),
                    recorded: Vec::new(),
                },
                HostKeyState::Changed,
            ),
            (
                E::HostKeyRevoked {
                    host: "bastion.corp".into(),
                    port: 22,
                    path: PathBuf::from("/u/kh"),
                    line: 4,
                },
                HostKeyState::Revoked,
            ),
            (
                E::HostCertificateUnsupported {
                    host: "bastion.corp".into(),
                    port: 22,
                    fingerprint: PIN.into(),
                },
                HostKeyState::Certificate,
            ),
            (
                E::HostKeyCertificateExpected {
                    host: "bastion.corp".into(),
                    port: 22,
                    key: presented.clone(),
                    path: PathBuf::from("/u/kh"),
                },
                HostKeyState::CertificateExpected,
            ),
            (
                E::HostKeyPinMismatch {
                    host: "bastion.corp".into(),
                    port: 22,
                    key: presented.clone(),
                    presented: presented.fingerprint(),
                    pinned: PIN.into(),
                },
                HostKeyState::PinMismatch,
            ),
            (
                E::HostKeyRecordFailed {
                    host: "bastion.corp".into(),
                    port: 22,
                    key: presented.clone(),
                    path: PathBuf::from("/app/known_hosts"),
                    reason: "disk full".into(),
                },
                HostKeyState::RecordFailed,
            ),
            (
                E::HostKeyStoreUnsafe {
                    path: PathBuf::from("/app/known_hosts"),
                    reason: "is a symlink",
                },
                HostKeyState::StoreUnsafe,
            ),
        ];
        for (error, state) in cases {
            let engine = refused(&error, &description);
            let failure = engine
                .host_key()
                .unwrap_or_else(|| panic!("{error:?} is a host key refusal"));
            assert_eq!(failure.state, state, "{error:?}");
            assert_eq!(engine.failure_kind(), FailureKind::Permanent);
            assert_eq!(failure.message, describe(&error));
            assert_eq!(failure.alias, None);
            assert_eq!(failure.app_known_hosts.as_deref(), Some("/app/known_hosts"));
        }

        // Details a sheet needs, on the two states that carry them.
        let unknown = host_key_failure(
            &E::HostKeyUnknown {
                host: "h".into(),
                port: 2222,
                path: PathBuf::new(),
                key: presented.clone(),
                covered_by_certificate_authority: false,
            },
            &description,
        )
        .unwrap();
        assert_eq!((unknown.host.as_str(), unknown.port), ("h", 2222));
        assert_eq!(unknown.key_type.as_deref(), Some("ssh-ed25519"));
        assert_eq!(unknown.fingerprint, Some(presented.fingerprint()));
        assert!(!unknown.ca_covered);
        let pin = host_key_failure(
            &E::HostKeyPinMismatch {
                host: "h".into(),
                port: 22,
                key: presented.clone(),
                presented: presented.fingerprint(),
                pinned: PIN.into(),
            },
            &description,
        )
        .unwrap();
        assert_eq!(pin.pinned.as_deref(), Some(PIN));
    }

    #[test]
    fn the_non_prompt_failures_stay_plain_connect_errors() {
        let description = bastion_description();
        // Nothing was checked: a fault, not a decision for a person.
        let engine = refused(&qh_tunnel::Error::HostKeyNotVerified, &description);
        assert!(engine.host_key().is_none());
        assert!(matches!(
            engine,
            EngineError::Connect {
                kind: FailureKind::Permanent,
                ..
            }
        ));
        let engine = refused(
            &qh_tunnel::Error::AuthenticationRejected {
                user: "u".into(),
                methods: "publickey".into(),
            },
            &description,
        );
        assert!(matches!(
            engine,
            EngineError::Connect {
                kind: FailureKind::Permanent,
                ..
            }
        ));
    }

    #[test]
    fn a_mismatch_lists_every_record_with_its_file_line_and_source() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let (user, app) = (dir.path().join("user"), dir.path().join("app"));
        let line = |key: &qh_tunnel::ServerKey| {
            use base64::Engine as _;
            format!(
                "bastion.corp ssh-ed25519 {}\n",
                base64::engine::general_purpose::STANDARD.encode(key.blob())
            )
        };
        std::fs::write(&user, line(&key_blob(2))).unwrap();
        std::fs::write(&app, line(&key_blob(3))).unwrap();
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o600)).unwrap();
        let files = [
            known_hosts::StoreFile::new(&user, Origin::User),
            known_hosts::StoreFile::new(&app, Origin::App),
        ];
        let presented = key_blob(1);
        let known_hosts::HostKeyVerdict::Mismatch { recorded } =
            known_hosts::check_all(&files, "bastion.corp", 22, presented.blob()).unwrap()
        else {
            panic!("expected a mismatch");
        };
        let failure = host_key_failure(
            &qh_tunnel::Error::HostKeyMismatch {
                host: "bastion.corp".into(),
                port: 22,
                path: user.clone(),
                key: presented,
                recorded,
            },
            &bastion_description(),
        )
        .unwrap();
        let sources: Vec<_> = failure
            .recorded
            .iter()
            .map(|r| (r.source.as_str(), r.line))
            .collect();
        assert_eq!(sources, [("user", 1), ("app", 1)]);
        assert_eq!(failure.recorded[0].path, user.display().to_string());
    }

    #[test]
    fn a_tls_name_follows_the_connection_through_the_tunnel_where_it_can() {
        use qh_driver::DriverKind;
        let named = |kind, host: &str| {
            let mut config = ConnectionConfig::new(kind, host, 5432, "app");
            retarget(&mut config, 44_100);
            config
        };
        let postgres = named(DriverKind::Postgres, "db.internal");
        assert_eq!(postgres.tls_server_name.as_deref(), Some("db.internal"));
        assert_eq!(
            (postgres.host.as_str(), postgres.port),
            ("127.0.0.1", 44_100)
        );
        assert_eq!(
            named(DriverKind::Trino, "trino.internal")
                .tls_server_name
                .as_deref(),
            Some("trino.internal")
        );
        // MySQL cannot take a name.
        assert_eq!(
            named(DriverKind::Mysql, "db.internal").tls_server_name,
            None
        );
        // A literal address cannot be redirected by name: naming it would bypass the tunnel on
        // Trino, so it stays unset there. PostgreSQL's `hostaddr` split has no such problem.
        assert_eq!(named(DriverKind::Trino, "10.0.0.5").tls_server_name, None);
        assert_eq!(
            named(DriverKind::Postgres, "10.0.0.5")
                .tls_server_name
                .as_deref(),
            Some("10.0.0.5")
        );
    }

    /// The standard base64 the `.pub` body is written in, without pulling in a
    /// dependency for one test vector.
    fn base64_decode(text: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .expect("the test vector is base64")
    }
}
