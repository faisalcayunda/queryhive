//! The SSH bastion a connection goes through, from settings to an open tunnel.
//!
//! The blueprint's placement, taken literally: `qh-tunnel` is the crate a
//! connection reaches when a bastion is configured (§3.2 line 618), built in phase 1
//! "before the UI touches it" (§7 line 1037), and it is part of *connecting* — there
//! is no tunnel command, and none was added. This module is the engine half of that:
//! it turns `SSH_*` settings into [`TunnelConfig`] ([`settings`]), and — in
//! [`crate::RealEngine::connect`] — opens the tunnel and hands the driver the
//! loopback endpoint it listens on.
//!
//! # The `SSH_*` vocabulary, and why these names
//!
//! Checked first, per the engine's own rule that existing vocabulary wins
//! (`config.rs`'s `DB_*`/`TRINO_*` aliases): neither the Python engine
//! (`app/engine/queryhive_engine.py`) nor the app defines any SSH setting today —
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
//!
//! # What an unknown host does here, and why
//!
//! The security rule of `qh-tunnel` is kept whole: the host key is checked during
//! the key exchange, before anything authenticating is sent, and trust-on-first-use
//! is two calls with a person's decision between them — never a silent accept. The
//! NDJSON event protocol is one-way today (an engine process emits events; there is
//! no channel back, blueprint §1.2: "Tidak ada kanal balik"), so the decision
//! cannot round-trip inside this engine. The honest minimal behaviour, then, is the
//! strict one: an unknown host is refused with a `connect` error whose message
//! carries the fingerprint and the `known_hosts` path, telling the user to add the
//! key out of band (`ssh-keyscan`, or connecting once with `ssh` itself) and to
//! point `SSH_KNOWN_HOSTS` at a file they control. The interactive TOFU prompt —
//! emit an event the app answers, then retry with
//! `qh_tunnel::HostKeyPolicy::TrustNew` — is app-side work on top of this.

use std::path::PathBuf;

use qh_driver::{ConnectionConfig, TunnelAuth, TunnelConfig};
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

    #[error(transparent)]
    Setting(#[from] SettingError),
}

/// The [`TunnelConfig`] the settings describe, or `None` when `SSH_HOST` is absent.
///
/// `SSH_HOST` is the single switch: it is the one value without which there is no
/// bastion at all, so a blank or missing one means "no tunnel" whatever the other
/// `SSH_*` settings say. That mirrors how `DB_HOST` decides there is a connection.
pub fn settings(settings: &Settings) -> Result<Option<TunnelConfig>, TunnelConfigError> {
    let host = settings.text("SSH_HOST", "");
    if host.is_empty() {
        return Ok(None);
    }

    let user = settings.text("SSH_USER", "");
    if user.is_empty() {
        return Err(TunnelConfigError::Missing("SSH_USER"));
    }

    let port = settings
        .number("SSH_PORT", i64::from(TunnelConfig::DEFAULT_PORT))
        .map_err(TunnelConfigError::Setting)?;
    let port = u16::try_from(port).map_err(|_| SettingError::NotANumber {
        key: "SSH_PORT".to_owned(),
        value: port.to_string(),
    })?;

    let auth = match settings.text("SSH_AUTH_METHOD", "agent").as_str() {
        "agent" => TunnelAuth::Agent,
        "key" => {
            let path = settings.text("SSH_KEY_PATH", "");
            if path.is_empty() {
                return Err(TunnelConfigError::KeyPathMissing);
            }
            TunnelAuth::Key(PathBuf::from(path))
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

    let known_hosts = match settings.text("SSH_KNOWN_HOSTS", "") {
        path if !path.is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };

    Ok(Some(TunnelConfig {
        host,
        port,
        user,
        auth,
        known_hosts,
    }))
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
        None => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".ssh").join("known_hosts"))
            .ok_or(TunnelConfigError::NoKnownHosts),
    }
}

/// Point the connection at the tunnel's loopback endpoint.
///
/// The tunnel forwards to the database host and port as configured, so the target
/// is read off the config *before* it is rewritten — the one order that cannot
/// disagree.
pub fn retarget(config: &mut ConnectionConfig, local_port: u16) -> qh_tunnel::Target {
    let target = qh_tunnel::Target::new(config.host.clone(), config.port);
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
    Ok(qh_tunnel::BastionConfig::new(
        config.host.clone(),
        config.port,
        config.user.clone(),
        auth,
        known_hosts_path(config)?,
    ))
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

    /// The standard base64 the `.pub` body is written in, without pulling in a
    /// dependency for one test vector.
    fn base64_decode(text: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .expect("the test vector is base64")
    }
}
