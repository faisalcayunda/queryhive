//! Turning settings into a connection.
//!
//! This is `DatabaseConfig.from_env` (`exporter/drivers.py:780`) with its rules
//! kept, because they are the ones the app already sends: **`DB_*` wins over its
//! older `TRINO_*` alias**, a blank value means "unset", and a URL is optional —
//! the app sends the parts only, so the parts alone have to build a working
//! connection.
//!
//! # Where the TLS decision lives
//!
//! The Python engine handed `http_scheme` to the trino client and the `sslmode`
//! string to psycopg and pymysql, and each library decided what to do with it. The
//! Rust drivers take a [`TlsMode`] instead, so this module is where that decision
//! now happens, once:
//!
//! | Setting | Trino | PostgreSQL / MySQL |
//! |---|---|---|
//! | `http` | [`TlsMode::Disable`] | — |
//! | `https` | [`TlsMode::Require`] | — |
//! | `sslmode=disable` | — | [`TlsMode::Disable`] |
//! | unset | — | [`TlsMode::Prefer`] |
//! | `sslmode=require` | — | [`TlsMode::Require`] |
//! | `sslmode=verify-ca`/`verify-full` | — | [`TlsMode::Require`] |
//!
//! `DB_INSECURE` (or `TRINO_INSECURE`) turned on downgrades a required TLS
//! connection to [`TlsMode::RequireNoVerify`] — it never turns TLS *off*, which is
//! the same reading the Python engine's `verify=False` had.
//!
//! # Two rules that look like accidents and are not
//!
//! 1. **A password implies TLS on Trino.** Trino refuses BasicAuth over plaintext,
//!    so a password (or a port of 443/8443) promotes `http` to `https`, and a
//!    password with neither a port nor a URL moves the port to 443. Removing this
//!    would produce a connection that cannot authenticate.
//! 2. **Port 0 means "the driver's default"**, never "port zero". It is resolved by
//!    the command, which is the layer that knows which driver is in play.

use thiserror::Error;

use qh_driver::{ConnectionConfig, DriverKind, TlsMode};

use crate::env::{parse_flag, SettingError, Settings};

/// The `DB_*` name, then the older `TRINO_*` name it supersedes.
///
/// Both are read because the app's own settings table moved from `TRINO_*` to
/// `DB_*` when the second and third drivers arrived, and a stored connection from
/// before that must keep working.
pub const ALIASES: &[(&str, &str)] = &[
    ("DB_URL", "TRINO_URL"),
    ("DB_HOST", "TRINO_HOST"),
    ("DB_PORT", "TRINO_PORT"),
    ("DB_USER", "TRINO_USER"),
    ("DB_PASSWORD", "TRINO_PASSWORD"),
    ("DB_DATABASE", "TRINO_CATALOG"),
    ("DB_SCHEMA", "TRINO_SCHEMA"),
    ("DB_INSECURE", "TRINO_INSECURE"),
];

/// Every spelling of one setting, `DB_*` first.
pub fn setting_names(key: &str) -> (&str, Option<&str>) {
    match ALIASES.iter().find(|(modern, _)| *modern == key) {
        Some((modern, alias)) => (modern, Some(alias)),
        None => (key, None),
    }
}

/// Every spelling of one setting, for a message a user has to act on.
pub fn setting_label(key: &str) -> String {
    match setting_names(key) {
        (modern, Some(alias)) => format!("{modern} (or {alias})"),
        (modern, None) => modern.to_owned(),
    }
}

/// The kinds this engine can connect to, in the order the usage and error messages
/// list them.
pub const KINDS: [&str; 3] = ["trino", "postgres", "mysql"];

/// Why a connection could not be built from the settings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("unknown {key} '{kind}'; expected one of trino, postgres, mysql")]
    UnknownKind { key: String, kind: String },

    #[error("unknown kind '{0}'; expected one of trino, postgres, mysql")]
    UnknownUrlKind(String),

    #[error("need DB_URL or DB_HOST (or TRINO_URL / TRINO_HOST)")]
    NoHost,

    #[error("DB_SCHEME must be http or https, got '{0}'")]
    BadScheme(String),

    #[error("unknown sslmode '{0}'; expected disable, prefer, require, verify-ca or verify-full")]
    BadSSLMode(String),

    #[error("cannot read a host out of '{0}'")]
    NoUrlHost(String),

    #[error(transparent)]
    Setting(#[from] SettingError),
}

