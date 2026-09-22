//! Settings, and the readers that turn text into values.
//!
//! Every setting comes from the environment and nothing is read from disk: that is
//! the whole configuration story of this engine, inherited unchanged from
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

use thiserror::Error;

/// One setting that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SettingError {
    /// The Python engine's wording, quotes included, so a user who has seen one of
    /// these messages recognises the next one.
    #[error("{key} must be a whole number, got '{value}'")]
    NotANumber { key: String, value: String },
}

/// Every setting this engine reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    values: BTreeMap<String, String>,
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

    /// A boolean setting: `1/true/yes/on` and `0/false/no/off`, case-insensitive.
    ///
    /// A blank value, or anything else unrecognised, keeps `default` — so only the
    /// documented spellings ever change what the caller asked for.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        parse_flag(&self.text(key, ""), default)
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
