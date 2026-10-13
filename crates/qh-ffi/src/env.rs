//! Settings, and the readers that turn text into values.
//!
//! Every setting comes from the environment and nothing is read from disk: that is
//! the whole configuration story of this engine, inherited unchanged from the removed
//! `queryhive_engine.py`'s `settings()`. The readers below are its `_raw`, `_value`,
//! `_int` and `_flag`, and each one exists for a reason worth keeping:
//!
//! - [`Settings::raw`] does **not** strip, because a padding space can be part of a
//!   password or a delimiter.
//! - [`Settings::text`] strips and treats blank as unset, because the app sends
//!   blank fields for settings the user cleared.
//! - [`Settings::flag`] accepts only the documented spellings and keeps the default
//!   for anything else, so a typo cannot silently turn a feature on.
//!
//! A test injects a map instead of the process environment, which is how the golden
//! harness replays Python's own cases.

use std::collections::BTreeMap;

use qh_sql::{SafeMode, SAFE_MODES};
use thiserror::Error;

/// One setting that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SettingError {
    /// The Python engine's wording, quotes included, so a user who has seen one of
    /// these messages recognises the next one.
    #[error("{key} must be a whole number, got '{value}'")]
    NotANumber { key: String, value: String },

    /// A whole number was read, and it was negative.
    ///
    /// Its own variant because it is a different mistake from a typo: the caller
    /// typed a number, and the number means the opposite of what the setting can
    /// be. Refused by name rather than floored, so a bound cannot silently become
    /// no bound.
    #[error("{key} cannot be negative, got '{value}'")]
    Negative { key: String, value: i64 },
}

/// The settings whose values are credentials, and so never print.
///
/// One list, pinned by a test, that [`Settings`]'s `Debug` reads. `DB_URL` and `TRINO_URL` are
/// here because a connection URL can carry the password (`user:secret@host`).
pub const SECRET_KEYS: [&str; 7] = [
    "DB_PASSWORD",
    "TRINO_PASSWORD",
    "DB_URL",
    "TRINO_URL",
    "DB_JWT",
    "SSH_PASSWORD",
    "SSH_KEY_PASSPHRASE",
];

/// The prefix a second connection's settings carry (`DEST_DB_HOST`, `DEST_SAFE_MODE`, …).
///
/// A transfer reads one query on a source connection and writes a table on another, so it
/// needs a second copy of every connection key. The source keeps the bare names it has
/// always had and the destination gets this prefix; see [`Settings::prefixed`].
pub const DEST_PREFIX: &str = "DEST_";

/// Whether the value under `key` is a credential: one of [`SECRET_KEYS`], bare or carrying
/// [`DEST_PREFIX`].
///
/// The single place that decides, because `DEST_DB_PASSWORD` is exactly as secret as
/// `DB_PASSWORD` and an exact match on the bare names alone would print it. The prefix is
/// stripped in a loop, so a key that was prefixed twice by mistake is still redacted
/// rather than being the one spelling that prints.
pub fn is_secret_key(key: &str) -> bool {
    let mut bare = key;
    while let Some(rest) = bare.strip_prefix(DEST_PREFIX) {
        bare = rest;
    }
    SECRET_KEYS.contains(&bare)
}

/// The settings a prefixed view takes from the top level because they are not about any
/// one connection.
const SHARED_KEYS: [&str; 2] = ["DB_PATH", "PROGRESS_MS"];

/// Every setting this engine reads.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Settings {
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for Settings {
    /// The keys, with every credential's value replaced by `<redacted>`.
    ///
    /// A derived `Debug` would print a password, a bearer token or a key passphrase into any log
    /// or panic message that formats a `Settings` (blueprint W11 §4, hazard §1.5).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut map = formatter.debug_map();
        for (key, value) in &self.values {
            if is_secret_key(key) {
                map.entry(key, &"<redacted>");
            } else {
                map.entry(key, value);
            }
        }
        map.finish()
    }
}