/// Resolve the connection described by `settings`.
pub fn build(settings: &Settings) -> Result<ConnectionConfig, ConfigError> {
    let (kind_key, kind_name) = pick(settings, "DB_KIND");
    // Whether the caller named a driver, as opposed to the trino default. It
    // matters for one thing only: a URL whose scheme names a driver. When
    // `DB_KIND` says nothing, the scheme decides; when it says `postgres`, it
    // decides, scheme or no scheme.
    let declared_kind = !kind_name.is_empty();
    let kind_name = if declared_kind {
        kind_name.to_ascii_lowercase()
    } else {
        "trino".to_owned()
    };
    let kind = DriverKind::parse(&kind_name).ok_or_else(|| ConfigError::UnknownKind {
        key: kind_key,
        kind: kind_name.clone(),
    })?;

    let (_, url) = pick(settings, "DB_URL");

    // Each part is an override: `None` when the caller did not set it, so a URL's
    // own value survives. A blank is an unset, which is why `_pick` and not `get`.
    let mut parts = Parts {
        kind,
        scheme: pick(settings, "DB_SCHEME").1.to_ascii_lowercase(),
        sslmode: pick(settings, "DB_SSLMODE").1.to_ascii_lowercase(),
        host: pick(settings, "DB_HOST").1,
        user: pick(settings, "DB_USER").1,
        password: optional(pick(settings, "DB_PASSWORD").1),
        database: pick(settings, "DB_DATABASE").1,
        schema: pick(settings, "DB_SCHEMA").1,
        port: None,
    };

    let (port_key, port_raw) = pick(settings, "DB_PORT");
    parts.port = if port_raw.is_empty() {
        None
    } else {
        Some(
            port_raw
                .parse::<i64>()
                .map_err(|_| SettingError::NotANumber {
                    key: port_key,
                    value: port_raw,
                })?,
        )
    };

    let insecure = parse_flag(&pick(settings, "DB_INSECURE").1, false);

    let mut resolved = if !url.is_empty() {
        from_url(&url, parts, declared_kind)?
    } else if !parts.host.is_empty() {
        parts
    } else {
        return Err(ConfigError::NoHost);
    };

    // The user falls back to the login name, then to `trino` for Trino alone: a
    // coordinator rejects an anonymous statement with a message that is harder to
    // act on than a name it will accept.
    if resolved.user.is_empty() {
        resolved.user = settings.text("USER", "");
        if resolved.user.is_empty() && resolved.kind == DriverKind::Trino {
            resolved.user = "trino".to_owned();
        }
    }

    // A password with neither a port nor a URL: Trino is about to be spoken to over
    // TLS whatever the scheme said, so the default port moves with it.
    if resolved.kind == DriverKind::Trino && resolved.password.is_some() && resolved.port.is_none()
    {
        resolved.port = Some(443);
    }

    resolved.into_config(insecure)
}

/// The connection as its individual parts, before the TLS and scheme rules run.
struct Parts {
    kind: DriverKind,
    host: String,
    /// `None` means "the driver's default port", never port zero.
    port: Option<i64>,
    user: String,
    password: Option<String>,
    database: String,
    schema: String,
    scheme: String,
    sslmode: String,
}

impl Parts {
    /// Apply the scheme/port rules and produce the config the drivers receive.
    fn into_config(self, insecure: bool) -> Result<ConnectionConfig, ConfigError> {
        let mut port = self.port.unwrap_or(0);
        // Assigned once in each branch below: the two families of server do not share
        // a spelling for this, so a default here would only be a value to overwrite.
        let tls: TlsMode;

        if self.kind == DriverKind::Trino {
            let scheme = if self.scheme.is_empty() {
                "http".to_owned()
            } else {
                self.scheme.clone()
            };
            if scheme != "http" && scheme != "https" {
                return Err(ConfigError::BadScheme(scheme));
            }
            // Trino refuses BasicAuth over plaintext, so a password implies TLS; the
            // standard HTTPS ports do too. Same rule as the Python engine's
            // `TrinoConfig.__post_init__`.
            let https = scheme == "https" || self.password.is_some() || port == 443 || port == 8443;
            tls = if !https {
                TlsMode::Disable
            } else if insecure {
                TlsMode::RequireNoVerify
            } else {
                TlsMode::Require
            };
            if port == 0 {
                port = if https { 443 } else { 8080 };
            }
        } else {
            // The stored vocabulary is libpq's, and it does not mean what our enum's names
            // suggest: libpq's `require` encrypts **without** verifying, and only
            // `verify-ca`/`verify-full` ask for the certificate to be checked. Mapping
            // `require` onto our verifying mode would refuse every internal, self-signed
            // server that works today, which is the one upgrade nobody forgives.
            let required = match self.sslmode.as_str() {
                "disable" => TlsMode::Disable,
                "require" => TlsMode::RequireNoVerify,
                "verify-ca" | "verify-full" => TlsMode::Require,
                // psycopg's own default, and what a driver does when nobody said anything:
                // encrypt when the server offers it.
                "" | "prefer" => TlsMode::Prefer,
                // A spelling nobody recognises is refused rather than treated as a default:
                // a typo in `sslmode` deciding whether the connection is encrypted is not a
                // decision to make silently.
                other => return Err(ConfigError::BadSSLMode(other.to_owned())),
            };
            tls = if insecure && required != TlsMode::Disable {
                TlsMode::RequireNoVerify
            } else {
                required
            };
        }

        let mut config = ConnectionConfig::new(self.kind, self.host, 0, self.user);
        config.port = u16::try_from(port).unwrap_or(0);
        config.password = self.password;
        config.database = optional(self.database);
        config.schema = optional(self.schema);
        config.tls = tls;
        config.insecure = insecure;
        Ok(config)
    }
}