impl Settings {
    /// The process environment, which is what production uses.
    pub fn from_env() -> Self {
        Self::from_pairs(std::env::vars())
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Whether the key is present at all, blank or not.
    ///
    /// Different from [`Settings::text`] being empty, and it has to be: `WRITE_MODE`
    /// set to `""` is a mode the app named and cannot be defaulted away, while an
    /// absent key means "create".
    pub fn is_set(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// The value verbatim; an empty value is treated as unset.
    pub fn raw(&self, key: &str, default: &str) -> String {
        match self.get(key) {
            None | Some("") => default.to_owned(),
            Some(value) => value.to_owned(),
        }
    }

    /// The value stripped, with blank meaning "unset".
    pub fn text(&self, key: &str, default: &str) -> String {
        let value = self.raw(key, "").trim().to_owned();
        if value.is_empty() {
            default.to_owned()
        } else {
            value
        }
    }

    /// A whole number, or a refusal naming the setting the caller actually set.
    pub fn number(&self, key: &str, default: i64) -> Result<i64, SettingError> {
        let raw = self.text(key, "");
        if raw.is_empty() {
            return Ok(default);
        }
        raw.parse::<i64>().map_err(|_| SettingError::NotANumber {
            key: key.to_owned(),
            value: raw,
        })
    }

    /// A whole number that must not be negative.
    ///
    /// `0` is a real value and not an absence: `STATEMENT_TIMEOUT_MS=0` means "no
    /// bound", which is the engine's default. A negative number is refused by name
    /// rather than floored, because flooring it would turn the bound the caller
    /// asked for into no bound at all — the opposite of what a safety setting is
    /// for.
    pub fn non_negative(&self, key: &str, default: i64) -> Result<i64, SettingError> {
        let value = self.number(key, default)?;
        if value < 0 {
            return Err(SettingError::Negative {
                key: key.to_owned(),
                value,
            });
        }
        Ok(value)
    }

    /// Whether any key starts with `prefix`, whatever its value, blank included.
    ///
    /// The question "was a destination asked for" cannot be asked of [`Settings::text`],
    /// which reads a blank as nothing: an app that sent `DEST_DB_KIND=""` asked for a
    /// destination it did not describe, and that has to be an error rather than the
    /// single-connection command.
    pub fn has_prefixed(&self, prefix: &str) -> bool {
        self.values.keys().any(|key| key.starts_with(prefix))
    }

    /// The settings of the connection named by `prefix`, with the prefix taken off.
    ///
    /// Only the keys that carry the prefix come across, plus the two that are not about a
    /// connection at all: `DB_PATH` (where the execution log and the local database live)
    /// and `PROGRESS_MS`. Those two are read from the top level **only**, so a
    /// `DEST_DB_PATH` cannot send the audit log somewhere else.
    ///
    /// Nothing else is inherited. In particular not `SAFE_MODE`, `SAFE_MODE_FLOOR`,
    /// `DB_READ_ONLY`, a password or a key passphrase: a destination that was never given
    /// a credential must fail to authenticate rather than borrow the source's, and one
    /// that was never given a Safe Mode must not exist at all (the caller refuses it).
    /// `SAFE_MODE_FLOOR` is the one a naive copy would get wrong, because the stripped
    /// `DEST_SAFE_MODE_FLOOR` lands on the same key and one would silently overwrite the
    /// other; the caller raises the top-level floor as its own entry instead.
    pub fn prefixed(&self, prefix: &str) -> Settings {
        let mut values: BTreeMap<String, String> = self
            .values
            .iter()
            .filter_map(|(key, value)| {
                let bare = key.strip_prefix(prefix)?;
                (!bare.is_empty()).then(|| (bare.to_owned(), value.clone()))
            })
            .collect();
        for shared in SHARED_KEYS {
            values.remove(shared);
            if let Some(value) = self.values.get(shared) {
                values.insert(shared.to_owned(), value.clone());
            }
        }
        Settings { values }
    }

    /// Everything except the keys that carry `prefix`: the settings of the connection that
    /// has no prefix, with the other one taken out.
    ///
    /// What the source side reads, so a destination key can never be mistaken for one of
    /// its own (and so a `DEST_` secret is not in the map that side holds).
    pub fn without_prefix(&self, prefix: &str) -> Settings {
        Settings {
            values: self
                .values
                .iter()
                .filter(|(key, _)| !key.starts_with(prefix))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }

    /// A boolean setting: `1/true/yes/on` and `0/false/no/off`, case-insensitive.
    ///
    /// A blank value, or anything else unrecognised, keeps `default` — so only the
    /// documented spellings ever change what the caller asked for.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        parse_flag(&self.text(key, ""), default)
    }
}

/// How a transfer treats a table that already exists. `append` is not among them: it is
/// not offered across connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Create,
    Replace,
}

/// A request to write to a second connection, split into the two connections' settings.
///
/// Built by [`Settings::destination`] and nowhere else, which is what makes "the source
/// never sees a `DEST_` key, and the destination never inherits a source one" a property
/// of one function.
#[derive(Debug, Clone)]
pub struct Destination {
    /// What the source side reads: every setting that does not carry [`DEST_PREFIX`].
    pub source: Settings,
    /// What the destination side reads: [`Settings::prefixed`] with [`DEST_PREFIX`].
    pub dest: Settings,
    pub mode: TransferMode,
    /// The destination's own `DEST_SAFE_MODE`, as the caller spelled it. The levels the
    /// floor adds to it are the transfer's to resolve.
    pub safe_mode: SafeMode,
}

/// Why a destination cannot be used. Always decided from the settings alone, before the
/// network is touched, and the messages never repeat a value that could be a credential.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DestinationError {
    #[error("writing to another connection needs DEST_DB_KIND or DEST_DB_URL, and neither is set")]
    NoConnection,

    /// Missing, blank or only spaces. There is no default: the source's `SAFE_MODE` is the
    /// source's, and a destination that silently ran as `full` would be the one level that
    /// asks for nothing.
    #[error(
        "writing to another connection needs DEST_SAFE_MODE (one of {}); the source's \
         SAFE_MODE does not apply to it",
        SAFE_MODES.join(", ")
    )]
    NoSafeMode,

    #[error(
        "unknown DEST_SAFE_MODE '{0}'; expected {modes}",
        modes = SAFE_MODES.join(", ")
    )]
    UnknownSafeMode(String),

    #[error("WRITE_MODE=append is not offered across connections; expected create or replace")]
    Append,

    #[error("unknown WRITE_MODE '{0}'; expected create or replace")]
    UnknownWriteMode(String),
}

impl Settings {
    /// Whether these settings ask to write to a second connection, and if so whether they
    /// describe one the engine can use.
    ///
    /// `Ok(None)` only when **no key at all** starts with [`DEST_PREFIX`]. One such key,
    /// blank or not, is a request: whatever else is wrong with it is an error here, so
    /// the one-connection command can never run by default on the strength of a partial
    /// set. What is checked is exactly what needs no network:
    ///
    /// - `DEST_DB_KIND` or `DEST_DB_URL`, at least one non-blank;
    /// - `DEST_SAFE_MODE`, present, non-blank and one of the four spellings;
    /// - `WRITE_MODE`, absent (create), `create` or `replace`. A blank is a mode nobody
    ///   can name, as in the one-connection command.
    ///
    /// Whether the kind is one the engine can write to, and the connection itself, are
    /// for the caller to resolve from [`Destination::dest`].
    pub fn destination(&self) -> Result<Option<Destination>, DestinationError> {
        if !self.has_prefixed(DEST_PREFIX) {
            return Ok(None);
        }
        let dest = self.prefixed(DEST_PREFIX);
        if dest.text("DB_KIND", "").is_empty() && dest.text("DB_URL", "").is_empty() {
            return Err(DestinationError::NoConnection);
        }
        let raw_safe = dest.text("SAFE_MODE", "");
        if raw_safe.is_empty() {
            return Err(DestinationError::NoSafeMode);
        }
        let safe_mode =
            SafeMode::parse(&raw_safe).ok_or(DestinationError::UnknownSafeMode(raw_safe))?;
        let mode = match self.get("WRITE_MODE") {
            None => TransferMode::Create,
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "create" => TransferMode::Create,
                "replace" => TransferMode::Replace,
                "append" => return Err(DestinationError::Append),
                other => return Err(DestinationError::UnknownWriteMode(other.to_owned())),
            },
        };
        Ok(Some(Destination {
            source: self.without_prefix(DEST_PREFIX),
            dest,
            mode,
            safe_mode,
        }))
    }
}