/// A blank part is absent, not empty: `"?schema="` and no schema at all behave
/// alike, which is what the Python engine's `v not in (None, "")` meant.
fn optional(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// `(the name that supplied this setting, its value)`; `DB_*` wins over the alias.
///
/// The name comes back so a validation error can quote the variable the caller
/// actually set instead of one they never used.
fn pick(settings: &Settings, key: &str) -> (String, String) {
    let (modern, alias) = setting_names(key);
    for name in [Some(modern), alias].into_iter().flatten() {
        let value = settings.text(name, "");
        if !value.is_empty() {
            return (name.to_owned(), value);
        }
    }
    (modern.to_owned(), String::new())
}

/// Build from a connection URL, with the `DB_*` parts winning over its own.
///
/// ```text
/// trino://user:secret@trino.internal:8443/hive/analytics   catalog, schema
/// postgresql://user:secret@pg.internal/appdb?sslmode=require
/// mysql://user:secret@mysql.internal:3306/shop
/// ```
///
/// A URL with no `//` is read as host-first, which is how a caller who pasted
/// `host:8080/hive` expects it to behave.
fn from_url(url: &str, overrides: Parts, declared_kind: bool) -> Result<Parts, ConfigError> {
    let parsed = Parsed::parse(url);

    let scheme_kind = match parsed.scheme.as_str() {
        "trino" | "http" | "https" => DriverKind::Trino,
        "postgres" | "postgresql" => DriverKind::Postgres,
        "mysql" => DriverKind::Mysql,
        // Not a driver name: an unknown scheme is not a reason to refuse the URL,
        // because a URL without one is read as host-first and lands here too.
        _ => DriverKind::Trino,
    };
    // `DB_KIND` wins when it named a driver; otherwise the scheme does. The Python
    // engine let its own trino default shadow the scheme here, so
    // `DB_URL=postgresql://…` with no `DB_KIND` connected to Trino — the app always
    // sends both, so the difference only shows up on the command line, where
    // reading the scheme is what the caller meant.
    let kind = if declared_kind {
        overrides.kind
    } else {
        scheme_kind
    };
    if parsed.host.is_empty() {
        return Err(ConfigError::NoUrlHost(url.to_owned()));
    }

    let mut base = Parts {
        kind,
        host: parsed.host.clone(),
        port: None,
        user: parsed.user.clone(),
        password: parsed.password.clone(),
        database: String::new(),
        schema: String::new(),
        scheme: String::new(),
        sslmode: String::new(),
    };

    if kind == DriverKind::Trino {
        let scheme = if parsed.scheme == "https" {
            "https".to_owned()
        } else {
            "http".to_owned()
        };
        base.scheme = scheme.clone();
        base.port = Some(
            parsed
                .port
                .unwrap_or(if scheme == "https" { 443 } else { 8080 }),
        );
        base.database = parsed.segment(0);
        base.schema = parsed.segment(1);
    } else {
        base.port = Some(parsed.port.unwrap_or(0));
        base.database = parsed.segment(0);
        base.sslmode = parsed.query("sslmode");
    }

    Ok(base.overridden_by(overrides))
}

impl Parts {
    /// Every part the caller set replaces the URL's own.
    fn overridden_by(self, overrides: Parts) -> Parts {
        Parts {
            // The driver is already settled above, by `DB_KIND` or the scheme;
            // there is nothing left to override it with here.
            kind: self.kind,
            host: or(self.host, overrides.host),
            port: overrides.port.or(self.port),
            user: or(self.user, overrides.user),
            password: overrides.password.or(self.password),
            database: or(self.database, overrides.database),
            schema: or(self.schema, overrides.schema),
            scheme: or(self.scheme, overrides.scheme),
            sslmode: or(self.sslmode, overrides.sslmode),
        }
    }
}

fn or(base: String, override_value: String) -> String {
    if override_value.is_empty() {
        base
    } else {
        override_value
    }
}

/// The parts of a connection URL.
struct Parsed {
    scheme: String,
    host: String,
    port: Option<i64>,
    user: String,
    password: Option<String>,
    segments: Vec<String>,
    query: Vec<(String, String)>,
}

impl Parsed {
    fn parse(url: &str) -> Self {
        // `urlparse(url if "//" in url else f"//{url}", scheme="http")`: a URL with
        // no scheme of its own is read as host-first against `http`, rather than
        // as a scheme whose remainder cannot be parsed.
        let (scheme, rest) = match url.split_once("://") {
            Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest.to_owned()),
            None => ("http".to_owned(), url.trim_start_matches("//").to_owned()),
        };

        // Authority, then path, then query.
        let (authority, tail) = match rest.find('/') {
            Some(index) => (rest[..index].to_owned(), rest[index..].to_owned()),
            None => (rest.clone(), String::new()),
        };
        let (path, query) = match tail.split_once('?') {
            Some((path, query)) => (path.to_owned(), query.to_owned()),
            None => (tail, String::new()),
        };

        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((userinfo, hostport)) => (Some(userinfo.to_owned()), hostport.to_owned()),
            None => (None, authority),
        };

        let (user, password) = match userinfo.as_deref() {
            Some(userinfo) => match userinfo.split_once(':') {
                Some((user, password)) => (percent_decode(user), Some(percent_decode(password))),
                None => (percent_decode(userinfo), None),
            },
            None => (String::new(), None),
        };

        // A bracketed IPv6 literal keeps its colons; anything else splits on the
        // last colon so `host:5432` works and a bare host does not become a port.
        let (host, port) = if let Some(end) = hostport.rfind(']') {
            match hostport[end + 1..].strip_prefix(':') {
                Some(port) => (hostport[..=end].to_owned(), port.parse::<i64>().ok()),
                None => (hostport.clone(), None),
            }
        } else {
            match hostport.rsplit_once(':') {
                Some((host, port))
                    if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
                {
                    (host.to_owned(), port.parse::<i64>().ok())
                }
                _ => (hostport, None),
            }
        };

        let segments = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(percent_decode)
            .collect();
        let query = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (percent_decode(key), percent_decode(value)),
                None => (percent_decode(pair), String::new()),
            })
            .collect();

        Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            user,
            password,
            segments,
            query,
        }
    }

    fn segment(&self, index: usize) -> String {
        self.segments.get(index).cloned().unwrap_or_default()
    }

    fn query(&self, key: &str) -> String {
        self.query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

/// Percent-decoding, as `urllib.parse.unquote` does it: a `%` that is not followed
/// by two hex digits is left as written rather than dropped.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