/// The flag rule, for a value that came from somewhere other than a keyed setting
/// (the `TRINO_INSECURE` alias is read this way).
pub fn parse_flag(raw: &str, default: bool) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "0" | "false" | "no" | "off" => false,
        "1" | "true" | "yes" | "on" => true,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        Settings::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn debug_never_prints_a_credential() {
        let env = settings(&[
            ("DB_PASSWORD", "pw-SENTINEL"),
            ("TRINO_PASSWORD", "tpw-SENTINEL"),
            ("DB_URL", "postgres://u:url-SENTINEL@h/db"),
            ("TRINO_URL", "trino://u:turl-SENTINEL@h"),
            ("DB_JWT", "jwt-SENTINEL"),
            ("SSH_PASSWORD", "ssh-SENTINEL"),
            ("SSH_KEY_PASSPHRASE", "phrase-SENTINEL"),
            ("DB_HOST", "db.internal"),
        ]);
        let printed = format!("{env:?} {env:#?}");
        assert!(!printed.contains("SENTINEL"), "{printed}");
        assert_eq!(printed.matches("<redacted>").count(), 2 * SECRET_KEYS.len());
        // Everything else still prints, so a log line stays useful.
        assert!(printed.contains("db.internal"), "{printed}");
        assert!(printed.contains("DB_PASSWORD"), "{printed}");
    }

    #[test]
    fn debug_never_prints_a_destination_credential() {
        // Every secret key, under the destination prefix, alone and prefixed twice by a
        // caller's mistake, in both the compact and the pretty form.
        for key in SECRET_KEYS {
            for prefix in ["DEST_", "DEST_DEST_"] {
                let name = format!("{prefix}{key}");
                let value = format!("{name}-SENTINEL");
                let env = settings(&[
                    (name.as_str(), value.as_str()),
                    ("DEST_DB_HOST", "b.internal"),
                ]);
                let printed = format!("{env:?} {env:#?}");
                assert!(!printed.contains("SENTINEL"), "{name} printed: {printed}");
                assert_eq!(
                    printed.matches("<redacted>").count(),
                    2,
                    "{name}: {printed}"
                );
                // The key stays, so a log line says which secret was set; the host is not one.
                assert!(printed.contains(&name), "{printed}");
                assert!(printed.contains("b.internal"), "{printed}");
            }
        }
        // The secret list itself is the only authority: no key outside it is redacted by
        // the prefix alone, so a log line about a destination stays readable.
        for key in [
            "DEST_DB_HOST",
            "DEST_DB_USER",
            "DEST_SAFE_MODE",
            "DEST_SSH_HOST",
            "DEST_",
        ] {
            assert!(!is_secret_key(key), "{key}");
        }
        for key in SECRET_KEYS {
            assert!(is_secret_key(key));
            assert!(is_secret_key(&format!("DEST_{key}")));
            // A lookalike that merely contains the name is not it.
            assert!(!is_secret_key(&format!("X_{key}")));
            assert!(!is_secret_key(&format!("DEST_{key}_PATH")));
        }
    }

    #[test]
    fn the_secret_list_is_pinned_and_covers_what_the_connection_readers_take_raw() {
        assert_eq!(
            SECRET_KEYS,
            [
                "DB_PASSWORD",
                "TRINO_PASSWORD",
                "DB_URL",
                "TRINO_URL",
                "DB_JWT",
                "SSH_PASSWORD",
                "SSH_KEY_PASSPHRASE"
            ]
        );
        // `Settings::raw` is the reader that keeps a secret byte for byte. Every key the
        // connection and tunnel builders read that way has to be on the list, so a new secret
        // setting cannot be added without being redacted.
        for (name, source) in [
            ("config.rs", include_str!("config.rs")),
            ("tunnel.rs", include_str!("tunnel.rs")),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            for (index, _) in production.match_indices(".raw(\"") {
                let key = production[index + 6..].split('"').next().unwrap();
                assert!(
                    SECRET_KEYS.contains(&key),
                    "{name} reads {key} with `raw` but it is not in SECRET_KEYS"
                );
            }
        }
    }

    #[test]
    fn raw_keeps_a_padding_space_and_text_drops_it() {
        let env = settings(&[("DELIMITER", " "), ("NAME", "  padded  ")]);
        assert_eq!(env.raw("DELIMITER", ","), " ");
        assert_eq!(env.text("NAME", "x"), "padded");
    }

    #[test]
    fn a_blank_value_is_unset_but_still_present() {
        let env = settings(&[("WRITE_MODE", ""), ("OTHER", "")]);
        assert_eq!(env.text("WRITE_MODE", "create"), "create");
        // Present and blank, which is not the same as absent: the write mode depends on
        // the difference, because an app that sent "" named a mode it cannot have.
        assert!(env.is_set("WRITE_MODE"));
        assert!(env.is_set("OTHER"));
        assert!(!env.is_set("ABSENT"));
    }

    #[test]
    fn a_number_is_read_or_refused_by_name() {
        let env = settings(&[("BATCH_SIZE", "500"), ("PORT", "not-a-number")]);
        assert_eq!(env.number("BATCH_SIZE", 10).unwrap(), 500);
        assert_eq!(env.number("ABSENT", 10).unwrap(), 10);
        let error = env.number("PORT", 0).unwrap_err();
        assert_eq!(
            error.to_string(),
            "PORT must be a whole number, got 'not-a-number'"
        );
    }

    #[test]
    fn a_non_negative_setting_refuses_a_negative_one_by_name() {
        let env = settings(&[
            ("STATEMENT_TIMEOUT_MS", "60000"),
            ("ZERO", "0"),
            ("NEGATIVE", "-1"),
            ("BROKEN", "soon"),
        ]);
        assert_eq!(env.non_negative("STATEMENT_TIMEOUT_MS", 0).unwrap(), 60_000);
        // Zero is a value, not an absence: it is how "no bound" is spelled.
        assert_eq!(env.non_negative("ZERO", 99).unwrap(), 0);
        assert_eq!(env.non_negative("ABSENT", 99).unwrap(), 99);
        assert_eq!(
            env.non_negative("NEGATIVE", 0).unwrap_err().to_string(),
            "NEGATIVE cannot be negative, got '-1'"
        );
        // A typo is still the number reader's refusal.
        assert_eq!(
            env.non_negative("BROKEN", 0).unwrap_err().to_string(),
            "BROKEN must be a whole number, got 'soon'"
        );
    }

    fn keys(env: &Settings) -> Vec<&str> {
        env.values.keys().map(String::as_str).collect()
    }

    #[test]
    fn a_dest_setting_never_reaches_the_source_and_back() {
        let env = settings(&[
            ("DB_HOST", "a.internal"),
            ("DB_PASSWORD", "a-secret"),
            ("SSH_KEY_PASSPHRASE", "a-phrase"),
            ("SAFE_MODE", "full"),
            ("SAFE_MODE_FLOOR", "no_ddl"),
            ("DB_READ_ONLY", "1"),
            ("DB_PATH", "/data/qh.db"),
            ("PROGRESS_MS", "250"),
            ("USER", "isal"),
            ("DEST_DB_HOST", "b.internal"),
            ("DEST_DB_PASSWORD", "b-secret"),
            ("DEST_SAFE_MODE", "confirm"),
            ("DEST_SAFE_MODE_FLOOR", "read_only"),
            ("DEST_", ""),
        ]);

        // The source holds no key of the destination's, not even a blank one.
        let source = env.without_prefix("DEST_");
        assert!(keys(&source).iter().all(|key| !key.starts_with("DEST_")));
        assert!(!source.has_prefixed("DEST_"));
        assert_eq!(source.get("DB_HOST"), Some("a.internal"));
        assert_eq!(source.get("DB_PASSWORD"), Some("a-secret"));
        assert_eq!(source.get("SAFE_MODE_FLOOR"), Some("no_ddl"));
        assert_eq!(source.get("USER"), Some("isal"));
        let printed = format!("{source:?}");
        assert!(
            !printed.contains("b-secret") && !printed.contains("b.internal"),
            "{printed}"
        );

        // The destination holds the prefixed keys, stripped, and none of the source's:
        // no credential, no Safe Mode, no floor, no read-only mark, no login name.
        let dest = env.prefixed("DEST_");
        assert_eq!(
            keys(&dest),
            [
                "DB_HOST",
                "DB_PASSWORD",
                "DB_PATH",
                "PROGRESS_MS",
                "SAFE_MODE",
                "SAFE_MODE_FLOOR"
            ]
        );
        assert_eq!(dest.get("DB_HOST"), Some("b.internal"));
        assert_eq!(dest.get("DB_PASSWORD"), Some("b-secret"));
        assert_eq!(dest.get("SAFE_MODE"), Some("confirm"));
        // The floor is the destination's own entry; the top-level one is not copied over it
        // (or under it), the caller raises that as a second entry.
        assert_eq!(dest.get("SAFE_MODE_FLOOR"), Some("read_only"));
        assert_eq!(dest.get("DB_PATH"), Some("/data/qh.db"));
        assert_eq!(dest.get("PROGRESS_MS"), Some("250"));
        assert!(!dest.is_set("DB_READ_ONLY") && !dest.is_set("USER"));
        assert!(!dest.is_set("SSH_KEY_PASSPHRASE"));
    }

    #[test]
    fn a_destination_inherits_no_credential_when_it_brings_none() {
        let env = settings(&[
            ("DB_PASSWORD", "a-secret"),
            ("DB_URL", "postgres://u:a-secret@a/db"),
            ("DB_JWT", "a-jwt"),
            ("SSH_PASSWORD", "a-ssh"),
            ("SSH_KEY_PASSPHRASE", "a-phrase"),
            ("DEST_DB_HOST", "b.internal"),
        ]);
        let dest = env.prefixed("DEST_");
        assert_eq!(keys(&dest), ["DB_HOST"]);
        for key in SECRET_KEYS {
            assert!(!dest.is_set(key), "{key} leaked into the destination");
        }
    }

    #[test]
    fn the_shared_keys_come_from_the_top_level_only() {
        // A destination cannot redirect the audit log or the local database.
        let env = settings(&[("DEST_DB_PATH", "/elsewhere.db"), ("DEST_PROGRESS_MS", "1")]);
        let dest = env.prefixed("DEST_");
        assert!(!dest.is_set("DB_PATH") && !dest.is_set("PROGRESS_MS"));
        let env = settings(&[
            ("DB_PATH", "/data/qh.db"),
            ("DEST_DB_PATH", "/elsewhere.db"),
            ("DEST_PROGRESS_MS", "1"),
        ]);
        let dest = env.prefixed("DEST_");
        assert_eq!(dest.get("DB_PATH"), Some("/data/qh.db"));
        assert!(!dest.is_set("PROGRESS_MS"));
    }

    #[test]
    fn any_key_with_the_prefix_is_a_request_even_when_blank() {
        for pairs in [
            &[("DEST_", "")][..],
            &[("DEST_DB_KIND", "")],
            &[("DEST_X", "y")],
        ] {
            assert!(settings(pairs).has_prefixed("DEST_"), "{pairs:?}");
        }
        // Neither a lookalike nor the bare names are one.
        for pairs in [
            &[("DB_KIND", "mysql")][..],
            &[("XDEST_DB_KIND", "mysql")],
            &[("dest_db_kind", "mysql")],
            &[("DEST", "mysql")],
        ] {
            assert!(!settings(pairs).has_prefixed("DEST_"), "{pairs:?}");
        }
    }

    fn complete() -> Vec<(&'static str, &'static str)> {
        vec![
            ("DB_KIND", "postgres"),
            ("DB_HOST", "a"),
            ("DEST_DB_KIND", "mysql"),
            ("DEST_DB_HOST", "b"),
            ("DEST_SAFE_MODE", " Confirm "),
        ]
    }

    #[test]
    fn no_dest_key_is_no_destination_and_a_complete_set_is_split() {
        assert!(settings(&[("DB_KIND", "postgres")])
            .destination()
            .unwrap()
            .is_none());
        assert!(settings(&[]).destination().unwrap().is_none());

        let destination = settings(&complete()).destination().unwrap().unwrap();
        assert_eq!(destination.mode, TransferMode::Create);
        assert_eq!(destination.safe_mode, SafeMode::Confirm);
        assert_eq!(destination.dest.get("DB_KIND"), Some("mysql"));
        assert_eq!(destination.source.get("DB_KIND"), Some("postgres"));
        assert!(!destination.source.has_prefixed("DEST_"));

        let mut pairs = complete();
        pairs.push(("WRITE_MODE", " Replace "));
        let destination = settings(&pairs).destination().unwrap().unwrap();
        assert_eq!(destination.mode, TransferMode::Replace);
        // A URL alone is a connection.
        let by_url = settings(&[("DEST_DB_URL", "mysql://b/db"), ("DEST_SAFE_MODE", "full")]);
        assert!(by_url.destination().unwrap().is_some());
    }

    #[test]
    fn a_partial_destination_is_an_error_naming_what_it_lacks() {
        let error = |pairs: &[(&str, &str)]| settings(pairs).destination().unwrap_err();
        // Neither the kind nor the URL: a bare marker, blanks, a host alone, an alias.
        for pairs in [
            &[("DEST_", "")][..],
            &[
                ("DEST_DB_KIND", ""),
                ("DEST_DB_URL", "  "),
                ("DEST_SAFE_MODE", "full"),
            ],
            &[("DEST_DB_HOST", "b"), ("DEST_SAFE_MODE", "full")],
            &[("DEST_TRINO_URL", "trino://b"), ("DEST_SAFE_MODE", "full")],
            &[("DEST_DB_PASSWORD", "x")],
        ] {
            assert_eq!(error(pairs), DestinationError::NoConnection, "{pairs:?}");
        }
        // No Safe Mode, whatever the source's own says.
        for pairs in [
            &[("SAFE_MODE", "full"), ("DEST_DB_KIND", "mysql")][..],
            &[("DEST_DB_KIND", "mysql"), ("DEST_SAFE_MODE", "")],
            &[("DEST_DB_KIND", "mysql"), ("DEST_SAFE_MODE", "  ")],
        ] {
            assert_eq!(error(pairs), DestinationError::NoSafeMode, "{pairs:?}");
        }
        assert_eq!(
            error(&[("DEST_DB_KIND", "mysql"), ("DEST_SAFE_MODE", "ful")]),
            DestinationError::UnknownSafeMode("ful".to_owned())
        );
        // Append is refused, by name, in any spelling; a blank mode is unnameable.
        for spelling in ["append", "APPEND", " append "] {
            let mut pairs = complete();
            pairs.push(("WRITE_MODE", spelling));
            assert_eq!(error(&pairs), DestinationError::Append, "{spelling}");
        }
        for spelling in ["", "merge"] {
            let mut pairs = complete();
            pairs.push(("WRITE_MODE", spelling));
            assert!(matches!(
                error(&pairs),
                DestinationError::UnknownWriteMode(_)
            ));
        }
    }

    #[test]
    fn a_destination_error_never_repeats_a_credential() {
        // The only value the messages quote is the Safe Mode or the write mode the caller
        // typed. Everything else in the set, secrets included, is not there.
        let env = settings(&[
            ("DEST_DB_KIND", "mysql"),
            ("DEST_DB_PASSWORD", "pw-SENTINEL"),
            ("DEST_DB_URL", "mysql://u:url-SENTINEL@b/db"),
            ("DEST_SAFE_MODE", "oops"),
        ]);
        let message = env.destination().unwrap_err().to_string();
        assert!(!message.contains("SENTINEL"), "{message}");
        assert!(message.contains("DEST_SAFE_MODE"), "{message}");
    }

    #[test]
    fn a_flag_only_answers_to_the_documented_spellings() {
        for (raw, expected) in [
            ("1", true),
            ("TRUE", true),
            ("yes", true),
            ("On", true),
            ("0", false),
            ("false", false),
            ("no", false),
            ("OFF", false),
        ] {
            let env = settings(&[("HEADER", raw)]);
            assert_eq!(env.flag("HEADER", !expected), expected, "{raw}");
        }
        // Anything unrecognised, and a blank, keeps the default.
        for raw in ["", "  ", "maybe", "2", "yes please"] {
            let env = settings(&[("HEADER", raw)]);
            assert!(env.flag("HEADER", true), "{raw}");
            assert!(!env.flag("HEADER", false), "{raw}");
        }
    }
}
